//! The client's side of a transport: any tower `Service` of `http` requests.

use std::borrow::Cow;
use std::future::Future;

use tower_service::Service;

use crate::{Body, ClientError, Request, Response};
use crate::{transport::MaybeSend, transport::MaybeSync};

/// The error of a tower service.
type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What a generated client sends through: any tower `Service` of `http` requests.
///
/// It is a bound, not a trait to implement. Write a `Service`, or take `tower::service_fn`, a server's
/// `into_service()` or an adapter such as `ReqwestTransport`.
///
/// The response body may stream. A client reads it whole when the operation does not stream. The service is cloned
/// for each request as tower's `oneshot` does. It is `Send` natively but not on `wasm32`.
pub trait Transport:
    Service<Request, Response = Response, Error: Into<BoxError>, Future: MaybeSend>
    + Clone
    + MaybeSend
    + MaybeSync
    + 'static
{
}

impl<S> Transport for S where
    S: Service<Request, Response = Response, Error: Into<BoxError>, Future: MaybeSend>
        + Clone
        + MaybeSend
        + MaybeSync
        + 'static
{
}

/// Returns the failure of a service as the client reports it.
pub(crate) fn failed(e: impl Into<BoxError>) -> ClientError {
    let e: Box<dyn std::error::Error> = e.into();
    ClientError::transport(e)
}

/// Sends `req` through a clone of `transport` and returns once the head of the response is in. Its body may still be
/// to come.
pub fn send_streaming<T: Transport>(
    transport: &T,
    req: Request,
) -> impl Future<Output = Result<Response, ClientError>> + MaybeSend + use<T> {
    let mut service = transport.clone();
    async move {
        std::future::poll_fn(|cx| service.poll_ready(cx)).await.map_err(failed)?;
        service.call(req).await.map_err(failed)
    }
}

/// Sends `req` through a clone of `transport` and reads the response body whole.
pub fn send<T: Transport>(
    transport: &T,
    req: Request,
) -> impl Future<Output = Result<Response, ClientError>> + MaybeSend + use<T> {
    let sent = send_streaming(transport, req);
    async move {
        let (head, body) = sent.await?.into_parts();
        let body = if body.is_streamed() { Body::new(body.collect().await?) } else { body };
        Ok(Response::from_parts(head, body))
    }
}

/// The parameters of a generated client method, owned or borrowed.
///
/// `client.op(params)` moves them. `client.op(&params)` lends them. A borrowed body that the client only reads (JSON,
/// XML, a form or multipart) is encoded from the reference. A body that it sends as it is (bytes, text or a stream) is
/// cloned as the caller would have done.
pub trait IntoArg<'a, T: Clone> {
    /// Returns the parameters as given.
    fn into_arg(self) -> Cow<'a, T>;
}

impl<'a, T: Clone> IntoArg<'a, T> for T {
    fn into_arg(self) -> Cow<'a, T> {
        Cow::Owned(self)
    }
}

impl<'a, T: Clone> IntoArg<'a, T> for &'a T {
    fn into_arg(self) -> Cow<'a, T> {
        Cow::Borrowed(self)
    }
}
