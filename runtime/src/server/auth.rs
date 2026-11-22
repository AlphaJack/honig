//! The server's side of security: which requirements a request's credentials satisfy.

use crate::auth::{KeyIn, Requirement, Scheme};
#[cfg(feature = "base64")]
use crate::base64::unbase64;
use crate::params::Query;
#[cfg(feature = "cookie")]
use crate::params::parse_cookies;

/// What the satisfied security requirements presented and require.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SatisfiedSecurity {
    /// Credentials of every satisfied requirement with their scheme. Each scheme comes once.
    pub auth: Vec<(Scheme, String)>,
    /// Scopes that the first satisfied requirement requires across its schemes.
    pub scopes: Vec<&'static str>,
}

impl SatisfiedSecurity {
    /// Keeps the credentials and scopes of `alternative`, the one that passed. A credential of another alternative was
    /// presented but no `verify_` method accepted it.
    pub(crate) fn keep(&mut self, alternative: &[Requirement]) {
        self.auth.retain(|(s, _)| alternative.iter().any(|(r, _)| r == s));
        self.scopes = alternative.iter().flat_map(|(_, scopes)| scopes.iter().copied()).collect();
    }
}

/// Who is calling and the scopes that their credential grants: what a `verify_` method of the `Api` found.
///
/// The `Api` has one `verify_` method per security scheme that its operations use, and does not compile without it.
/// The server checks that a request presents the credentials that its operation declares. It calls the method of each
/// scheme of an alternative, at most once per request, and checks the scopes that the credentials grant. Each
/// operation then gets what its `security` asks for:
///
/// | The operation's `security` | `ctx.principal` | When no alternative passes |
/// |---|---|---|
/// | none (`security: []`) | `()` | the handler runs |
/// | an empty alternative (`[{}, {apiKey: []}]`) | `Option<Principal>` | `None`; 401 when a method refused a credential |
/// | credentials required | `Principal` | 401; 403 when only a scope is missing |
///
/// The principal is the one of the first scheme of the alternative that passes. Layers get it from the request's
/// extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified<P> {
    /// Who is calling.
    pub principal: P,
    /// The scopes or roles that the credential grants.
    pub scopes: Vec<&'static str>,
}

impl<P> Verified<P> {
    /// Returns `principal` with no scopes: what a scheme without scopes or roles grants.
    pub const fn new(principal: P) -> Self {
        Self { principal, scopes: Vec::new() }
    }
}

/// What a scheme with scopes or roles grants: who is calling and the scopes of the credential, as the generated scope
/// enum of the scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Granted<P, S> {
    /// Who is calling.
    pub principal: P,
    /// The scopes or roles that the credential grants.
    pub scopes: Vec<S>,
}

/// The credentials of an `http basic` scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Basic<'a> {
    /// The user.
    pub user: &'a str,
    /// The password.
    pub password: &'a str,
}

impl<'a> Basic<'a> {
    /// Returns the user and the password of `user:password`. A credential without `:` is a user with no password.
    #[must_use]
    pub fn of(credential: &'a str) -> Self {
        let (user, password) = credential.split_once(':').unwrap_or((credential, ""));
        Self { user, password }
    }
}

/// Returns, on the server side, the credentials and scopes of every requirement whose credentials
/// are all present. `None` if there is none. Whether they are valid is for the `Api` to check.
pub fn check_auth(
    headers: &http::HeaderMap,
    query: &Query<'_>,
    requirements: &[&[Requirement]],
) -> Option<SatisfiedSecurity> {
    if requirements.is_empty() {
        return Some(SatisfiedSecurity::default());
    }
    let mut out: Option<SatisfiedSecurity> = None;
    for (found, scopes) in satisfied(headers, query, requirements) {
        let g = out.get_or_insert_with(|| SatisfiedSecurity { scopes, ..SatisfiedSecurity::default() });
        for (s, c) in found {
            if !g.auth.iter().any(|(n, _)| n.name() == s.name()) {
                g.auth.push((s, c));
            }
        }
    }
    out
}

