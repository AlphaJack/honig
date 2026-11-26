//! Text bodies and the text and errors that every other body is decoded and encoded with.

use std::fmt;

use crate::ClientError;

/// Returns a body that does not decode as [`ClientError::Decode`].
pub(crate) fn decode_err(status: Option<u16>, body: &[u8], e: impl fmt::Display) -> ClientError {
    ClientError::Decode {
        status,
        message: e.to_string(),
        body: bytes::Bytes::copy_from_slice(body),
        pointer: String::new(),
    }
}

/// Returns how many characters (Unicode scalar values) `s` has, as `chars().count()` does, in a constant too: the
/// length that `minLength` and `maxLength` count.
#[must_use]
pub const fn char_count(s: &str) -> usize {
    // Each character starts at a boundary: the boundaries before the end count them.
    let (mut i, mut n) = (0, 0);
    while i < s.len() {
        if s.is_char_boundary(i) {
            n += 1;
        }
        i += 1;
    }
    n
}

/// Encodes a text body.
pub fn text_encode(s: &str) -> Vec<u8> {
    s.as_bytes().to_vec()
}

/// Encodes a text body from `T`, the type of its schema: a string as it is, any other value as its JSON text.
///
/// # Errors
/// `T` does not serialize.
pub fn text_encode_as<T: serde::Serialize>(value: T) -> Result<Vec<u8>, ClientError> {
    let value = serde_json::to_value(value).map_err(|e| ClientError::Encode(e.to_string()))?;
    Ok(value.as_str().map_or_else(|| value.to_string(), str::to_owned).into_bytes())
}

/// Decodes a text body.
///
/// # Errors
/// The body is not UTF-8.
pub fn text_decode(status: Option<u16>, body: &[u8]) -> Result<String, ClientError> {
    utf8_text(status, body).map(str::to_owned)
}

/// Returns a body as text.
///
/// # Errors
/// The body is not UTF-8: a server answers 400 and a client reports a body that does not decode.
pub fn utf8_text(status: Option<u16>, body: &[u8]) -> Result<&str, ClientError> {
    utf8(body).ok_or_else(|| decode_err(status, body, "the body is not UTF-8"))
}

/// Returns `bytes` as text when they are UTF-8.
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
        assert!(text_decode(None, b"h\xFF").is_err(), "invalid UTF-8 is refused, not replaced");
    }
}
