//! Hand-written checks for the `uploads` fixture: a binary request body reaches the handler as it arrives.

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use e2e::v32_uploads::api::{self, Client, operations as ops, types};
use futures_util::StreamExt;

/// Counts what arrives; refuses a body that was buffered whole before the handler ran.
struct Server;

impl api::Api for Server {
    async fn put_file(
        &self,
        params: ops::put_file::Params,
        _: &api::Context<'_, ()>,
    ) -> Result<ops::put_file::Response, api::ApiError> {
        // ANCHOR: upload
        let mut body = params.body;
        if !body.is_streamed() {
            return Err(api::ApiError::new(
                api::http::StatusCode::INTERNAL_SERVER_ERROR,
                "the body was buffered before the handler",
            ));
        }
        let (mut bytes, mut chunks) = (0, 0);
        while let Some(chunk) = body.next().await {
            bytes += chunk?.len();
            chunks += 1;
        }
        // ANCHOR_END: upload
        Ok(ops::put_file::Response::Ok(types::PutFileOk { bytes: bytes as i64, chunks }))
    }
}

/// A request body sent in these frames.
struct Frames(VecDeque<bytes::Bytes>);

impl http_body::Body for Frames {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<bytes::Bytes>, Self::Error>>> {
        Poll::Ready(self.get_mut().0.pop_front().map(|b| Ok(http_body::Frame::data(b))))
    }
}

#[cfg(feature = "axum")]
fn put(limit: usize, frames: &[&'static [u8]]) -> (u16, String) {
    use tower::ServiceExt;
    let body = axum::body::Body::new(Frames(frames.iter().map(|f| bytes::Bytes::from_static(f)).collect()));
    let req = axum::http::Request::put("/files/a.bin").header("content-type", "application/octet-stream").body(body);
    let router = api::Server::new(Server).max_body_bytes(limit).into_axum();
    let resp = api::block_on(router.oneshot(req.unwrap())).unwrap();
    let status = resp.status().as_u16();
    let bytes = api::block_on(http_body_util::BodyExt::collect(resp.into_body())).unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(feature = "axum")]
#[test]
fn an_axum_upload_reaches_the_handler_frame_by_frame() {
    assert_eq!(put(1 << 20, &[b"abc", b"de", b"f"]), (200, r#"{"bytes":6,"chunks":3}"#.to_owned()));
}

#[cfg(feature = "axum")]
#[test]
fn an_upload_past_the_limit_fails_where_it_crosses_it() {
    let (status, _) = put(4, &[b"abc", b"de"]);
    assert_eq!(status, 413);
}

#[test]
fn a_streamed_client_upload_reaches_the_handler_streamed() {
    let server = api::Server::new(Server).into_service();
    let client = Client::new(server);
    let pieces = futures_util::stream::iter(vec![b"ab".to_vec(), b"cd".to_vec()]);
    let params = ops::put_file::Params::new("a.bin".to_owned(), api::Blob::stream(pieces));
    let resp = api::block_on(client.put_file(params)).unwrap();
    assert_eq!(resp, ops::put_file::Response::Ok(types::PutFileOk { bytes: 4, chunks: 2 }));
}
