//! Security schemes that both halves name and the helpers that they share. The client applies credentials
//! (`client::auth`). The server checks them (`server::auth`).

use std::collections::BTreeMap;

use crate::{Error, Request};

/// One security scheme as declared in `components.securitySchemes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    /// `apiKey`: (scheme name, location, parameter name).
    ApiKey(&'static str, KeyIn, &'static str),
    /// `http basic`: credentials are `user:password`.
    #[cfg(feature = "base64")]
    Basic(&'static str),
    /// `http bearer`, `OAuth2` and `OpenID` Connect.
    Bearer(&'static str),
    /// Another `http` scheme: (scheme name, auth-scheme token).
    Http(&'static str, &'static str),
    /// mutualTLS: satisfied by the transport.
    MutualTls(&'static str),
}

/// A credential that a request presented, by the kind of its scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presented<'a> {
    /// `http bearer`, `OAuth2` and `OpenID` Connect: the token.
    Bearer(&'a str),
    /// `http basic`: the user and the password.
    Basic {
        /// The user.
        user: &'a str,
        /// The password.
        password: &'a str,
    },
    /// `apiKey`: the key, from its header, query parameter or cookie.
    ApiKey(&'a str),
    /// Another `http` scheme: its auth-scheme token (`Digest`) and the credential after it.
    Http {
        /// The auth-scheme token of the `Authorization` header.
        scheme: &'static str,
        /// The credential after it.
        value: &'a str,
    },
    /// `mutualTLS`: the transport carries it. A layer verifies the certificate.
    MutualTls,
}

#[cfg(feature = "server")]
impl<'a> Presented<'a> {
    /// Returns the credential `value` of `scheme`.
    pub(crate) fn of(scheme: Scheme, value: &'a str) -> Self {
        match scheme {
            Scheme::ApiKey(..) => Self::ApiKey(value),
            #[cfg(feature = "base64")]
            Scheme::Basic(_) => {
                let (user, password) = value.split_once(':').unwrap_or((value, ""));
                Self::Basic { user, password }
            }
            Scheme::Bearer(_) => Self::Bearer(value),
            Scheme::Http(_, scheme) => Self::Http { scheme, value },
            Scheme::MutualTls(_) => Self::MutualTls,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Where an `apiKey` scheme carries its key.
pub enum KeyIn {
    /// In a request header.
    Header,
    /// In the query string.
    Query,
    /// In a cookie: the server reads it with the `cookie` feature.
    #[cfg(feature = "cookie")]
    Cookie,
}

impl Scheme {
    /// Returns the scheme name from `components.securitySchemes`.
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "base64")]
            Self::Basic(n) => n,
            Self::ApiKey(n, ..) | Self::Bearer(n) | Self::Http(n, _) | Self::MutualTls(n) => n,
        }
    }
}

/// Credentials by scheme name. Basic credentials are `user:password`.
pub type Credentials = BTreeMap<String, String>;

/// One scheme of a security requirement and the scopes that it must grant
/// (`OAuth2` / `OpenID` scopes or roles for other schemes).
pub type Requirement = (Scheme, &'static [&'static str]);

/// Adds `pair` to the `Cookie` header of `req`.
///
/// # Errors
/// The pair holds a line break or another control character.
pub(crate) fn add_cookie(req: &mut Request, pair: &str) -> Result<(), Error> {
    let headers = req.headers_mut();
    let value = match headers.get(http::header::COOKIE).and_then(|v| v.to_str().ok()) {
        Some(sent) => format!("{sent}; {pair}"),
        None => pair.to_owned(),
    };
    let value = http::HeaderValue::try_from(value).map_err(|e| Error::Encode(format!("cookie: {e}")))?;
    headers.insert(http::header::COOKIE, value);
    Ok(())
}
