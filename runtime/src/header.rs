//! Header values to and from text. Nothing here depends on the rest of the runtime.

use std::borrow::Cow;

/// Returns a header value: literals (`application/json`) are used in place; anything else is copied once. `None` for a
/// value with a line break or another control character.
pub(crate) fn header_value(v: Cow<'static, str>) -> Option<http::HeaderValue> {
    match v {
        Cow::Borrowed(s) if s.bytes().all(|b| b == b'\t' || (b' '..=b'~').contains(&b)) => {
            Some(http::HeaderValue::from_static(s))
        }
        Cow::Borrowed(s) => http::HeaderValue::from_str(s).ok(),
        Cow::Owned(s) => http::HeaderValue::try_from(s).ok(),
    }
}

/// Returns a header value as text. It is borrowed when it is UTF-8. Otherwise it is decoded as ISO-8859-1. Adapters use
/// it.
pub fn header_text(bytes: &[u8]) -> Cow<'_, str> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(latin1(bytes)),
    }
}

/// Returns ISO-8859-1: each byte is the code point of the same value (RFC 9110 section 5.5 `obs-text`).
pub(crate) fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{header_text, header_value};

    #[test]
    fn a_header_value_has_no_control_character_but_a_tab() {
        assert!(header_value(Cow::Borrowed("a\tb")).is_some());
        assert!(header_value(Cow::Borrowed("a\nb")).is_none());
        assert!(header_value(Cow::Owned("a\rb".to_owned())).is_none());
    }

    #[test]
    fn text_that_is_not_utf8_is_latin1() {
        assert_eq!(header_text(b"caf\xE9"), "café");
        assert!(matches!(header_text(b"cafe"), Cow::Borrowed("cafe")));
    }
}
