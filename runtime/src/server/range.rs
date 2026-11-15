//! Range requests (RFC 9110 section 14): one byte range of a binary response, for a download that resumes or a
//! player that seeks.
//!
//! The server answers `Range` itself on a `GET` whose success response is binary, such as `application/zip` or an
//! image. Your handler returns the whole body as usual:
//!
//! | Request | Response |
//! |---|---|
//! | No `Range` | `200` with `Accept-Ranges: bytes` |
//! | `Range: bytes=1000-`, `bytes=0-499` or `bytes=-500` | `206 Partial Content`: those bytes and `Content-Range: bytes 1000-4999/5000` |
//! | A range that starts past the end | `416 Range Not Satisfiable` with `Content-Range: bytes */5000` |
//! | `If-Range` next to it with an `ETag` or date that the body no longer has | `200`: the whole new body |
//!
//! Only a body whose length is known can be cut. That is a body held whole (shared bytes of a [`crate::Blob`] are cut
//! without a copy) or a [`crate::Blob::ranged`]. The server calls the `open(first, last)` of a ranged blob once with
//! the bytes it sends. So the bytes before the range are never read.
//!
//! These go out whole with no `Accept-Ranges`, as RFC 9110 allows: a [`crate::Blob::stream`] of unknown length, JSON
//! and other structured bodies, several ranges in one request and a `Range` that does not parse. An operation that
//! declares a `206` (or `2XX`) or a `Range` parameter of its own answers ranges in its handler
//! ([`Op::ranges`](crate::Op::ranges)). The server leaves it alone.

use crate::server::conditional::if_range_holds;
use crate::{Body, Response};

/// The one range unit that HTTP defines. `Accept-Ranges` names it.
const BYTES: &str = "bytes";

/// The `Range` and `If-Range` of a request. They are kept while the request goes through the layers.
#[derive(Debug, Default)]
pub(crate) struct Ranges {
    /// The response of the operation may be cut and the method is `GET` or `HEAD`. Its `200` says `Accept-Ranges`.
    on: bool,
    /// `Range` on a `GET`.
    range: Option<String>,
    if_range: Option<String>,
}

impl Ranges {
    /// Returns the `Range` and `If-Range` of the request when `on`. The operation lets the server answer them.
    pub(crate) fn of(head: &http::request::Parts, on: bool) -> Self {
        let on = on && matches!(head.method, http::Method::GET | http::Method::HEAD);
        let text =
            |name: http::header::HeaderName| head.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let get = head.method == http::Method::GET;
        Self {
            on,
            range: (on && get).then(|| text(http::header::RANGE)).flatten(),
            if_range: (on && get).then(|| text(http::header::IF_RANGE)).flatten(),
        }
    }

    /// Returns the response cut to the range asked for. That is `206` with `Content-Range` or `416` when no byte of it
    /// exists. A body held whole is cut without a copy. A body that streams is cut only when its source opens at the
    /// range.
    pub(crate) fn answer(self, mut resp: Response) -> Response {
        if !self.on || resp.status() != http::StatusCode::OK {
            return resp;
        }
        let body = resp.body();
        let seekable = body.is_empty().then(|| body.chunks().seekable()).flatten();
        if body.is_streamed() && seekable.is_none() {
            return resp;
        }
        resp.headers_mut().insert(http::header::ACCEPT_RANGES, http::HeaderValue::from_static(BYTES));
        let len = seekable.unwrap_or_else(|| resp.body().len() as u64);
        let asked = self.range.filter(|_| self.if_range.as_deref().is_none_or(|c| if_range_holds(c, &resp)));
        match asked.map_or(Asked::Ignored, |r| byte_range(&r, len)) {
            Asked::Ignored => {}
            Asked::Unsatisfiable => {
                let mut refused = crate::new_response(http::StatusCode::RANGE_NOT_SATISFIABLE.as_u16());
                content_range(&mut refused, format!("{BYTES} */{len}"));
                return refused;
            }
            Asked::Bytes(first, last) if seekable.is_some() => {
                resp.body().chunks().cut(first, last);
                *resp.status_mut() = http::StatusCode::PARTIAL_CONTENT;
                content_range(&mut resp, format!("{BYTES} {first}-{last}/{len}"));
            }
            Asked::Bytes(first, last) => {
                let (Ok(first), Ok(end)) = (usize::try_from(first), usize::try_from(last + 1)) else { return resp };
                let (bytes, _) = std::mem::take(resp.body_mut()).into_parts();
                *resp.body_mut() = Body::new(bytes.slice(first..end));
                *resp.status_mut() = http::StatusCode::PARTIAL_CONTENT;
                content_range(&mut resp, format!("{BYTES} {first}-{last}/{len}"));
            }
        }
        resp
    }
}