/// One satisfied requirement: its credentials with their scheme and the scopes that it requires.
type Satisfied = (Vec<(Scheme, String)>, Vec<&'static str>);

/// Returns each requirement whose credentials are all present.
fn satisfied(headers: &http::HeaderMap, query: &Query<'_>, requirements: &[&[Requirement]]) -> Vec<Satisfied> {
    let header = |name: &str| crate::message::first_header(headers, name);
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
                    // UTF-8 or else ISO-8859-1: some clients encode Basic credentials in Latin-1.
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
        out.push((found, scopes));
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
    use crate::{Context, RequestEvent};

    const READ: Scheme = Scheme::Bearer("pat");
    const KEY: Scheme = Scheme::ApiKey("key", KeyIn::Header, "X-Key");

    fn request(headers: &[(&'static str, &'static str)]) -> crate::Request {
        let mut r = crate::message::new_request("GET", "/".to_owned(), vec![]).unwrap();
        for (k, v) in headers {
            crate::Message::set_header(&mut r, k, *v).unwrap();
        }
        r
    }

    fn check(headers: &[(&'static str, &'static str)], requirements: &[&[Requirement]]) -> Option<SatisfiedSecurity> {
        check_auth(request(headers).headers(), &[], requirements)
    }

    #[test]
    fn either_permission_on_one_token_presents_it_once() {
        let alts: &[&[Requirement]] = &[&[(READ, &["issues:read"])], &[(READ, &["issues:write"])]];
        let g = check(&[("Authorization", "Bearer t")], alts).expect("token present");
        assert_eq!(g.scopes, ["issues:read"], "first requirement");
        assert_eq!(g.auth, [(READ, "t".to_owned())], "the token once");
    }

    #[test]
    fn only_alternatives_with_credentials_count() {
        let alts: &[&[Requirement]] = &[&[(KEY, &["admin"])], &[(READ, &["issues:read"])]];
        let g = check(&[("Authorization", "Bearer t")], alts).expect("token present");
        assert_eq!(g.scopes, ["issues:read"], "no key, so `admin` cannot be what grants access");
    }

    #[test]
    fn and_requires_every_scheme() {
        let alts: &[&[Requirement]] = &[&[(KEY, &["a"]), (READ, &["b"])]];
        assert_eq!(check(&[("Authorization", "Bearer t")], alts), None, "the key is missing");
        let g = check(&[("Authorization", "Bearer t"), ("X-Key", "k")], alts).expect("both present");
        assert_eq!(g.scopes, ["a", "b"]);
    }

    /// A layer for any API gets each credential by the kind of its scheme, not by the scheme's name.
    #[cfg(feature = "base64")]
    #[test]
    fn credentials_are_presented_by_kind() {
        use crate::Secured as _;
        const BASIC: Scheme = Scheme::Basic("login");
        let alts: &[&[Requirement]] = &[&[(KEY, &[])], &[(BASIC, &[])]];
        let basic = format!("Basic {}", crate::base64::base64(b"ann:open sesame"));
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
    fn a_basic_credential_splits_at_the_first_colon() {
        assert_eq!(Basic::of("ann:a:b"), Basic { user: "ann", password: "a:b" });
        assert_eq!(Basic::of("ann"), Basic { user: "ann", password: "" });
    }

    #[test]
    fn a_context_gives_the_headers_of_its_request() {
        let req = request(&[("X-Trace", "abc")]);
        let event = RequestEvent::new(Some(crate::server::serve::TestOp), "GET");
        let ctx = Context::new(&req, crate::server::serve::TestOp, SatisfiedSecurity::default(), &event);
        assert_eq!(ctx.headers().get("X-Trace").map(http::HeaderValue::as_bytes), Some(&b"abc"[..]));
    }
}
