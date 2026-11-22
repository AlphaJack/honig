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

/// Returns the reason phrase of a registered status as the `http` crate spells it. The generator derives the names of
/// response variants from it (`404` is `NotFound`).
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

/// Fails when a handler returns a status that its response variant does not stand for.
///
/// `own` is the class of a range variant (`2` for `2XX`), `None` for `default`. `codes` and `ranges` are the statuses
/// that the description declares. A variant stands for the statuses of its own range or, for `default`, those that no
/// other variant has: a client decodes any other as the declared response, not as this one.
///
/// # Errors
/// The status is one of another variant, or outside the range of this one.
pub fn check_status(status: u16, own: Option<u16>, codes: &[u16], ranges: &[u16]) -> Result<(), crate::ClientError> {
    let declared = codes.contains(&status);
    let in_range = |class: u16| status / CLASS == class;
    let wrong = match own {
        Some(class) => !in_range(class) || declared,
        None => declared || ranges.iter().any(|class| in_range(*class)),
    };
    if wrong {
        return Err(crate::ClientError::Encode(format!(
            "status {status} does not belong to the response variant that was returned: use the variant that declares it"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_status;

    #[test]
    fn a_variant_stands_for_its_own_statuses_only() {
        let (codes, ranges) = ([404, 409], [2]);
        assert!(check_status(201, Some(2), &codes, &ranges).is_ok(), "2XX stands for 201");
        assert!(check_status(404, Some(2), &codes, &ranges).is_err(), "404 is outside 2XX");
        assert!(check_status(500, None, &codes, &ranges).is_ok(), "default stands for what no variant has");
        assert!(check_status(404, None, &codes, &ranges).is_err(), "404 is declared");
        assert!(check_status(250, None, &codes, &ranges).is_err(), "250 is in the declared 2XX");
        assert!(check_status(209, Some(2), &[209], &ranges).is_err(), "a declared code beats its range");
    }
}
