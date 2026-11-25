//! A `mutualTLS` requirement is satisfied only when `verify_cert` says so. Nothing in the request proves a client
//! certificate. So the `Api` checks what the TLS terminator vouched for.

mod support;

use std::future::Future;
use std::sync::{Arc, Mutex};

use e2e::v32_mtls::api::{self, Client, operations as ops};

fn ok<T>(r: T) -> impl Future<Output = Result<T, api::ApiError>> + Send
where
    T: Send,
{
    std::future::ready(Ok(r))
}

/// Trusts a certificate that its TLS terminator vouched for (`x-client-verify: SUCCESS`) and every other credential.
struct Server;

impl api::Api for Server {
    type Cert = ();
    type Bearer = ();
    type ApiKey = ();
    type BasicAuth = ();
    async fn verify_cert(&self, request: &api::Request) -> Result<Option<()>, api::ApiError> {
        let vouched = request.headers().get("x-client-verify").and_then(|v| v.to_str().ok()) == Some("SUCCESS");
        Ok(vouched.then_some(()))
    }
    async fn verify_bearer(&self, _: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }
    async fn verify_api_key(&self, _: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }
    async fn verify_basic_auth(&self, _: api::Basic<'_>, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }

    fn keyed(
        &self,
        _: ops::keyed::Params,
        _: &api::Context<'_, ops::keyed::Security<Self>>,
    ) -> impl Future<Output = Result<ops::keyed::Response, api::ApiError>> + Send {
        ok(ops::keyed::Response::NoContent)
    }
    fn private(
        &self,
        _: ops::private::Params,
        _: &api::Context<'_, ops::private::Security<Self>>,
    ) -> impl Future<Output = Result<ops::private::Response, api::ApiError>> + Send {
        ok(ops::private::Response::NoContent)
    }
    fn token(
        &self,
        _: ops::token::Params,
        _: &api::Context<'_, ops::token::Security<Self>>,
    ) -> impl Future<Output = Result<ops::token::Response, api::ApiError>> + Send {
        ok(ops::token::Response::NoContent)
    }
}

fn get(path: &str, headers: &[(&str, &str)]) -> u16 {
    let mut req = honig::__private::new_request("GET", path.to_owned(), vec![]).unwrap();
    for (k, v) in headers {
        req.headers_mut().append(
            honig::http::HeaderName::try_from(*k).unwrap(),
            honig::http::HeaderValue::try_from((*v).to_owned()).unwrap(),
        );
    }
    honig::block_on(api::Server::new(Server).into_service().handle(req)).status().as_u16()
}

#[test]
fn unverified_mutual_tls_is_refused() {
    assert_eq!(get("/private", &[]), 401);
}

#[test]
fn other_schemes_need_no_certificate() {
    assert_eq!(get("/token", &[("authorization", "Bearer t")]), 204);
}

#[test]
fn the_verify_method_of_the_scheme_decides() {
    assert_eq!(get("/private", &[("x-client-verify", "FAILED")]), 401);
    assert_eq!(get("/private", &[("x-client-verify", "SUCCESS")]), 204);
}

/// Takes the key `k1` and the login `ann:pw`, and nothing else.
struct Strict;

impl api::Api for Strict {
    type Cert = &'static str;
    type Bearer = &'static str;
    type ApiKey = &'static str;
    type BasicAuth = &'static str;

    async fn verify_cert(&self, _: &api::Request) -> Result<Option<&'static str>, api::ApiError> {
        Ok(None)
    }
    async fn verify_bearer(&self, _: &str, _: &api::Request) -> Result<Option<&'static str>, api::ApiError> {
        Ok(None)
    }
    async fn verify_api_key(&self, key: &str, _: &api::Request) -> Result<Option<&'static str>, api::ApiError> {
        Ok((key == "k1").then_some("key holder"))
    }
    async fn verify_basic_auth(
        &self,
        login: api::Basic<'_>,
        _: &api::Request,
    ) -> Result<Option<&'static str>, api::ApiError> {
        Ok((login == api::Basic { user: "ann", password: "pw" }).then_some("ann"))
    }

    async fn keyed(
        &self,
        _: ops::keyed::Params,
        ctx: &api::Context<'_, ops::keyed::Security<Self>>,
    ) -> Result<ops::keyed::Response, api::ApiError> {
        let ops::keyed::Security::ApiKeyAndBasicAuth(key, login) = &ctx.security;
        assert_eq!((*key, *login), ("key holder", "ann"), "the value of each scheme of the alternative");
        Ok(ops::keyed::Response::NoContent)
    }
    async fn private(
        &self,
        _: ops::private::Params,
        _: &api::Context<'_, ops::private::Security<Self>>,
    ) -> Result<ops::private::Response, api::ApiError> {
        Ok(ops::private::Response::NoContent)
    }
    async fn token(
        &self,
        _: ops::token::Params,
        _: &api::Context<'_, ops::token::Security<Self>>,
    ) -> Result<ops::token::Response, api::ApiError> {
        Ok(ops::token::Response::NoContent)
    }
}

