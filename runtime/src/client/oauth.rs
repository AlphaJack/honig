//! The `OAuth` flows of the client: token grants, PKCE, device authorization and discovery.

#[cfg(feature = "pkce")]
use crate::base64::base64url;

// `Pkce::new`: wasm32 has no OS randomness without configuration. A browser passes its own verifier.
#[cfg(all(feature = "pkce", not(target_arch = "wasm32")))]
mod random;
use crate::{Error, Message, Request, Response, Transport, base64, json_decode, pct};

/// An `OAuth2` access token from a token endpoint.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Token {
    /// The bearer credential.
    pub access_token: String,
    /// Usually `Bearer`.
    #[serde(default)]
    pub token_type: String,
    /// Lifetime in seconds if the server says.
    #[serde(default)]
    pub expires_in: Option<u64>,
    /// For [`refresh_token`] if the server issued one.
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Granted scopes if they differ from the requested ones.
    #[serde(default)]
    pub scope: Option<String>,
}

/// Sends a token request with form `fields`. A given secret means client
/// authentication by HTTP basic auth. Otherwise `client_id` goes in the form.
async fn token_call<T: Transport>(
    transport: &T,
    token_url: &str,
    mut fields: Vec<String>,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<Token, Error> {
    if client_secret.is_none() {
        fields.push(format!("client_id={}", pct(client_id, false)));
    }
    let mut req = token_request(token_url, &fields)?;
    if let Some(secret) = client_secret {
        req.set_header(
            "Authorization",
            format!("Basic {}", base64(format!("{}:{}", pct(client_id, false), pct(secret, false)).as_bytes())),
        )?;
    }
    let resp = crate::send(transport, req).await?;
    answer(resp)
}

fn scope_field(scopes: &[&str]) -> Option<String> {
    (!scopes.is_empty()).then(|| format!("scope={}", pct(&scopes.join(" "), false)))
}

/// Returns `OAuth2` password grant (RFC 6749 section 4.3). OAuth 2.1 deprecates it.
/// Errors: a transport failure or a non-2xx or undecodable token response.
pub async fn password_grant<T: Transport>(
    transport: &T,
    token_url: &str,
    username: &str,
    password: &str,
    client_id: &str,
    client_secret: Option<&str>,
    scopes: &[&str],
) -> Result<Token, Error> {
    let mut fields = vec![
        "grant_type=password".to_owned(),
        format!("username={}", pct(username, false)),
        format!("password={}", pct(password, false)),
    ];
    fields.extend(scope_field(scopes));
    token_call(transport, token_url, fields, client_id, client_secret).await
}

/// Returns a fresh access token from a refresh token (RFC 6749 section 6).
/// Errors: a transport failure or a non-2xx or undecodable token response.
pub async fn refresh_token<T: Transport>(
    transport: &T,
    token_url: &str,
    refresh: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<Token, Error> {
    let fields = vec!["grant_type=refresh_token".to_owned(), format!("refresh_token={}", pct(refresh, false))];
    token_call(transport, token_url, fields, client_id, client_secret).await
}

/// A PKCE pair (RFC 7636). Send `challenge` in the authorization URL. Keep
/// `verifier` for the code exchange.
#[cfg(feature = "pkce")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pkce {
    /// The secret kept by the client.
    pub verifier: String,
    /// `BASE64URL(SHA256(verifier))`. It is sent with `code_challenge_method=S256`.
    pub challenge: String,
}

#[cfg(feature = "pkce")]
impl Pkce {
    /// Creates the pair for a verifier from a CSPRNG (43 to 128 unreserved
    /// characters). Outside wasm32 `Pkce::new` makes one.
    pub fn from_verifier(verifier: &str) -> Self {
        Self { verifier: verifier.to_owned(), challenge: base64url(&sha256(verifier.as_bytes())) }
    }
}

/// Returns the URL to send the user to for an authorization code (RFC 6749 section 4.1.1 with PKCE).
#[cfg(feature = "pkce")]
pub fn authorization_url(
    base: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[&str],
    state: &str,
    pkce: &Pkce,
) -> String {
    let mut q = vec![
        "response_type=code".to_owned(),
        format!("client_id={}", pct(client_id, false)),
        format!("redirect_uri={}", pct(redirect_uri, false)),
        format!("state={}", pct(state, false)),
        format!("code_challenge={}", pct(&pkce.challenge, false)),
        "code_challenge_method=S256".to_owned(),
    ];
    q.extend(scope_field(scopes));
    format!("{base}{}{}", if base.contains('?') { '&' } else { '?' }, q.join("&"))
}

/// Returns URL for the implicit grant (RFC 6749 section 4.2): the token comes back in the
/// redirect's fragment. Deprecated by OAuth 2.1; prefer the code flow.
pub fn implicit_url(base: &str, client_id: &str, redirect_uri: &str, scopes: &[&str], state: &str) -> String {
    let mut q = vec![
        "response_type=token".to_owned(),
        format!("client_id={}", pct(client_id, false)),
        format!("redirect_uri={}", pct(redirect_uri, false)),
        format!("state={}", pct(state, false)),
    ];
    q.extend(scope_field(scopes));
    format!("{base}{}{}", if base.contains('?') { '&' } else { '?' }, q.join("&"))
}

/// Exchanges an authorization code for a token (RFC 6749 section 4.1.3 with the PKCE verifier).
/// Errors: a transport failure or a non-2xx or undecodable token response.
#[cfg(feature = "pkce")]
pub async fn exchange_code<T: Transport>(
    transport: &T,
    token_url: &str,
    code: &str,
    redirect_uri: &str,
    client_id: &str,
    client_secret: Option<&str>,
    pkce: &Pkce,
) -> Result<Token, Error> {
    let fields = vec![
        "grant_type=authorization_code".to_owned(),
        format!("code={}", pct(code, false)),
        format!("redirect_uri={}", pct(redirect_uri, false)),
        format!("code_verifier={}", pct(&pkce.verifier, false)),
    ];
    token_call(transport, token_url, fields, client_id, client_secret).await
}

/// Authorization server metadata: `OpenID` Connect discovery or RFC 8414.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct AuthMetadata {
    /// `issuer`.
    #[serde(default)]
    pub issuer: String,
    /// `authorization_endpoint`.
    #[serde(default)]
    pub authorization_endpoint: Option<String>,
    /// `token_endpoint`.
    #[serde(default)]
    pub token_endpoint: Option<String>,
    /// `device_authorization_endpoint` (RFC 8628).
    #[serde(default)]
    pub device_authorization_endpoint: Option<String>,
    /// `jwks_uri`.
    #[serde(default)]
    pub jwks_uri: Option<String>,
    /// `scopes_supported`.
    #[serde(default)]
    pub scopes_supported: Vec<String>,
}