/// Sets the `Content-Range` of `resp`. Its text is made of digits, `bytes`, `-`, `/` and `*`.
fn content_range(resp: &mut Response, range: String) {
    if let Ok(v) = http::HeaderValue::try_from(range) {
        resp.headers_mut().insert(http::header::CONTENT_RANGE, v);
    }
}

/// What a `Range` asks of a body (RFC 9110 section 14.1.2).
enum Asked {
    /// Nothing that the server answers. That is another unit, several ranges or a malformed one.
    Ignored,
    /// Bytes past the end only.
    Unsatisfiable,
    /// The first and last byte. Both are in the body.
    Bytes(u64, u64),
}

/// Returns what `range` asks of a body of `len` bytes.
fn byte_range(range: &str, len: u64) -> Asked {
    let Some((unit, set)) = range.trim().split_once('=') else { return Asked::Ignored };
    let Some((first, last)) = set.trim().split_once('-') else { return Asked::Ignored };
    if !unit.trim().eq_ignore_ascii_case(BYTES) || set.contains(',') {
        return Asked::Ignored;
    }
    let within =
        |first: u64, last: u64| if first < len { Asked::Bytes(first, last.min(len - 1)) } else { Asked::Unsatisfiable };
    match (first.trim().is_empty(), digits(first), digits(last)) {
        // `-n`: the last `n` bytes.
        (true, _, Some(0)) => Asked::Unsatisfiable,
        (true, _, Some(n)) => within(len.saturating_sub(n), u64::MAX),
        (false, Some(first), None) if last.trim().is_empty() => within(first, u64::MAX),
        (false, Some(first), Some(last)) if first <= last => within(first, last),
        _ => Asked::Ignored,
    }
}

