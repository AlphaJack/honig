//! Layers: the limits of tower inside the server, a rate limit as a filter, your own error bodies and tower-http
//! outside.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::response::IntoResponse;
use e2e::v32_catalog::api;
use tower::ServiceExt;

use super::Catalog;

// ANCHOR: inside
/// Limits that answer in the description's error schema: a `503` once a request takes too long or too many run.
fn limited() -> axum::Router {
    api::Server::new(Catalog::default())
        .authenticate(super::check_key)
        .operation_layer(tower::timeout::TimeoutLayer::new(Duration::from_secs(10)))
        .operation_layer(tower::load_shed::LoadShedLayer::new())
        .operation_layer(tower::limit::ConcurrencyLimitLayer::new(64))
        .into_axum()
}
// ANCHOR_END: inside

// ANCHOR: rate-limit
/// At most `limit` requests. A real limiter keys its count by principal and time window. It keeps it in the same way in
/// an `Arc` because the layer is cloned for every request.
fn rate_limited(limit: u32) -> axum::Router {
    let used = Arc::new(AtomicU32::new(0));
    api::Server::new(Catalog::default())
        .authenticate(super::check_key)
        .operation_layer(tower::filter::AsyncFilterLayer::new(move |req: api::Request| {
            let over = used.fetch_add(1, Ordering::Relaxed) >= limit;
            let refusal = api::ApiError::new(api::http::StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded")
                .with_header(api::http::header::RETRY_AFTER, api::http::HeaderValue::from_static("60"));
            std::future::ready(if over { Err(refusal) } else { Ok(req) })
        }))
        .into_axum()
}
// ANCHOR_END: rate-limit

// ANCHOR: per-operation
/// Writes cost more than reads: only `createProduct` is limited. The match lists every operation, so one added to the
/// description fails to compile here until you decide its limit.
fn writes_limited(limit: u32) -> axum::Router {
    let used = Arc::new(AtomicU32::new(0));
    api::Server::new(Catalog::default())
        .authenticate(super::check_key)
        .operation_layer(tower::filter::AsyncFilterLayer::new(move |req: api::Request| {
            let limited = match req.extensions().get::<api::Operation>() {
                Some(api::Operation::CreateProduct) => true,
                Some(api::Operation::ListProducts | api::Operation::GetProduct) | None => false,
            };
            let over = limited && used.fetch_add(1, Ordering::Relaxed) >= limit;
            std::future::ready(if over {
                Err(api::ApiError::new(api::http::StatusCode::TOO_MANY_REQUESTS, "too many writes"))
            } else {
                Ok(req)
            })
        }))
        .into_axum()
}
// ANCHOR_END: per-operation

// ANCHOR: error-response
/// Every error that the server raises itself as plain text. The own typed errors of a handler go out as they are.
fn plain_text_errors() -> axum::Router {
    api::Server::new(Catalog::default())
        .authenticate(super::check_key)
        .error_response(|rejection: api::Rejection| {
            let reason = &rejection.reason;
            if let Some(own) = reason.own() {
                return own;
            }
            let mut response = reason.head();
            response
                .headers_mut()
                .insert(api::http::header::CONTENT_TYPE, api::http::HeaderValue::from_static("text/plain"));
            *response.body_mut() = api::Body::from(reason.detail().unwrap_or_default());
            response
        })
        .into_axum()
}
// ANCHOR_END: error-response

// ANCHOR: outside
/// tower-http layers outside the router. A layer outside that answers on its own renders the error schema of the
/// description with `errors()`. Here it answers for a path that the description does not declare.
fn with_cors() -> axum::Router {
    let server = api::Server::new(Catalog::default()).authenticate(super::check_key);
    let errors = server.errors();
    server
        .into_axum()
        .fallback(move || {
            std::future::ready(errors.response(api::http::StatusCode::NOT_FOUND, "no such page").into_response())
        })
        .layer(tower_http::cors::CorsLayer::permissive())
}
// ANCHOR_END: outside

async fn get(router: axum::Router, path: &str) -> axum::http::Response<axum::body::Body> {
    let req = axum::http::Request::get(path)
        .header("X-API-Key", "secret")
        .header("Origin", "https://shop.example.com")
        .body(axum::body::Body::empty())
        .unwrap();
    router.oneshot(req).await.unwrap()
}

#[tokio::test]
async fn the_limits_let_a_request_through() {
    assert_eq!(get(limited(), "/v1/products").await.status(), 200);
}

#[tokio::test]
async fn the_guard_refuses_past_the_limit() {
    let router = rate_limited(1);
    assert_eq!(get(router.clone(), "/v1/products").await.status(), 200);
    let refused = get(router, "/v1/products").await;
    assert_eq!(refused.status(), 429);
    assert_eq!(refused.headers()["retry-after"], "60");
}

#[tokio::test]
async fn only_the_operation_that_the_guard_names_is_limited() {
    let router = writes_limited(0);
    assert_eq!(get(router.clone(), "/v1/products").await.status(), 200, "a read");
    let post = axum::http::Request::post("/v1/products")
        .header("X-API-Key", "secret")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(r#"{"kind":"digital","name":"Manual"}"#))
        .unwrap();
    assert_eq!(router.oneshot(post).await.unwrap().status(), 429, "a write");
}

/// A refusal carries its operation as the generated enum, and a request that no operation takes carries none.
#[tokio::test]
async fn a_rejection_names_its_operation() {
    let seen = Arc::new(std::sync::Mutex::new(vec![]));
    let record = Arc::clone(&seen);
    let router = api::Server::new(Catalog::default())
        .authenticate(super::check_key)
        .error_response(move |r: api::Rejection| {
            record.lock().unwrap().push(r.operation);
            r.response()
        })
        .into_service();
    for path in ["/v1/products/not-a-number", "/v1/nowhere"] {
        let req = api::runtime::new_request("GET", path.to_owned(), vec![]).unwrap();
        router.handle(req).await;
    }
    assert_eq!(*seen.lock().unwrap(), [Some(api::Operation::GetProduct), None]);
}

#[tokio::test]
async fn errors_the_server_raises_go_out_as_text() {
    let resp = get(plain_text_errors(), "/v1/products/not-a-number").await;
    assert_eq!(resp.status(), 400);
    assert_eq!(resp.headers()["content-type"], "text/plain");
}

#[tokio::test]
async fn layers_outside_answer_in_the_error_schema() {
    let resp = get(with_cors(), "/v1/elsewhere").await;
    assert_eq!(resp.status(), 404);
    assert!(resp.headers().contains_key("access-control-allow-origin"));
}

// ANCHOR: signed-body
/// A signature over the body, as webhooks (`X-Hub-Signature-256`) and Matrix (`X-Matrix`) send. Here the signature is
/// the body's length, a stand-in for an HMAC. The filter owns the request, so it reads the body itself. The operation
/// then decodes the same bytes and reads nothing again.
async fn verify(mut req: api::Request) -> Result<api::Request, api::ApiError> {
    let len = api::read_body(&mut req).await?.len();
    let ok = req.headers().get("x-signature").and_then(|v| v.to_str().ok()) == Some(len.to_string().as_str());
    if ok { Ok(req) } else { Err(api::ApiError::new(api::http::StatusCode::UNAUTHORIZED, "bad signature")) }
}
// ANCHOR_END: signed-body

#[test]
fn a_guard_checks_a_signature_over_the_body_that_the_operation_then_decodes() {
    use api::operations::create_product;
    let client = |sign: fn(usize) -> String| {
        // ANCHOR: read-body
        let service = api::Server::new(Catalog::default())
            .authenticate(super::check_key)
            .operation_layer(tower::filter::AsyncFilterLayer::new(verify))
            .into_service();
        // ANCHOR_END: read-body
        // The client signs in its transport: the request is final there, body included.
        api::Client::new(crate::support::loopback(move |mut req: api::Request| {
            let signature = sign(req.body().bytes().len());
            req.headers_mut().append("x-signature", honig::http::HeaderValue::try_from(signature).unwrap());
            service.handle(req)
        }))
        .with_api_key("secret")
    };
    let created = api::block_on(client(|n| n.to_string()).create_product(create_product::Params::new(super::lamp())));
    assert!(matches!(created, Ok(create_product::Response::Created(_))), "{created:?}");
    let refused = api::block_on(client(|_| "0".to_owned()).create_product(create_product::Params::new(super::lamp())));
    assert!(
        matches!(refused, Ok(create_product::Response::Default { status: api::http::StatusCode::UNAUTHORIZED, .. })),
        "{refused:?}"
    );
}

// ANCHOR: any-api
/// A tower layer for any honig server: it counts the requests of each operation by its name. It is a plain
/// `tower::Layer` over `http` requests that reads the `MatchedOperation` that the server puts in the extensions.
#[derive(Clone, Default)]
struct CountByOperationLayer(Arc<std::sync::Mutex<std::collections::BTreeMap<&'static str, u32>>>);

impl<S> tower::Layer<S> for CountByOperationLayer {
    type Service = CountByOperation<S>;

    fn layer(&self, inner: S) -> CountByOperation<S> {
        CountByOperation { inner, counts: Arc::clone(&self.0) }
    }
}

#[derive(Clone)]
struct CountByOperation<S> {
    inner: S,
    counts: Arc<std::sync::Mutex<std::collections::BTreeMap<&'static str, u32>>>,
}

impl<S: tower::Service<honig::Request>> tower::Service<honig::Request> for CountByOperation<S> {
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), S::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: honig::Request) -> S::Future {
        if let Some(routed) = req.extensions().get::<honig::MatchedOperation>() {
            *self.counts.lock().unwrap().entry(routed.name()).or_default() += 1;
        }
        self.inner.call(req)
    }
}
// ANCHOR_END: any-api

/// The scopes fixture: a second API for the same layer.
struct Issues;

impl e2e::v32_scopes::api::Api for Issues {
    type Principal = ();

    async fn list_issues(
        &self,
        _: e2e::v32_scopes::api::operations::list_issues::Params,
        _: &e2e::v32_scopes::api::Context<'_>,
    ) -> Result<e2e::v32_scopes::api::operations::list_issues::Response, api::ApiError> {
        Ok(e2e::v32_scopes::api::operations::list_issues::Response::NoContent)
    }
    async fn create_issue(
        &self,
        _: e2e::v32_scopes::api::operations::create_issue::Params,
        _: &e2e::v32_scopes::api::Context<'_>,
    ) -> Result<e2e::v32_scopes::api::operations::create_issue::Response, api::ApiError> {
        Ok(e2e::v32_scopes::api::operations::create_issue::Response::NoContent)
    }
    async fn open(
        &self,
        _: e2e::v32_scopes::api::operations::open::Params,
        _: &e2e::v32_scopes::api::Context<'_>,
    ) -> Result<e2e::v32_scopes::api::operations::open::Response, api::ApiError> {
        Ok(e2e::v32_scopes::api::operations::open::Response::NoContent)
    }
}

/// One tower layer on two APIs: each request is counted under its own API's operation.
#[tokio::test]
async fn one_tower_layer_serves_any_api() {
    let counts = CountByOperationLayer::default();
    let catalog =
        api::Server::new(Catalog::default()).authenticate(super::check_key).operation_layer(counts.clone()).into_axum();
    let issues = e2e::v32_scopes::api::Server::new(Issues)
        .authenticate(|req: &e2e::v32_scopes::api::Request| {
            let read = req.headers().get("authorization").is_some_and(|token| token == "Bearer read");
            std::future::ready(Ok(read.then_some(())))
        })
        .operation_layer(counts.clone())
        .into_service();
    assert_eq!(get(catalog, "/v1/products").await.status(), 200);
    let mut req = e2e::v32_scopes::api::runtime::new_request("GET", "/issues".to_owned(), vec![]).unwrap();
    req.headers_mut().append("authorization", honig::http::HeaderValue::from_static("Bearer read"));
    assert_eq!(issues.handle(req).await.status().as_u16(), 204);
    let seen: Vec<(&str, u32)> = counts.0.lock().unwrap().iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(seen, [("list_issues", 1), ("list_products", 1)]);
}
