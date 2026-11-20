//! A `mutualTLS` requirement is never satisfied by default. Nothing in the
//! request proves a client certificate. So a layer must check it.

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

struct Server;

impl api::Api for Server {
    type Principal = ();

    fn keyed(
        &self,
        _: ops::keyed::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::keyed::Response, api::ApiError>> + Send {
        ok(ops::keyed::Response::NoContent)
    }
    fn private(
        &self,
        _: ops::private::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::private::Response, api::ApiError>> + Send {
        ok(ops::private::Response::NoContent)
    }
    fn token(
        &self,
        _: ops::token::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::token::Response, api::ApiError>> + Send {
        ok(ops::token::Response::NoContent)
    }
}

/// Trusts a certificate its TLS terminator vouched for (`x-client-verify: SUCCESS`).
#[derive(Clone)]
struct VerifyCertificateLayer;

impl<S> tower::Layer<S> for VerifyCertificateLayer {
    type Service = VerifyCertificate<S>;

    fn layer(&self, inner: S) -> VerifyCertificate<S> {
        VerifyCertificate(inner)
    }
}

#[derive(Clone)]
struct VerifyCertificate<S>(S);

impl<S: tower::Service<api::Request>> tower::Service<api::Request> for VerifyCertificate<S> {
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), S::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, mut req: api::Request) -> S::Future {
        if api::Secured::relies_on_mutual_tls(&req)
            && req.headers().get("x-client-verify").and_then(|v| v.to_str().ok()) == Some("SUCCESS")
        {
            req.extensions_mut().insert(api::ClientCertificateVerified);
        }
        self.0.call(req)
    }
}

fn get(verify: bool, path: &str, headers: &[(&str, &str)]) -> u16 {
    let mut req = api::runtime::new_request("GET", path.to_owned(), vec![]).unwrap();
    for (k, v) in headers {
        req.headers_mut().append(
            honig::http::HeaderName::try_from(*k).unwrap(),
            honig::http::HeaderValue::try_from((*v).to_owned()).unwrap(),
        );
    }
    let server = api::Server::new(Server).authenticate(|_: &api::Request| std::future::ready(Ok(Some(()))));
    let resp = if verify {
        api::block_on(server.operation_layer(VerifyCertificateLayer).into_service().handle(req))
    } else {
        api::block_on(server.into_service().handle(req))
    };
    resp.status().as_u16()
}

#[test]
fn unverified_mutual_tls_is_refused() {
    assert_eq!(get(false, "/private", &[]), 401);
}

#[test]
fn other_schemes_need_no_certificate() {
    assert_eq!(get(false, "/token", &[("authorization", "Bearer t")]), 204);
}

#[test]
fn a_layer_that_checks_the_certificate_decides() {
    assert_eq!(get(true, "/private", &[]), 401);
    assert_eq!(get(true, "/private", &[("x-client-verify", "SUCCESS")]), 204);
}

/// RFC 7636, Appendix B: the PKCE challenge is the own SHA-256 of the runtime.
#[test]
fn pkce_challenge_matches_the_rfc() {
    let p = api::Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
    assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

/// A verifier longer than one 64-byte block exercises the block loop.
#[test]
fn pkce_challenge_of_a_long_verifier() {
    let p = api::Pkce::from_verifier(&"x".repeat(200));
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
        assert_eq!(api::Pkce::from_verifier(&"x".repeat(n)).challenge, want, "{n} bytes");
    }
}

/// Keeps the last request a client sent.
type Seen = Arc<Mutex<Option<api::Request>>>;

fn recorder(seen: &Seen) -> impl api::Transport {
    let keep = seen.clone();
    support::loopback(move |req: api::Request| {
        *keep.lock().unwrap() = Some(req);
        std::future::ready(api::runtime::new_response(204))
    })
}

#[test]
fn typed_setters_put_each_credential_where_its_scheme_says() {
    let seen = Seen::default();
    let client = Client::new(recorder(&seen)).with_api_key("k1").with_basic_auth("ann", "pw").with_bearer("t0k");
    api::block_on(client.keyed(ops::keyed::Params {})).unwrap();
    let req = seen.lock().unwrap().take().expect("sent");
    assert!(req.uri().to_string().ends_with("?key=k1"), "{}", req.uri().to_string());
    // `ann:pw` in base64.
    assert_eq!(req.headers().get("authorization").and_then(|v| v.to_str().ok()), Some("Basic YW5uOnB3"));
    api::block_on(client.token(ops::token::Params {})).unwrap();
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
        malformed::check(api::Server::new(Server).authenticate(|_: &api::Request| std::future::ready(Ok(Some(())))).into_service(), &case)?;
    }
}