/// Fetches authorization server metadata from `url` (an `openIdConnectUrl` or `oauth2MetadataUrl`).
/// Errors: a transport failure or a non-2xx or undecodable response.
pub async fn discover<T: Transport>(transport: &T, url: &str) -> Result<AuthMetadata, Error> {
    let mut req = crate::new_request("GET", url.to_owned(), vec![])?;
    req.set_header("Accept", "application/json")?;
    let resp = crate::send(transport, req).await?;
    answer(resp)
}

/// Returns SHA-256 for the PKCE challenge.
#[cfg(feature = "pkce")]
fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(data).into()
}

/// Returns `OAuth2` client credentials grant (RFC 6749 section 4.4) with HTTP basic auth.
/// Errors: a transport failure or a non-2xx or undecodable token response.
pub async fn client_credentials<T: Transport>(
    transport: &T,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    scopes: &[&str],
) -> Result<Token, Error> {
    let mut form = vec!["grant_type=client_credentials".to_owned()];
    if !scopes.is_empty() {
        form.push(format!("scope={}", pct(&scopes.join(" "), false)));
    }
    let mut req = token_request(token_url, &form)?;
    req.set_header(
        "Authorization",
        format!("Basic {}", base64(format!("{}:{}", pct(client_id, false), pct(client_secret, false)).as_bytes())),
    )?;
    let resp = crate::send(transport, req).await?;
    answer(resp)
}

/// A device authorization response (RFC 8628 section 3.2). Show `user_code` and
/// `verification_uri` to the user. Then poll with [`device_token`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct DeviceCode {
    /// Code the client polls with.
    pub device_code: String,
    /// Code the user types at `verification_uri`.
    pub user_code: String,
    /// Where the user goes to approve.
    pub verification_uri: String,
    /// `verification_uri` with the user code filled in if the server offers it.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    /// Seconds until the codes expire.
    pub expires_in: u64,
    /// Seconds to wait between polls (default 5).
    #[serde(default = "default_interval")]
    pub interval: u64,
}

/// Seconds between device-flow polls when the server names none (RFC 8628 section 3.2).
const DEFAULT_INTERVAL: u64 = 5;

const fn default_interval() -> u64 {
    DEFAULT_INTERVAL
}

/// One poll of the device token endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DevicePoll {
    /// The user approved: here is the token.
    Ready(Token),
    /// Not approved yet: poll again after `interval` seconds.
    Pending,
    /// Polling too fast. Add 5 seconds to `interval` and then poll again.
    SlowDown,
}

/// Starts the `OAuth2` device authorization grant (RFC 8628 section 3.1).
/// Errors: a transport failure or a non-2xx or undecodable response.
pub async fn device_authorization<T: Transport>(
    transport: &T,
    device_url: &str,
    client_id: &str,
    scopes: &[&str],
) -> Result<DeviceCode, Error> {
    let mut form = vec![format!("client_id={}", pct(client_id, false))];
    if !scopes.is_empty() {
        form.push(format!("scope={}", pct(&scopes.join(" "), false)));
    }
    let resp = crate::send(transport, token_request(device_url, &form)?).await?;
    answer(resp)
}

/// An `OAuth2` error response (RFC 6749 section 5.2).
#[derive(serde::Deserialize)]
struct OAuthError {
    error: String,
}

