//! A transport over any tower `Service` of `http` requests. That is a hyper-util client under tower-http layers or a
//! stack of your own. It is included when the crate depends on `tower`.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http_body::Body as HttpBody;
use tower_service::Service;

use crate::transport::Pending;
use crate::{Body, ByteStream, ClientError, MaybeSend, MaybeSync, Request, Response};

/// The error of a tower service or of an `http` body.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

use crate::client::transport::failed;

/// A transport over a tower `Service`: `Client::new(api::TowerTransport::new(service, "https://api.example.com"))`.
/// Retries, timeouts, tracing and decompression come from the layers around `service`.
#[derive(Debug, Clone)]
pub struct TowerTransport<S> {
    /// The service that sends each request. It is cloned for each one.
    pub service: S,
    /// Base URL without a trailing slash.
    pub base_url: String,
}

impl<S> TowerTransport<S> {
    /// Creates a transport over `service` for this base URL.
    pub fn new(service: S, base_url: impl Into<String>) -> Self {
        Self { service, base_url: crate::base_url(base_url) }
    }
}

impl<S, B> TowerTransport<S>
where
    S: Service<Request, Response = http::Response<B>> + Clone + MaybeSend + MaybeSync + 'static,
    S::Future: MaybeSend,
    S::Error: Into<BoxError>,
    B: HttpBody<Data = bytes::Bytes> + MaybeSend + 'static,
    B::Error: Into<BoxError>,
{
    /// Sends `req` and returns the response once its head is in. The body is still to read.
    fn start(&self, mut req: Request) -> impl Future<Output = Result<Response, ClientError>> + MaybeSend + use<S, B> {
        let mut service = self.service.clone();
        let target = http::Uri::try_from(crate::target(&self.base_url, req.uri()));
        async move {
            *req.uri_mut() = target.map_err(ClientError::transport)?;
            std::future::poll_fn(|cx| service.poll_ready(cx)).await.map_err(failed)?;
            let resp = service.call(req).await.map_err(failed)?;
            Ok(resp.map(|body| Body::stream(ByteStream::chunks(Frames(Box::pin(body)), None))))
        }
    }
}

impl<S, B> Service<Request> for TowerTransport<S>
where
    S: Service<Request, Response = http::Response<B>> + Clone + MaybeSend + MaybeSync + 'static,
    S::Future: MaybeSend,
    S::Error: Into<BoxError>,
    B: HttpBody<Data = bytes::Bytes> + MaybeSend + 'static,
    B::Error: Into<BoxError>,
{
    type Response = Response;
    type Error = ClientError;
    type Future = Pending<'static, Result<Response, ClientError>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), ClientError>> {
        Poll::Ready(Ok(()))
    }

    /// Returns once the head has arrived. Each chunk of the body is a data frame of the service's body.
    fn call(&mut self, req: Request) -> Self::Future {
        Box::pin(self.start(req))
    }
}

/// The data frames of a response body. Trailers are skipped.
struct Frames<B>(Pin<Box<B>>);

impl<B> futures_core::Stream for Frames<B>
where
    B: HttpBody<Data = bytes::Bytes>,
    B::Error: Into<BoxError>,
{
    type Item = Result<bytes::Bytes, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match self.0.as_mut().poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(data) => Poll::Ready(Some(Ok(data))),
                    Err(_trailers) => continue,
                },
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(failed(e)))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            };
        }
    }
}
