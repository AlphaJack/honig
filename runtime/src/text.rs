//! Text bodies and the text and errors that every other body is read and written with.

use std::borrow::Cow;
use std::fmt;

use crate::ClientError;

/// Returns a body that does not decode as [`ClientError::Decode`].
pub fn decode_err(status: Option<u16>, body: &[u8], e: impl fmt::Display) -> ClientError {
    ClientError::Decode {
        status,
        message: e.to_string(),
        body: bytes::Bytes::copy_from_slice(body),
        pointer: String::new(),
    }
}

/// Encodes a text body.
pub fn text_encode(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// Decodes a text body. Invalid UTF-8 is replaced.
pub fn text_decode(body: &[u8]) -> String {
    lossy(body).into_owned()
}

/// Returns `body` as text with invalid UTF-8 replaced.
pub(crate) fn lossy(body: &[u8]) -> Cow<'_, str> {
    utf8(body).map_or_else(|| String::from_utf8_lossy(body), Cow::Borrowed)
}

/// Returns `bytes` as text when they are UTF-8. The `simd` feature checks valid text twice as fast.
pub(crate) fn utf8(bytes: &[u8]) -> Option<&str> {
    #[cfg(feature = "simd")]
    return simdutf8::basic::from_utf8(bytes).ok();
    #[cfg(not(feature = "simd"))]
    std::str::from_utf8(bytes).ok()
}

/// Appends each piece. That is what `out.push_str(&format!(..))` writes but without a `String` for each call.
pub(crate) fn push_all(out: &mut String, pieces: &[&str]) {
    for p in pieces {
        out.push_str(p);
    }
}

#[cfg(test)]
mod tests {
    use super::{text_decode, text_encode};

    #[test]
    fn a_text_body_is_its_utf8_bytes() {
        assert_eq!(text_encode("hé"), "hé".as_bytes());
        assert_eq!(text_encode(""), b"");
        assert_eq!(text_decode(b"h\xFF"), "h\u{FFFD}");
    }
}
