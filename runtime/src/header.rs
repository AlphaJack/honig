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

/// Returns why a browser cannot send the header `name` with `value`, or `None` when it can.
///
/// `fetch` sets `Cookie` itself and takes only ASCII text: an adapter that dropped such a header would send a request
/// without it and no word. The adapter for a browser uses it.
pub fn browser_refuses(name: &str, value: &http::HeaderValue) -> Option<String> {
    if name.eq_ignore_ascii_case("cookie") {
        return Some(
            "a browser sets `Cookie` itself: send cookies with `prepare(|r| r.credentials(RequestCredentials::Include))`"
                .to_owned(),
        );
    }
    value.to_str().is_err().then(|| format!("`{name}` holds a value that is not ASCII text, which `fetch` cannot send"))
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

#[cfg(test)]
mod browser_tests {
    use super::browser_refuses;

    /// A header that `fetch` would drop is reported: a cookie and a value that is not ASCII.
    #[test]
    fn a_header_that_fetch_drops_is_refused() {
        let v = |s: &[u8]| http::HeaderValue::from_bytes(s).unwrap();
        assert!(browser_refuses("Cookie", &v(b"a=b")).is_some_and(|w| w.contains("RequestCredentials")));
        assert!(browser_refuses("x-name", &v("café".as_bytes())).is_some_and(|w| w.contains("`x-name`")));
        assert!(browser_refuses("x-name", &v(b"plain")).is_none());
        assert!(browser_refuses("authorization", &v(b"Bearer t")).is_none());
    }
}
