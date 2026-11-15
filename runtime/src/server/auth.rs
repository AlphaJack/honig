//! The server's side of security: which requirements a request's credentials satisfy.

use crate::Query;
use crate::auth::{KeyIn, Requirement, Scheme};
#[cfg(feature = "cookie")]
use crate::parse_cookies;
#[cfg(feature = "base64")]
use crate::unbase64;

/// What the satisfied security requirements presented and require.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SatisfiedSecurity {
    /// Credentials of every satisfied requirement with their scheme. Each scheme comes once.
    pub auth: Vec<(Scheme, String)>,
    /// Scopes that the first satisfied requirement requires across its schemes.
    pub scopes: Vec<&'static str>,
    /// Scopes of every requirement whose credentials are present in document
    /// order. `[a] OR [b]` on one token puts both here. The request is
    /// allowed when the credential holds either set.
    pub requirements: Vec<Vec<&'static str>>,
    /// A satisfied requirement relies on `mutualTLS`. Only the TLS layer can verify it.
    pub mutual_tls: bool,
}

/// Returns, on the server side, the credentials and scopes of every requirement whose credentials
/// are all present. `None` if there is none.
///
/// Which scopes a credential
/// actually holds is for your authentication layer to check
/// ([`crate::Secured::scopes_held`]).
pub fn check_auth(
    headers: &http::HeaderMap,
    query: &Query<'_>,
    requirements: &[&[Requirement]],
) -> Option<SatisfiedSecurity> {
    if requirements.is_empty() {
        return Some(SatisfiedSecurity::default());
    }
    let mut out: Option<SatisfiedSecurity> = None;
    for (found, scopes, mutual_tls) in satisfied(headers, query, requirements) {
        let g = out.get_or_insert_with(|| SatisfiedSecurity { scopes: scopes.clone(), ..SatisfiedSecurity::default() });
        g.mutual_tls |= mutual_tls;
        for (s, c) in found {
            if !g.auth.iter().any(|(n, _)| n.name() == s.name()) {
                g.auth.push((s, c));
            }
        }
        g.requirements.push(scopes);
    }
    out
}

/// One satisfied requirement: its credentials with their scheme, the scopes that it requires and whether it relies
/// on `mutualTLS`.
type Satisfied = (Vec<(Scheme, String)>, Vec<&'static str>, bool);

/// Returns each requirement whose credentials are all present.
fn satisfied(headers: &http::HeaderMap, query: &Query<'_>, requirements: &[&[Requirement]]) -> Vec<Satisfied> {
    let header = |name: &str| crate::first_header(headers, name);
    let auth = header("authorization").unwrap_or_default();
    let auth = auth.as_ref();
    let mut out = vec![];
    'alt: for alt in requirements {
        let mut found = vec![];
        let mut scopes = vec![];
        for (s, required) in *alt {
            let cred = match s {
                Scheme::ApiKey(_, KeyIn::Header, p) => header(p).map(std::borrow::Cow::into_owned),
                Scheme::ApiKey(_, KeyIn::Query, p) => {
                    query.iter().find(|(k, _)| k == p).map(|(_, v)| v.clone().into_owned())
                }
                #[cfg(feature = "cookie")]
                Scheme::ApiKey(_, KeyIn::Cookie, p) => parse_cookies(&header("cookie").unwrap_or_default())
                    .into_iter()
                    .find(|(k, _)| k == p)
                    .map(|(_, v)| v),
                #[cfg(feature = "base64")]
                Scheme::Basic(_) => {
                    // UTF-8 or else ISO-8859-1. The `requests` of Python and OkHttp encode Basic credentials in
                    // Latin-1.
                    strip_scheme(auth, "basic")
                        .and_then(unbase64)
                        .map(|b| String::from_utf8(b).unwrap_or_else(|e| crate::header::latin1(e.as_bytes())))
                }
                Scheme::Bearer(_) => strip_scheme(auth, "bearer").map(String::from),
                Scheme::Http(_, scheme) => strip_scheme(auth, scheme).map(String::from),
                Scheme::MutualTls(_) => Some(String::new()),
            };
            // An empty credential (`X-API-Key:` or `Bearer `) is no credential.
            // Mutual TLS has none to carry.
            match cred {
                Some(c) if !c.trim().is_empty() || matches!(s, Scheme::MutualTls(_)) => found.push((*s, c)),
                Some(_) | None => continue 'alt,
            }
            scopes.extend_from_slice(required);
        }
        let mutual_tls = alt.iter().any(|(s, _)| matches!(s, Scheme::MutualTls(_)));
        out.push((found, scopes, mutual_tls));
    }
    out
}

