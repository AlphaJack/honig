//! Percent-encoding and decoding of parameter text.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

/// Every byte but RFC 3986's unreserved ones (`A-Z a-z 0-9 - . _ ~`).
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// [`COMPONENT`] without the reserved characters of RFC 3986 for `allowReserved`.
const RESERVED_KEPT: &AsciiSet = &COMPONENT
    .remove(b':')
    .remove(b'/')
    .remove(b'?')
    .remove(b'#')
    .remove(b'[')
    .remove(b']')
    .remove(b'@')
    .remove(b'!')
    .remove(b'$')
    .remove(b'&')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')')
    .remove(b'*')
    .remove(b'+')
    .remove(b',')
    .remove(b';')
    .remove(b'=');

/// [`RESERVED_KEPT`] that still encodes `/`, `?` and `#`. A path value that contains
/// one would end its segment, its path or the URL (3.2 Path Templating: "MUST
/// NOT contain any unescaped generic syntax characters").
const PATH_RESERVED_KEPT: &AsciiSet = &RESERVED_KEPT.add(b'/').add(b'?').add(b'#');

/// Returns `s` percent-encoded. With `allow_reserved` the RFC 3986 reserved characters pass through.
pub fn pct(s: &str, allow_reserved: bool) -> String {
    utf8_percent_encode(s, if allow_reserved { RESERVED_KEPT } else { COMPONENT }).to_string()
}

/// Returns [`pct`] for a path parameter. `allow_reserved` keeps every reserved character except `/`, `?` and `#`.
pub(crate) fn pct_path(s: &str, allow_reserved: bool) -> String {
    utf8_percent_encode(s, if allow_reserved { PATH_RESERVED_KEPT } else { COMPONENT }).to_string()
}

/// Decodes `%XX` escapes. Invalid escapes stay as they are. `None` when the decoded bytes are not UTF-8.
pub fn pct_decode(s: &str) -> Option<String> {
    percent_decode_str(s).decode_utf8().ok().map(std::borrow::Cow::into_owned)
}
