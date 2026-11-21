//! The client's side of security: sending the credentials an operation's requirements ask for.

#[cfg(feature = "cookie")]
use crate::auth::add_cookie;
use crate::auth::{Credentials, KeyIn, Requirement, Scheme};
#[cfg(feature = "base64")]
use crate::base64::base64;
use crate::{ClientError, Message, Request, params::pct};

/// Applies the first requirement whose schemes all have credentials.
///
/// An empty requirement (`{}`) means that credentials are optional. It sends none. So it is the last resort and the
/// credentials that you configured are sent.
///
/// # Errors
/// A credential that a header or the query cannot carry: it contains a line break or another control character.
pub fn apply_auth(req: &mut Request, creds: &Credentials, requirements: &[&[Requirement]]) -> Result<(), ClientError> {
    let Some(alt) = requirements
        .iter()
        .filter(|alt| !alt.is_empty())
        .find(|alt| alt.iter().all(|(s, _)| matches!(s, Scheme::MutualTls(_)) || creds.contains_key(s.name())))
    else {
        return Ok(());
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
