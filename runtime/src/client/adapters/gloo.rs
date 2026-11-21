//! Browser `fetch` transport through `gloo-net`. It is included when the crate
//! depends on `gloo-net` and compiled only for `wasm32`.

use std::rc::Rc;

use wasm_bindgen::JsCast as _;

use crate::transport::Pending;
use crate::{Body, ByteStream, ClientError, Request, Response};

/// Changes each request before it is sent.
type Prepare = Rc<dyn Fn(gloo_net::http::RequestBuilder) -> gloo_net::http::RequestBuilder>;

/// Browser `fetch` transport: `Client::new(api::GlooTransport::new("/api"))`.
///
/// A response streams: each chunk comes as `fetch` reads it. A request body is read whole
/// first because `fetch` takes no streamed one.
#[derive(Clone)]
pub struct GlooTransport {
    /// Base URL without a trailing slash. A path such as `/api` resolves
    /// against the page's origin.
    pub base_url: String,
    prepare: Option<Prepare>,
}

impl std::fmt::Debug for GlooTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlooTransport").field("base_url", &self.base_url).finish_non_exhaustive()
    }
}

impl GlooTransport {
    /// Creates a transport for this base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: crate::message::base_url(base_url), prepare: None }
    }

    /// Changes every request before it is sent, such as credentials, mode, cache or an
    /// abort signal. Send cookies cross-origin with
    /// `.prepare(|r| r.credentials(web_sys::RequestCredentials::Include))`.
    #[must_use]
    pub fn prepare(
        mut self,
        f: impl Fn(gloo_net::http::RequestBuilder) -> gloo_net::http::RequestBuilder + 'static,
    ) -> Self {
        self.prepare = Some(Rc::new(f));
        self
    }

    fn build(&self, req: &http::request::Parts, body: bytes::Bytes) -> Result<gloo_net::http::Request, ClientError> {
        let url = crate::message::target(&self.base_url, &req.uri);
        let method =
            gloo_net::http::Method::from_bytes(req.method.as_str().as_bytes()).map_err(ClientError::transport)?;
        // `append` keeps repeated headers; the builder's `header` would replace them.
        let headers = gloo_net::http::Headers::new();
        for (k, v) in &req.headers {
            if let (false, Ok(v)) = (forbidden(k.as_str()), v.to_str()) {
                headers.append(k.as_str(), v);
            }
        }
        let mut b = gloo_net::http::RequestBuilder::new(&url).method(method).headers(headers);
        if let Some(f) = &self.prepare {
            b = f(b);
        }
        // `fetch` rejects a body on GET and HEAD. It also rejects an empty one.
        let built = if body.is_empty() { b.build() } else { b.body(Vec::from(body)) };
        built.map_err(ClientError::transport)
    }
}

/// Returns whether the browser sets the header `name` itself. `fetch` drops or rejects such headers.
fn forbidden(name: &str) -> bool {
    const NAMES: [&str; 6] = ["host", "content-length", "cookie", "connection", "keep-alive", "transfer-encoding"];
    NAMES.iter().any(|n| name.eq_ignore_ascii_case(n))
}

impl GlooTransport {
    /// Sends `req` and returns once the head of the response has arrived. Its body is still to read.
    async fn start(&self, req: Request) -> Result<(Response, gloo_net::http::Response), ClientError> {
        // `fetch` takes no streamed request body: its pieces are read first.
        let (head, body) = req.into_parts();
        let body = body.collect().await?;
        let built = self.build(&head, body);
        // Network and CORS failures surface as one opaque `TypeError`.
        let resp = built?.send().await.map_err(ClientError::transport)?;
        let mut out = crate::message::new_response(resp.status());
        for (k, v) in resp.headers().entries() {
            if let (Ok(k), Ok(v)) = (http::HeaderName::try_from(k), http::HeaderValue::try_from(v)) {
                out.headers_mut().append(k, v);
            }
        }
        Ok((out, resp))
    }
}

impl tower_service::Service<Request> for GlooTransport {
    type Response = Response;
    type Error = ClientError;
    type Future = Pending<'static, Result<Response, ClientError>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), ClientError>> {
        std::task::Poll::Ready(Ok(()))
    }

    /// Returns once the head has arrived. Each chunk of the body is one `read()` of `fetch`.
    fn call(&mut self, req: Request) -> Self::Future {
        let transport = self.clone();
        Box::pin(async move {
            let (out, resp) = transport.start(req).await?;
            // A response without a body (`204` or a `HEAD`) has no stream. It ends at once.
            let reader = resp.body().map(|s| s.get_reader().unchecked_into());
            let stream = ByteStream::chunks(futures_util::stream::unfold(reader, read), None);
            Ok(out.map(|_| Body::stream(stream)))
        })
    }
}

/// Returns the next chunk of a response body from its `ReadableStream` and the reader of the rest. Each chunk is one
/// `read()` as the browser gives it.
async fn read(
    reader: Option<web_sys::ReadableStreamDefaultReader>,
) -> Option<(Result<bytes::Bytes, ClientError>, Option<web_sys::ReadableStreamDefaultReader>)> {
    let reader = reader?;
    let read = match wasm_bindgen_futures::JsFuture::from(reader.read()).await {
        Ok(read) => read.unchecked_into::<web_sys::ReadableStreamReadResult>(),
        Err(e) => return Some((Err(ClientError::transport(format!("reading the response body: {e:?}"))), None)),
    };
    if read.get_done().unwrap_or(true) {
        return None;
    }
    Some((Ok(bytes::Bytes::from(js_sys::Uint8Array::new(&read.get_value()).to_vec())), Some(reader)))
}
