//! The server's one adapter: the `http` crate's types. hyper and axum speak them.

use std::pin::Pin;
use std::task::{Context, Poll, ready};

use crate::{BodyError, BodySource, ClientError, ReadAll};

/// Returns the `Content-Length` that a request declares. It sizes the buffer of its body.
pub(crate) fn content_length(headers: &http::HeaderMap) -> Option<usize> {
    headers.get(http::header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

/// A request body that the adapter has not read. For an operation that decodes it the server reads it whole into a
/// buffer sized from `Content-Length`. Otherwise a handler reads it piece by piece. Either way it stops past `limit`.
pub(crate) struct Unread<B> {
    body: B,
    hint: Option<usize>,
    limit: usize,
    seen: usize,
}

/// Returns `body` left unread for [`Body::stream`](crate::Body::stream).
pub(crate) fn unread<B>(body: B, headers: &http::HeaderMap, limit: usize) -> crate::ByteStream
where
    B: http_body::Body<Data = bytes::Bytes> + Send + Unpin + 'static,
    B::Error: std::fmt::Display,
{
    crate::ByteStream::new(Unread { body, hint: content_length(headers), limit, seen: 0 })
}

impl<B> BodySource for Unread<B>
where
    B: http_body::Body<Data = bytes::Bytes> + Send + Unpin + 'static,
    B::Error: std::fmt::Display,
{
    fn poll_piece(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<bytes::Bytes, ClientError>>> {
        let this = self.get_mut();
        loop {
            let Some(frame) = ready!(Pin::new(&mut this.body).poll_frame(cx)) else { return Poll::Ready(None) };
            let data = match frame {
                Err(e) => return Poll::Ready(Some(Err(ClientError::Transport(e.to_string())))),
                Ok(frame) => match frame.into_data() {
                    Ok(data) if !data.is_empty() => data,
                    Ok(_) | Err(_) => continue,
                },
            };
            this.seen += data.len();
            if this.seen > this.limit {
                return Poll::Ready(Some(Err(ClientError::TooLarge(this.limit))));
            }
            return Poll::Ready(Some(Ok(data)));
        }
    }

    fn read_all(self: Pin<Box<Self>>, limit: usize) -> ReadAll {
        let Self { body, hint, limit: own, .. } = *Pin::into_inner(self);
        Box::pin(read_body(body, hint, limit.min(own)))
    }
}

/// Returns the whole body in one buffer sized from `Content-Length` (`hint`). Each frame is appended and then dropped.
/// So the body is held once and never as frames plus a joined copy. Reading stops past `limit`.
async fn read_body<B>(mut body: B, hint: Option<usize>, limit: usize) -> Result<Vec<u8>, BodyError>
where
    B: http_body::Body<Data = bytes::Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let mut out = Vec::with_capacity(hint.unwrap_or(0).min(limit));
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|e| BodyError::Unreadable(e.to_string()))?;
        if let Ok(data) = frame.into_data() {
            if out.len() + data.len() > limit {
                return Err(BodyError::TooLarge);
            }
            out.extend_from_slice(&data);
        }
    }
    Ok(out)
}
