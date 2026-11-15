//! HTTP status codes (RFC 9110 section 15) beyond what `http::StatusCode` names: the ranges and classes that the
//! runtime and the generator decide on.

use std::ops::Range;

/// The statuses that turn a request away for what it carries (RFC 9110 section 15.5).
///
/// That is a bad request, method, length, precondition, size, URI, media type, range or expectation and unprocessable
/// content.
pub const REFUSALS: [std::ops::RangeInclusive<u16>; 3] = [400..=400, 405..=417, 422..=422];
/// The statuses a response other than an informational one may have.
pub const FINAL: Range<u16> = 200..600;
/// The codes of one class: `4XX` is 400 to 499.
pub const CLASS: u16 = 100;

/// Returns the status that the code field of an error body names. A number that is no status is `500`.
#[must_use]
pub fn status_of(code: impl TryInto<u16>) -> http::StatusCode {
    code.try_into()
        .ok()
        .and_then(|c| http::StatusCode::from_u16(c).ok())
        .unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR)
}

/// Returns the reason phrase of a registered status as the `http` crate names it. The generator names response variants
/// after it (`404` is `NotFound`).
#[must_use]
pub fn reason(code: u16) -> Option<&'static str> {
    http::StatusCode::from_u16(code).ok()?.canonical_reason()
}

/// The phrases of the status classes `1XX` to `5XX` by their first digit.
pub const CLASSES: &[(u16, &str)] =
    &[(1, "Informational"), (2, "Success"), (3, "Redirection"), (4, "Client Error"), (5, "Server Error")];

/// Returns the phrase of a status class by its first digit.
#[must_use]
pub fn class(digit: u16) -> Option<&'static str> {
    CLASSES.iter().find(|(c, _)| *c == digit).map(|(_, phrase)| *phrase)
}