/// Polls the token endpoint once for a device code (RFC 8628 section 3.4). The caller waits between polls.
/// Errors: a transport failure, a denied or expired code or an undecodable response.
pub async fn device_token<T: Transport>(
    transport: &T,
    token_url: &str,
    client_id: &str,
    device_code: &str,
) -> Result<DevicePoll, Error> {
    let form = [
        format!("grant_type={}", pct("urn:ietf:params:oauth:grant-type:device_code", false)),
        format!("device_code={}", pct(device_code, false)),
        format!("client_id={}", pct(client_id, false)),
    ];
    let resp = crate::send(transport, token_request(token_url, &form)?).await?;
    let status = resp.status().as_u16();
    let body = resp.into_body();
    if http::StatusCode::from_u16(status).is_ok_and(|s| s.is_success()) {
        return json_decode(status, body.bytes()).map(DevicePoll::Ready);
    }
    match serde_json::from_slice::<OAuthError>(body.bytes()).map(|e| e.error).as_deref() {
        Ok("authorization_pending") => Ok(DevicePoll::Pending),
        Ok("slow_down") => Ok(DevicePoll::SlowDown),
        _ => Err(Error::UnexpectedStatus {
            status: http::StatusCode::from_u16(status).unwrap_or_default(),
            body: body.into_parts().0,
        }),
    }
}

/// Returns a form POST to a token endpoint that accepts JSON back.
fn token_request(url: &str, form: &[String]) -> Result<Request, Error> {
    let mut req = crate::new_request("POST", url.to_owned(), vec![])?;
    req.set_header("Accept", "application/json")?;
    req.set_body("application/x-www-form-urlencoded", form.join("&"));
    Ok(req)
}

/// Returns the JSON of a `2xx` answer of an authorization server.
fn answer<R: serde::de::DeserializeOwned>(resp: Response) -> Result<R, Error> {
    let status = resp.status();
    if !status.is_success() {
        return Err(Error::UnexpectedStatus { status, body: resp.into_body().into_parts().0 });
    }
    json_decode(status.as_u16(), resp.body().bytes())
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[cfg(feature = "pkce")]
    #[test]
    fn the_challenge_is_the_one_rfc_7636_gives_for_its_verifier() {
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(pkce.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[cfg(feature = "pkce")]
    #[test]
    fn the_authorization_url_carries_every_rfc_6749_and_pkce_field() {
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(
            authorization_url(
                "https://id.example/auth",
                "app 1",
                "https://app.example/cb",
                &["read", "write"],
                "s/1",
                &pkce
            ),
            "https://id.example/auth?response_type=code&client_id=app%201&redirect_uri=https%3A%2F%2Fapp.example%2Fcb\
             &state=s%2F1&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256\
             &scope=read%20write"
        );
        assert_eq!(
            authorization_url("https://id.example/auth?tenant=t", "a", "r", &[], "s", &pkce),
            "https://id.example/auth?tenant=t&response_type=code&client_id=a&redirect_uri=r&state=s\
             &code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256",
            "a base with a query gets `&`, and no scopes means no `scope`"
        );
    }

    /// A metadata endpoint that answers `status` with `body`. It first checks the request that `discover` sends.
    fn metadata(status: u16, body: &'static str) -> impl Transport {
        tower::service_fn(move |req: Request| {
            assert_eq!(
                (req.method().as_str(), req.uri().to_string().as_str()),
                ("GET", "https://id.example/.well-known/openid-configuration")
            );
            assert_eq!(req.header("accept").as_deref(), Some("application/json"));
            let resp = crate::body_response(status, "application/json", body);
            std::future::ready(Ok::<_, std::convert::Infallible>(resp))
        })
    }

    #[test]
    fn discover_reads_the_metadata_document() {
        let url = "https://id.example/.well-known/openid-configuration";
        let body = r#"{"issuer":"https://id.example","token_endpoint":"https://id.example/token","scopes_supported":["openid"],"extra":1}"#;
        let got = crate::block_on(discover(&metadata(200, body), url)).unwrap();
        assert_eq!(
            got,
            AuthMetadata {
                issuer: "https://id.example".to_owned(),
                token_endpoint: Some("https://id.example/token".to_owned()),
                scopes_supported: vec!["openid".to_owned()],
                ..AuthMetadata::default()
            }
        );
        let missing = crate::block_on(discover(&metadata(404, "{}"), url));
        assert_matches!(
            missing,
            Err(Error::UnexpectedStatus { status: http::StatusCode::NOT_FOUND, .. }),
            "{missing:?}"
        );
    }

    #[test]
    fn the_implicit_url_asks_for_a_token() {
        assert_eq!(
            implicit_url("https://id.example/auth", "a", "https://app.example/cb", &["read"], "s"),
            "https://id.example/auth?response_type=token&client_id=a&redirect_uri=https%3A%2F%2Fapp.example%2Fcb&state=s\
             &scope=read"
        );
        assert_eq!(
            implicit_url("https://id.example/auth?x=1", "a", "r", &[], "s"),
            "https://id.example/auth?x=1&response_type=token&client_id=a&redirect_uri=r&state=s"
        );
    }
}
