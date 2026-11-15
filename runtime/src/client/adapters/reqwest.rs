//! `reqwest` transport. It is included when the crate depends on `reqwest`. With `reqwest-middleware` the same
//! transport sends through a middleware stack. Retries, tracing and caching come from the stack that you build.

use std::future::Future;

use crate::transport::Pending;
use crate::{Body, ByteStream, Error, MaybeSend, MaybeSync, Request, Response};

/// A client that sends a built `reqwest` request: `reqwest::Client` or a `reqwest-middleware` stack.
pub trait Execute: Clone + MaybeSend + MaybeSync + 'static {
    /// Sends `req` and returns once the response headers are in.
    fn execute(&self, req: reqwest::Request) -> impl Future<Output = Result<reqwest::Response, Error>> + MaybeSend;
}

impl Execute for reqwest::Client {
    fn execute(&self, req: reqwest::Request) -> impl Future<Output = Result<reqwest::Response, Error>> + MaybeSend {
        let sent = Self::execute(self, req);
        async move { sent.await.map_err(Error::transport) }
    }
}

/// `reqwest` transport: `Client::new(api::ReqwestTransport::new("https://api.example.com"))`.
#[derive(Debug, Clone)]
pub struct ReqwestTransport<C = reqwest::Client> {
    /// The underlying client. Configure timeouts, proxies, TLS or the middleware stack on it.
    pub client: C,
    /// Base URL without a trailing slash.
    pub base_url: String,
}

impl ReqwestTransport {
    /// Creates a transport with a default client for this base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_client(reqwest::Client::new(), base_url)
    }
}

impl<C> ReqwestTransport<C> {
    /// Creates a transport over `client` for this base URL.
    pub fn with_client(client: C, base_url: impl Into<String>) -> Self {
        Self { client, base_url: crate::base_url(base_url) }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
use native as imp;
#[cfg(target_arch = "wasm32")]
use wasm as imp;

impl<C: Execute> ReqwestTransport<C> {
    /// Sends `req` and returns the response once its head is in.
    fn start(&self, req: Request) -> impl Future<Output = Result<reqwest::Response, Error>> + MaybeSend + use<C> {
        let client = self.client.clone();
        let target = crate::target(&self.base_url, req.uri());
        async move {
            let (mut head, body) = req.into_parts();
            head.uri = http::Uri::try_from(target).map_err(Error::transport)?;
            let req = http::Request::from_parts(head, imp::body(body).await?);
            client.execute(reqwest::Request::try_from(req).map_err(Error::transport)?).await
        }
    }
}

/// Returns the status and headers of `resp` around `body`.
fn response(resp: &reqwest::Response, body: Body) -> Response {
    let mut out = Response::new(body);
    *out.status_mut() = resp.status();
    out.headers_mut().clone_from(resp.headers());
    out
}

impl<C: Execute> tower_service::Service<Request> for ReqwestTransport<C> {
    type Response = Response;
    type Error = Error;
    type Future = Pending<'static, Result<Response, Error>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    /// Returns once the head has arrived. The body arrives a chunk at a time.
    fn call(&mut self, req: Request) -> Self::Future {
        let started = self.start(req);
        Box::pin(async move {
            let resp = started.await?;
            let head = response(&resp, Body::default());
            Ok(head.map(|_| Body::stream(ByteStream::chunks(imp::chunks(resp), None))))
        })
    }
}