/// Returns a run of decimal digits and nothing else.
fn digits(text: &str) -> Option<u64> {
    let text = text.trim();
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    const OK: u16 = http::StatusCode::OK.as_u16();
    const PARTIAL_CONTENT: u16 = http::StatusCode::PARTIAL_CONTENT.as_u16();
    const RANGE_NOT_SATISFIABLE: u16 = http::StatusCode::RANGE_NOT_SATISFIABLE.as_u16();
    use crate::{Blob, Message};

    /// A ranged `GET` of `body` with `range` and `if_range`. A `200` that carries `etag` answers it.
    fn ranged(body: Blob, range: &str, if_range: Option<&str>) -> Response {
        let mut req = http::Request::get("/files/1").header(http::header::RANGE, range);
        if let Some(c) = if_range {
            req = req.header(http::header::IF_RANGE, c);
        }
        let (head, ()) = req.body(()).unwrap().into_parts();
        let mut resp = crate::body_response(OK, "application/octet-stream", body.into_body());
        resp.set_header("ETag", "\"v1\"").unwrap();
        Ranges::of(&head, true).answer(resp)
    }

    /// The body: the bytes held whole and then every piece of its stream.
    fn bytes(resp: &Response) -> Vec<u8> {
        let mut out = resp.body().bytes().to_vec();
        if let Some(mut source) = resp.body().chunks().take_source() {
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            while let std::task::Poll::Ready(Some(piece)) = source.as_mut().poll_piece(&mut cx) {
                out.extend_from_slice(&piece.unwrap());
            }
        }
        out
    }

    /// A body of `BODY` that opens at the range. `opened` records each range that it was opened at.
    fn seekable(opened: &std::sync::Arc<std::sync::Mutex<Vec<(u64, u64)>>>) -> Blob {
        let opened = opened.clone();
        Blob::ranged(BODY.len() as u64, move |first, last| {
            opened.lock().unwrap().push((first, last));
            let (first, last) = (usize::try_from(first).unwrap(), usize::try_from(last).unwrap());
            futures_util::stream::iter([&BODY[first..=last]].map(<[u8]>::to_vec))
        })
    }

    const BODY: &[u8] = b"0123456789";

    #[test]
    fn one_range_is_cut_out_with_its_place_in_the_whole() {
        for (range, want, place) in [
            ("bytes=2-5", &b"2345"[..], "bytes 2-5/10"),
            ("bytes=7-", b"789", "bytes 7-9/10"),
            ("bytes=-3", b"789", "bytes 7-9/10"),
            ("bytes=8-100", b"89", "bytes 8-9/10"),
            ("BYTES = 0-0", b"0", "bytes 0-0/10"),
        ] {
            let resp = ranged(Blob::from(BODY.to_vec()), range, None);
            assert_eq!(resp.status(), PARTIAL_CONTENT, "{range}");
            assert_eq!(bytes(&resp), want, "{range}");
            assert_eq!(resp.header("content-range").as_deref(), Some(place), "{range}");
            assert_eq!(resp.header("accept-ranges").as_deref(), Some("bytes"), "{range}");
        }
    }

    #[test]
    fn a_shared_buffer_is_cut_without_a_copy() {
        let shared = bytes::Bytes::from_static(BODY);
        let kept = Blob::from(shared.clone());
        let resp = ranged(kept, "bytes=4-6", None);
        assert_eq!(resp.status(), PARTIAL_CONTENT);
        assert_eq!(bytes(&resp), b"456");
        drop(shared);
    }

    #[test]
    fn a_seekable_body_is_opened_once_at_the_range() {
        for (range, status, want, at) in [
            ("bytes=2-5", PARTIAL_CONTENT, &b"2345"[..], (2, 5)),
            ("bytes=-3", PARTIAL_CONTENT, b"789", (7, 9)),
            ("bytes=8-100", PARTIAL_CONTENT, b"89", (8, 9)),
            ("bytes=0-1,4-5", OK, BODY, (0, 9)),
        ] {
            let opened = std::sync::Arc::default();
            let resp = ranged(seekable(&opened), range, None);
            assert_eq!((resp.status().as_u16(), bytes(&resp)), (status, want.to_vec()), "{range}");
            assert_eq!(*opened.lock().unwrap(), [at], "{range}");
            assert_eq!(resp.header("accept-ranges").as_deref(), Some("bytes"), "{range}");
        }
    }

    #[test]
    fn a_seekable_body_past_its_end_is_never_opened() {
        let opened = std::sync::Arc::default();
        let resp = ranged(seekable(&opened), "bytes=10-", None);
        assert_eq!(
            (resp.status().as_u16(), resp.header("content-range").as_deref()),
            (RANGE_NOT_SATISFIABLE, Some("bytes */10"))
        );
        assert!(opened.lock().unwrap().is_empty());
    }

    #[test]
    fn a_range_past_the_end_is_not_satisfiable() {
        for range in ["bytes=10-", "bytes=10-20", "bytes=-0"] {
            let resp = ranged(Blob::from(BODY.to_vec()), range, None);
            assert_eq!(resp.status(), RANGE_NOT_SATISFIABLE, "{range}");
            assert_eq!(resp.header("content-range").as_deref(), Some("bytes */10"), "{range}");
            assert!(resp.body().bytes().is_empty(), "{range}");
        }
    }

    #[test]
    fn what_the_server_ignores_gets_the_whole_body() {
        for range in ["bytes=0-1,4-5", "items=0-1", "bytes=5-2", "bytes=a-b", "bytes=a-5", "bytes=+1-2", "bytes"] {
            let resp = ranged(Blob::from(BODY.to_vec()), range, None);
            assert_eq!((resp.status().as_u16(), bytes(&resp)), (OK, BODY.to_vec()), "{range}");
            assert_eq!(resp.header("accept-ranges").as_deref(), Some("bytes"), "{range}");
        }
    }

    #[test]
    fn if_range_cuts_only_the_representation_it_names() {
        let current = ranged(Blob::from(BODY.to_vec()), "bytes=0-1", Some("\"v1\""));
        assert_eq!((current.status().as_u16(), bytes(&current)), (PARTIAL_CONTENT, b"01".to_vec()));
        for stale in ["\"v0\"", "W/\"v1\"", "Sun, 06 Nov 1994 08:49:37 GMT"] {
            let resp = ranged(Blob::from(BODY.to_vec()), "bytes=0-1", Some(stale));
            assert_eq!((resp.status().as_u16(), bytes(&resp)), (OK, BODY.to_vec()), "{stale}");
        }
    }

    #[test]
    fn an_operation_that_does_not_allow_ranges_is_left_alone() {
        let (head, ()) =
            http::Request::get("/").header(http::header::RANGE, "bytes=0-1").body(()).unwrap().into_parts();
        let resp = Ranges::of(&head, false).answer(Response::new(Body::new(BODY)));
        assert_eq!((resp.status().as_u16(), resp.header("accept-ranges")), (OK, None));
    }
}
