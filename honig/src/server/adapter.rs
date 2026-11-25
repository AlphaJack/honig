//! The server's one adapter: the `http` crate's types. hyper and axum speak them.

use std::pin::Pin;
use std::task::{Context, Poll, ready};

use crate::{ClientError, source::BodyError, source::BodySource, source::ReadAll};

/// Returns the `Content-Length` that a request declares. It sizes the buffer of its body.
pub(crate) fn content_length(headers: &http::HeaderMap) -> Option<usize> {
    headers.get(http::header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

/// A request body that the adapter has not consumed. For an operation that decodes it the server buffers it whole into
/// a buffer sized from `Content-Length`. Otherwise a handler consumes it piece by piece. Either way it stops past
/// `limit`.
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

/// Returns the whole body in one buffer allocated once. A body of one frame is that frame, not a copy. With its size
/// known from `Content-Length` (`hint`) or from the body, each frame is appended to a buffer of that size and then
/// dropped, so the body is in memory once. Otherwise the frames are kept and joined at the end into a buffer of their
/// total, and never grown frame by frame. Buffering stops past `limit`.
async fn read_body<B>(mut body: B, hint: Option<usize>, limit: usize) -> Result<bytes::Bytes, BodyError>
where
    B: http_body::Body<Data = bytes::Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    let known = hint.or_else(|| http_body::Body::size_hint(&body).exact().and_then(|n| usize::try_from(n).ok()));
    let (mut frames, mut joined, mut read) = (Vec::<bytes::Bytes>::new(), None::<Vec<u8>>, 0);
    while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let frame = frame.map_err(|e| BodyError::Unreadable(e.to_string()))?;
        let Ok(data) = frame.into_data() else { continue };
        read += data.len();
        if read > limit {
            return Err(BodyError::TooLarge);
        }
        match (&mut joined, known) {
            (Some(out), _) => out.extend_from_slice(&data),
            (None, Some(size)) if !frames.is_empty() => {
                let mut out = Vec::with_capacity(size.min(limit).max(read));
                for f in std::mem::take(&mut frames) {
                    out.extend_from_slice(&f);
                }
                out.extend_from_slice(&data);
                joined = Some(out);
            }
            (None, _) => frames.push(data),
        }
    }
    Ok(match (joined, frames.len()) {
        (Some(out), _) => out.into(),
        (None, 1) => frames.pop().unwrap_or_default(),
        (None, _) => {
            let mut out = Vec::with_capacity(read);
            for f in &frames {
                out.extend_from_slice(f);
            }
            out.into()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body of one frame is that frame: buffering it copies nothing, so a large body is in memory once.
    #[test]
    fn a_body_of_one_frame_is_not_copied() {
        let frame = bytes::Bytes::from(vec![7; 1 << 10]);
        let read = crate::block_on(read_body(crate::Body::new(frame.clone()), Some(frame.len()), 1 << 20))
            .ok()
            .unwrap_or_default();
        assert_eq!(read.as_ptr(), frame.as_ptr());
        assert!(matches!(crate::block_on(read_body(crate::Body::new(frame), None, 10)), Err(BodyError::TooLarge)));
    }
}