/// `[{api_key, basic-auth}]`: both credentials must be valid. A present but forged one fails the whole alternative.
#[test]
fn an_and_requirement_needs_every_credential_valid() {
    let keyed = |key: &str, login: &str| {
        let path = format!("/keyed?key={key}");
        let mut req = honig::__private::new_request("GET", path, vec![]).unwrap();
        let basic = format!("Basic {}", honig::__private::base64(login.as_bytes()));
        req.headers_mut().append("authorization", honig::http::HeaderValue::try_from(basic).unwrap());
        honig::block_on(api::Server::new(Strict).into_service().handle(req)).status().as_u16()
    };
    assert_eq!(keyed("k1", "ann:pw"), 204);
    assert_eq!(keyed("forged", "ann:pw"), 401, "a forged key with a valid login");
    assert_eq!(keyed("k1", "ann:guess"), 401, "a valid key with a wrong password");
}

/// RFC 7636, Appendix B: the PKCE challenge is the own SHA-256 of honig.
#[test]
fn pkce_challenge_matches_the_rfc() {
    let p = honig::Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

/// A verifier longer than one 64-byte block exercises the block loop.
#[test]
fn pkce_challenge_of_a_long_verifier() {
    let p = honig::Pkce::from_verifier(&"x".repeat(200));
    assert_eq!(p.challenge, "qiDCPjIBg0BQZ54diJQbmm_tBVfJpwXLLDFeLmP9SG0");
}

/// SHA-256 padding edges. 55 and 56 bytes straddle the length field. 63 to
/// 64 and 119 to 120 straddle the block boundary. Expected values come from `openssl dgst`.
#[test]
fn pkce_challenge_at_padding_edges() {
    let cases = [
        (0, "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU"),
        (55, "1eKFaDzU78AtAhpcYgFGlJWJAQBdb3HongmJ-sd-QHI"),
        (56, "BMJiYTcO51QVSdFt7jIMcj4_0UZx5moJmv4KN3wWiI4"),
        (63, "dSILRyGCeOZW8gE7uPDEVaJerwHobGSSTp1I2Jd21vI"),
        (64, "fOEAlx9k5wAej-WlGXPs3-HO1Cvv5-6NX9YhlQa1OTw"),
        (119, "AAtI1O3w-nvuPGI27NJ4W6pdtO64u1Q0GwKeDZ-l-ww"),
        (120, "E_BaC1lHh_Xs0xXtyWFBvTJDID0bfU8INvNzCLJ2upg"),
        (128, "JNobgdCxbfZCju5zxp_LKpPHa8bfcG8MZnD-a_6ABGQ"),
    ];
    for (n, want) in cases {
        assert_eq!(honig::Pkce::from_verifier(&"x".repeat(n)).challenge, want, "{n} bytes");
    }
}

/// Keeps the last request a client sent.
type Seen = Arc<Mutex<Option<api::Request>>>;

fn recorder(seen: &Seen) -> impl api::Transport {
    let keep = seen.clone();
    support::loopback(move |req: api::Request| {
        *keep.lock().unwrap() = Some(req);
        std::future::ready(honig::__private::new_response(204))
    })
}

#[test]
fn typed_setters_put_each_credential_where_its_scheme_says() {
    let seen = Seen::default();
    let client = Client::new(recorder(&seen)).with_api_key("k1").with_basic_auth("ann", "pw").with_bearer("t0k");
    honig::block_on(client.keyed(ops::keyed::Params {})).unwrap();
    let req = seen.lock().unwrap().take().expect("sent");
    assert!(req.uri().to_string().ends_with("?key=k1"), "{}", req.uri().to_string());
    // `ann:pw` in base64.
    assert_eq!(req.headers().get("authorization").and_then(|v| v.to_str().ok()), Some("Basic YW5uOnB3"));
    honig::block_on(client.token(ops::token::Params {})).unwrap();
    let req = seen.lock().unwrap().take().expect("sent");
    assert_eq!(req.headers().get("authorization").and_then(|v| v.to_str().ok()), Some("Bearer t0k"));
}

mod malformed;

static FIXTURE: malformed::Fixture = malformed::Fixture {
    routes: &[
        (ops::private::METHOD, ops::private::PATH),
        (ops::token::METHOD, ops::token::PATH),
        (ops::keyed::METHOD, ops::keyed::PATH),
    ],
    segments: &[],
    query_names: &["key"],
    header_names: &[],
};

proptest::proptest! {
    #![proptest_config(malformed::config())]

    #[test]
    fn malformed_requests_get_a_client_error(case in malformed::case(&FIXTURE)) {
        malformed::check(api::Server::new(Server).into_service(), &case)?;
    }
}