fn strip_scheme<'a>(auth: &'a str, scheme: &str) -> Option<&'a str> {
    let (s, rest) = auth.split_once(' ')?;
    s.eq_ignore_ascii_case(scheme).then_some(rest.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, RequestEvent, Secured as _};

    const READ: Scheme = Scheme::Bearer("pat");
    const KEY: Scheme = Scheme::ApiKey("key", KeyIn::Header, "X-Key");

    fn request(headers: &[(&'static str, &'static str)]) -> crate::Request {
        let mut r = crate::new_request("GET", "/".to_owned(), vec![]).unwrap();
        for (k, v) in headers {
            crate::Message::set_header(&mut r, k, *v).unwrap();
        }
        r
    }

    fn check(headers: &[(&'static str, &'static str)], requirements: &[&[Requirement]]) -> Option<SatisfiedSecurity> {
        check_auth(request(headers).headers(), &[], requirements)
    }

    #[test]
    fn either_permission_on_one_token_keeps_both_alternatives() {
        let alts: &[&[Requirement]] = &[&[(READ, &["issues:read"])], &[(READ, &["issues:write"])]];
        let g = check(&[("Authorization", "Bearer t")], alts).expect("token present");
        assert_eq!(g.requirements, [vec!["issues:read"], vec!["issues:write"]]);
        assert_eq!(g.scopes, ["issues:read"], "first requirement, as before");
        assert_eq!(g.auth, [(READ, "t".to_owned())], "the token once");
        let req = request(&[]);
        let event = RequestEvent::new(Some(crate::server::serve::TestOp), "GET");
        let ctx = Context::new(&req, crate::server::serve::TestOp, g, &event);
        assert!(ctx.scopes_held(|s| s == "issues:write"), "a write-only token passes the OR");
        assert!(ctx.scopes_held(|s| s == "issues:read"));
        assert!(!ctx.scopes_held(|_| false));
    }

    #[test]
    fn only_alternatives_with_credentials_count() {
        let alts: &[&[Requirement]] = &[&[(KEY, &["admin"])], &[(READ, &["issues:read"])]];
        let g = check(&[("Authorization", "Bearer t")], alts).expect("token present");
        assert_eq!(g.requirements, [vec!["issues:read"]], "no key, so `admin` cannot be what grants access");
        assert_eq!(g.scopes, ["issues:read"]);
    }

    #[test]
    fn and_requires_every_scheme_and_unions_scopes() {
        let alts: &[&[Requirement]] = &[&[(KEY, &["a"]), (READ, &["b"])]];
        assert_eq!(check(&[("Authorization", "Bearer t")], alts), None, "the key is missing");
        let g = check(&[("Authorization", "Bearer t"), ("X-Key", "k")], alts).expect("both present");
        assert_eq!(g.requirements, [vec!["a", "b"]]);
    }

    /// A layer for any API reads each credential by the kind of its scheme, not by the scheme's name.
    #[cfg(feature = "base64")]
    #[test]
    fn credentials_are_presented_by_kind() {
        const BASIC: Scheme = Scheme::Basic("login");
        let alts: &[&[Requirement]] = &[&[(KEY, &[])], &[(BASIC, &[])]];
        let basic = format!("Basic {}", crate::base64(b"ann:open sesame"));
        let mut req = request(&[("X-Key", "k1")]);
        crate::Message::set_header(&mut req, "Authorization", basic).unwrap();
        let g = check_auth(req.headers(), &[], alts).expect("both present");
        let event = RequestEvent::new(Some(crate::server::serve::TestOp), "GET");
        let ctx = Context::new(&req, crate::server::serve::TestOp, g, &event);
        assert_eq!(
            ctx.presented(),
            [crate::Presented::ApiKey("k1"), crate::Presented::Basic { user: "ann", password: "open sesame" }]
        );
    }

    #[test]
    fn no_security_and_no_scopes_are_held() {
        assert_eq!(check(&[], &[]), Some(SatisfiedSecurity::default()));
        let req = request(&[]);
        let event = RequestEvent::new(Some(crate::server::serve::TestOp), "GET");
        let ctx = Context::new(&req, crate::server::serve::TestOp, SatisfiedSecurity::default(), &event);
        assert!(ctx.scopes_held(|_| false), "nothing required");
    }
}
