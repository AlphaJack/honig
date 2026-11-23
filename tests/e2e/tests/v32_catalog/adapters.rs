//! Adapters: the server under hyper without axum, a blocking ureq client, an in-memory client for tests and a
//! transport of your own.

use assert_matches::assert_matches;
use std::sync::Arc;

use e2e::v32_catalog::api::{self, operations as ops};

use super::{Catalog, lamp};

// ANCHOR: hyper
async fn serve_with_hyper(listener: tokio::net::TcpListener) -> std::io::Result<()> {
    let service = api::Server::new(Catalog::default()).into_service();
    loop {
        let (stream, _) = listener.accept().await?;
        let service = hyper_util::service::TowerToHyperService::new(service.clone());
        let io = hyper_util::rt::TokioIo::new(stream);
        tokio::spawn(hyper::server::conn::http1::Builder::new().serve_connection(io, service));
    }
}
// ANCHOR_END: hyper

// ANCHOR: ureq
/// The blocking client. `block_on` returns once the response is received with no async runtime.
fn list_products(base_url: &str) -> Result<ops::list_products::Response, api::ClientError> {
    let client = api::Client::new(api::UreqTransport::new(base_url)).with_api_key("secret");
    api::block_on(client.list_products(ops::list_products::Params::new()))
}
// ANCHOR_END: ureq

#[tokio::test]
async fn hyper_serves_the_catalog_and_ureq_calls_it() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(serve_with_hyper(listener));
    let listed = tokio::task::spawn_blocking(move || list_products(&base_url)).await.unwrap().unwrap();
    assert_eq!(listed, ops::list_products::Response::Ok { body: vec![], x_next: None });
}

// ANCHOR: retry
/// Retries only methods that RFC 9110 calls idempotent. `RetryTransientMiddleware` alone retries any method. A `POST`
/// sent twice may create twice.
struct IdempotentOnly<M>(M);

#[async_trait::async_trait]
impl<M: reqwest_middleware::Middleware> reqwest_middleware::Middleware for IdempotentOnly<M> {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: reqwest_middleware::Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        use reqwest::Method;
        let idempotent = [Method::GET, Method::HEAD, Method::OPTIONS, Method::PUT, Method::DELETE];
        if idempotent.contains(req.method()) {
            self.0.handle(req, extensions, next).await
        } else {
            next.run(req, extensions).await
        }
    }
}

/// A client that keeps an in-memory HTTP cache and revalidates with `If-None-Match`. It retries transient failures
/// (a lost connection, 5xx, 429) of idempotent requests with exponential backoff. It is a `reqwest-middleware` stack
/// sent through `ReqwestMiddlewareTransport`.
fn retrying(base_url: &str) -> api::Client<api::ReqwestMiddlewareTransport> {
    use http_cache_reqwest::{Cache, CacheMode, HttpCache, HttpCacheOptions, MokaManager};
    let policy = reqwest_retry::policies::ExponentialBackoff::builder().build_with_max_retries(3);
    let cache =
        HttpCache { mode: CacheMode::Default, manager: MokaManager::default(), options: HttpCacheOptions::default() };
    let stack = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
        .with(Cache(cache))
        .with(IdempotentOnly(reqwest_retry::RetryTransientMiddleware::new_with_policy(policy)))
        .build();
    api::Client::new(api::ReqwestMiddlewareTransport::with_client(stack, base_url)).with_api_key("secret")
}
// ANCHOR_END: retry

#[tokio::test]
async fn the_client_retries_until_the_server_answers() {
    let failures = Arc::new(std::sync::atomic::AtomicU32::new(1));
    let router = api::Server::new(Catalog::default())
        .operation_layer(tower::filter::AsyncFilterLayer::new(move |req: api::Request| {
            let fail = failures.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) > 0;
            std::future::ready(if fail {
                Err(api::ApiError::new(api::http::StatusCode::SERVICE_UNAVAILABLE, "starting up"))
            } else {
                Ok(req)
            })
        }))
        .into_axum();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    let listed = retrying(&base_url).list_products(ops::list_products::Params::new()).await.unwrap();
    assert_eq!(listed, ops::list_products::Response::Ok { body: vec![], x_next: None });
}

#[tokio::test]
async fn the_client_sends_a_failed_post_once() {
    let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let seen = Arc::clone(&calls);
    let router = api::Server::new(Catalog::default())
        .operation_layer(tower::filter::AsyncFilterLayer::new(move |_: api::Request| {
            seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::future::ready(Err::<api::Request, _>(api::ApiError::new(
                api::http::StatusCode::SERVICE_UNAVAILABLE,
                "starting up",
            )))
        }))
        .into_axum();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    let created = retrying(&base_url).create_product(ops::create_product::Params::new(lamp())).await.unwrap();
    assert_matches!(
        created,
        ops::create_product::Response::Default { status: ops::create_product::DefaultStatus::ServiceUnavailable, .. }
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1, "a POST is not idempotent: no retry");
}

// ANCHOR: loopback
/// A client wired straight to the server with no socket. A test of your server needs this.
fn in_memory_client() -> api::Client<impl api::Transport> {
    let service = api::Server::new(Catalog::default()).into_service();
    api::Client::new(service).with_api_key("secret")
}
// ANCHOR_END: loopback

#[tokio::test]
async fn the_in_memory_client_reaches_the_server() {
    let client = in_memory_client();
    client.create_product(ops::create_product::Params::new(lamp())).await.unwrap();
    let found = client.get_product(ops::get_product::Params::new(0)).await.unwrap();
    assert_eq!(found, ops::get_product::Response::Ok(lamp()));
}

// ANCHOR: transport
/// A transport is any tower `Service`, so tower's layers wrap one: this one adds a request ID to every request.
fn with_request_id(transport: impl api::Transport) -> impl api::Transport {
    tower::ServiceBuilder::new()
        .map_request(|mut req: api::Request| {
            req.headers_mut().insert("x-request-id", api::http::HeaderValue::from_static("req-1"));
            req
        })
        .service(transport)
}
// ANCHOR_END: transport

#[tokio::test]
async fn a_transport_of_your_own_wraps_another() {
    let service = api::Server::new(Catalog::default()).into_service();
    let seen = crate::support::loopback(move |req: api::Request| {
        assert!(req.headers().get("x-request-id").is_some_and(|v| v == "req-1"));
        service.handle(req)
    });
    let client = api::Client::new(with_request_id(seen)).with_api_key("secret");
    let found = client.get_product(ops::get_product::Params::new(0)).await.unwrap();
    assert_matches!(found, ops::get_product::Response::NotFound(_));
}
