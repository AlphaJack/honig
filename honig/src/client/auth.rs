//! The client's side of security: sending the credentials an operation's requirements ask for.

#[cfg(feature = "cookie")]
use crate::auth::add_cookie;
use crate::auth::{Credentials, KeyIn, Requirement, Scheme};
#[cfg(feature = "base64")]
use crate::base64::base64;
use crate::{ClientError, Message, Request, params::pct};

/// Applies the first requirement whose schemes all have credentials. `with` pairs each scheme by name with the client
/// method that sets its credential.
///
/// An empty requirement (`{}`) means that credentials are optional. It sends none. So it is the last resort and the
/// credentials that you configured are sent.
///
/// # Errors
/// - [`ClientError::MissingCredentials`] when the client has the credentials of no requirement and none is optional: a
///   server would answer 401, so the request is not sent.
/// - A credential that a header or the query cannot carry: it contains a line break or another control character.
pub fn apply_auth(
    req: &mut Request,
    creds: &Credentials,
    requirements: &[&[Requirement]],
    with: &[(&str, &str)],
) -> Result<(), ClientError> {
    let Some(alt) = requirements
        .iter()
        .filter(|alt| !alt.is_empty())
        .find(|alt| alt.iter().all(|(s, _)| matches!(s, Scheme::MutualTls(_)) || creds.contains_key(s.name())))
    else {
        let optional = requirements.iter().any(|alt| alt.is_empty());
        if optional || requirements.is_empty() {
            return Ok(());
        }
        let listed = |sep: &str, item: &dyn Fn(&str) -> Option<String>| {
            let alt = |alt: &&[Requirement]| alt.iter().filter_map(|(s, _)| item(s.name())).collect::<Vec<_>>();
            requirements.iter().map(|a| alt(a).join(sep)).collect::<Vec<_>>().join(" or ")
        };
        let setter = |name: &str| with.iter().find(|(n, _)| *n == name).map(|(_, f)| format!("`{f}`"));
        let (names, setters) = (listed(" + ", &|n| Some(format!("`{n}`"))), listed(" and ", &setter));
        return Err(ClientError::MissingCredentials(format!("{names}: call {setters}")));
    };
    for (s, _) in *alt {
        let Some(c) = creds.get(s.name()) else {
            continue;
        };
        match s {
            Scheme::ApiKey(_, KeyIn::Header, p) => req.set_header(p, c.clone())?,
            Scheme::ApiKey(_, KeyIn::Query, p) => {
                let target = req.uri().to_string();
                let sep = if target.contains('?') { '&' } else { '?' };
                let target = format!("{target}{sep}{}={}", pct(p, false), pct(c, false));
                *req.uri_mut() = http::Uri::try_from(target).map_err(|e| ClientError::Encode(e.to_string()))?;
            }
            #[cfg(feature = "cookie")]
            Scheme::ApiKey(_, KeyIn::Cookie, p) => add_cookie(req, &format!("{p}={c}"))?,
            #[cfg(feature = "base64")]
            Scheme::Basic(_) => req.set_header("Authorization", format!("Basic {}", base64(c.as_bytes())))?,
            Scheme::Bearer(_) => req.set_header("Authorization", format!("Bearer {c}"))?,
            Scheme::Http(_, scheme) => req.set_header("Authorization", format!("{scheme} {c}"))?,
            Scheme::MutualTls(_) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        crate::__private::new_request("GET", "/a".to_owned(), vec![]).unwrap()
    }

    /// An operation that needs credentials fails before it is sent when the client has none for any alternative. One
    /// alternative with its credentials, or an optional one, sends.
    #[test]
    fn missing_credentials_fail_before_the_request_is_sent() {
        let key: &[Requirement] = &[(Scheme::ApiKey("key", KeyIn::Header, "X-Key"), &[])];
        let session: &[Requirement] = &[(Scheme::ApiKey("session", KeyIn::Query, "s"), &[])];
        let creds =
            |names: &[&str]| -> Credentials { names.iter().map(|n| ((*n).to_owned(), "v".to_owned())).collect() };
        let with = &[("key", "Client::with_key"), ("session", "Client::with_session")];
        let err = apply_auth(&mut request(), &creds(&[]), &[key, session], with).unwrap_err();
        let wanted = "`key` or `session`: call `Client::with_key` or `Client::with_session`";
        assert!(matches!(&err, ClientError::MissingCredentials(w) if w == wanted), "{err}");
        assert!(apply_auth(&mut request(), &creds(&["session"]), &[key, session], with).is_ok());
        assert!(
            apply_auth(&mut request(), &creds(&[]), &[key, &[]], with).is_ok(),
            "an optional alternative sends none"
        );
        assert!(apply_auth(&mut request(), &creds(&[]), &[], with).is_ok(), "no security needs none");
    }
}
