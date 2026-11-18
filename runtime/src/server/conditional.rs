//! Conditional requests (RFC 9110 section 13).
//!
//! Some `GET` and `HEAD` requests get `304 Not Modified`. Their success response carries an `ETag` or a
//! `Last-Modified`. The `If-None-Match` or `If-Modified-Since` of the request says that the client already holds it.
//! A handler that will change a resource first checks `If-Match`, `If-Unmodified-Since` and `If-None-Match` with
//! [`Context::preconditions`]. It refuses with `412 Precondition Failed`.

use crate::http_date::{
    DAY_HOURS, DAY_SECONDS, HOUR_MINUTES, HOUR_SECONDS, MINUTE_SECONDS, MONTHS, YEAR_DIGITS, days_from_civil,
    two_digit_year,
};
use crate::{ApiError, Context, Message, Response, joined_header};

/// The current validators of a resource as its response would carry them. [`Context::preconditions`] compares the
/// conditions of the request with them.
///
/// These are the HTTP validators of RFC 9110 section 8.8 (`ETag` and `Last-Modified`). They have nothing to do with
/// the keywords of a schema.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Validators<'a> {
    /// The `ETag` header value with its quotes, such as `"33a64df5"` or `W/"33a64df5"`.
    pub etag: Option<&'a str>,
    /// The `Last-Modified` header value. It is an HTTP date such as `Sun, 06 Nov 1994 08:49:37 GMT`.
    pub last_modified: Option<&'a str>,
}

impl<C, Id> Context<'_, C, Id> {
    /// Checks the request's preconditions against the resource's current validators before changing it: `None` when
    /// the resource does not exist.
    ///
    /// The checks run in the order of RFC 9110 section 13.2.2. First `If-Match` (strong comparison) or else
    /// `If-Unmodified-Since`. Then `If-None-Match` (weak comparison). The server handles `If-Modified-Since` and an
    /// `If-None-Match` on a `GET` or `HEAD`. It answers `304` from the `ETag` and `Last-Modified` that your response
    /// carries.
    ///
    /// # Errors
    /// `412 Precondition Failed` in the error schema of the description when a condition does not hold.
    pub fn preconditions(&self, current: Option<Validators<'_>>) -> Result<(), ApiError> {
        let header = |name| joined_header(self.request.headers(), name);
        let etag = current.and_then(|c| c.etag);
        if let Some(cond) = header("if-match") {
            if !matches_any(&cond, current.is_some(), etag, strong) {
                return Err(failed("If-Match"));
            }
        } else if let Some(since) = header("if-unmodified-since").as_deref().and_then(http_date)
            && let Some(modified) = current.and_then(|c| c.last_modified).and_then(http_date)
            && modified > since
        {
            return Err(failed("If-Unmodified-Since"));
        }
        let read = matches!(*self.request.method(), http::Method::GET | http::Method::HEAD);
        match header("if-none-match") {
            Some(cond) if !read && matches_any(&cond, current.is_some(), etag, weak) => Err(failed("If-None-Match")),
            Some(_) | None => Ok(()),
        }
    }
}

fn failed(header: &str) -> ApiError {
    ApiError::new(http::StatusCode::PRECONDITION_FAILED, format!("precondition failed: {header}"))
}

/// The conditions of a `GET` or `HEAD` that a `304` answers. They are kept while the request goes through the layers.
#[derive(Debug, Default)]
pub(crate) struct Conditions {
    if_none_match: Option<String>,
    if_modified_since: Option<String>,
}

impl Conditions {
    /// Returns the request's `If-None-Match` and `If-Modified-Since`; none for another method.
    pub(crate) fn of(head: &http::request::Parts) -> Self {
        if !matches!(head.method, http::Method::GET | http::Method::HEAD) {
            return Self::default();
        }
        let joined = |name: http::header::HeaderName| {
            let lines: Vec<&str> = head.headers.get_all(name).iter().filter_map(|v| v.to_str().ok()).collect();
            (!lines.is_empty()).then(|| lines.join(","))
        };
        Self {
            if_none_match: joined(http::header::IF_NONE_MATCH),
            if_modified_since: joined(http::header::IF_MODIFIED_SINCE),
        }
    }

    /// Returns the response or `304 Not Modified` when it is a success that the client already holds.
    pub(crate) fn answer(self, resp: Response) -> Response {
        if (self.if_none_match.is_none() && self.if_modified_since.is_none()) || !resp.status().is_success() {
            return resp;
        }
        let held = match (&self.if_none_match, &self.if_modified_since) {
            // RFC 9110 section 13.1.3: `If-Modified-Since` is ignored beside `If-None-Match`.
            (Some(cond), _) => matches_any(cond, true, resp.header("etag").as_deref(), weak),
            (None, Some(since)) => {
                match (http_date(since), resp.header("last-modified").as_deref().and_then(http_date)) {
                    (Some(since), Some(modified)) => modified <= since,
                    _ => false,
                }
            }
            (None, None) => false,
        };
        if held { not_modified(resp) } else { resp }
    }
}

/// Returns `304 Not Modified` for a response. It has the headers without the body and without the metadata that
/// describes the body. A 304 must not send those (RFC 9110 section 15.4.5). `Content-Location` stays because it names
/// the representation.
fn not_modified(resp: Response) -> Response {
    let (mut head, _) = resp.into_parts();
    let names: Vec<http::HeaderName> = head.headers.keys().cloned().collect();
    for k in names {
        if k.as_str().starts_with("content-") && k != http::header::CONTENT_LOCATION {
            head.headers.remove(k);
        }
    }
    head.status = http::StatusCode::NOT_MODIFIED;
    Response::from_parts(head, crate::Body::default())
}

/// Does an `If-Range` value name the representation of the response (RFC 9110 section 13.1.5)? An entity tag must
/// match its `ETag` strongly. A date must match its `Last-Modified` exactly. Otherwise the client gets all of it.
pub(crate) fn if_range_holds(cond: &str, resp: &Response) -> bool {
    let cond = cond.trim();
    if cond.starts_with('"') || cond.starts_with("W/") {
        let etag = resp.header("etag");
        let current = etag.as_deref().and_then(|e| tags(e).next());
        return tags(cond).next().zip(current).is_some_and(|(a, b)| strong(a, b));
    }
    http_date(cond).is_some_and(|date| resp.header("last-modified").as_deref().and_then(http_date) == Some(date))
}

/// Does an `If-Match` or `If-None-Match` value match the current representation? `*` matches any that `exists`.
fn matches_any(cond: &str, exists: bool, etag: Option<&str>, same: fn(Tag<'_>, Tag<'_>) -> bool) -> bool {
    if cond.trim() == "*" {
        return exists;
    }
    let Some(current) = etag.and_then(|e| tags(e).next()) else { return false };
    tags(cond).any(|t| same(t, current))
}

/// An entity tag (RFC 9110 section 8.8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tag<'a> {
    weak: bool,
    opaque: &'a str,
}

/// Returns whether the tags match by strong comparison: neither is weak and the opaque text is the same.
fn strong(a: Tag<'_>, b: Tag<'_>) -> bool {
    !a.weak && !b.weak && a.opaque == b.opaque
}

/// Returns whether the tags match by weak comparison: the opaque text is the same whether weak or not.
fn weak(a: Tag<'_>, b: Tag<'_>) -> bool {
    a.opaque == b.opaque
}

/// Returns the entity tags of a comma-separated list up to the first malformed one. An opaque tag may hold a comma. So
/// the list is read tag by tag and not split.
fn tags(list: &str) -> impl Iterator<Item = Tag<'_>> {
    let mut rest = list;
    std::iter::from_fn(move || {
        rest = rest.trim_start_matches([' ', '\t', ',']);
        let (weak, quoted) = match rest.strip_prefix("W/") {
            Some(r) => (true, r),
            None => (false, rest),
        };
        let body = quoted.strip_prefix('"')?;
        let end = body.find('"')?;
        let opaque = body.get(..end)?;
        // etagc: `!`, `#` to `~` or obs-text.
        if !opaque.bytes().all(|b| b == b'!' || (b'#'..=b'~').contains(&b) || b >= OBS_TEXT) {
            return None;
        }
        rest = body.get(end + 1..)?;
        Some(Tag { weak, opaque })
    })
}

/// The first byte of obs-text. An entity tag may hold it (RFC 9110 section 5.5).
const OBS_TEXT: u8 = 0x80;

/// The last day of the longest month.
const MAX_MONTH_DAY: i64 = 31;

/// Returns an HTTP date as seconds since the Unix epoch. It may be in any of the three forms that a recipient must
/// accept (RFC 9110 section 5.6.7): IMF-fixdate, the obsolete RFC 850 form and asctime. `None` when it is none of them.
pub(crate) fn http_date(text: &str) -> Option<i64> {
    let words: Vec<&str> = text.split([' ', ',']).filter(|w| !w.is_empty()).collect();
    let (day, month, year, time) = match words.as_slice() {
        // `Sun, 06 Nov 1994 08:49:37 GMT`
        [_, day, month, year, time, "GMT"] if year.len() == YEAR_DIGITS => (*day, *month, year.parse().ok()?, *time),
        // `Sunday, 06-Nov-94 08:49:37 GMT`
        [_, date, time, "GMT"] => {
            let mut parts = date.split('-');
            let (day, month, yy) = (parts.next()?, parts.next()?, parts.next()?);
            (day, month, two_digit_year(yy.parse().ok()?), *time)
        }
        // `Sun Nov  6 08:49:37 1994`
        [_, month, day, time, year] => (*day, *month, year.parse().ok()?, *time),
        _ => return None,
    };
    let day: i64 = day.parse().ok()?;
    let month = MONTHS.iter().position(|m| *m == month)?;
    let mut clock = time.split(':').map(|n| n.parse::<i64>().ok());
    let (h, m, s) = (clock.next()??, clock.next()??, clock.next()??);
    if !(1..=MAX_MONTH_DAY).contains(&day) || h >= DAY_HOURS || m >= HOUR_MINUTES || s > MINUTE_SECONDS {
        return None;
    }
    let month = i64::try_from(month).ok()? + 1;
    Some(days_from_civil(year, month, day) * DAY_SECONDS + h * HOUR_SECONDS + m * MINUTE_SECONDS + s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RequestEvent, SatisfiedSecurity};

    #[test]
    fn the_three_date_forms_read_the_same_instant() {
        let fixed = http_date("Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(fixed, Some(784_111_777));
        assert_eq!(http_date("Sunday, 06-Nov-94 08:49:37 GMT"), fixed);
        assert_eq!(http_date("Sun Nov  6 08:49:37 1994"), fixed);
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        for bad in [
            "",
            "yesterday",
            "Sun, 06 Nov 1994 08:49:37 CET",
            "Sun, 32 Nov 1994 08:49:37 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
        ] {
            assert_eq!(http_date(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_tag_list_is_read_tag_by_tag_up_to_the_first_malformed_one() {
        let t = |weak, opaque| Tag { weak, opaque };
        assert_eq!(tags(r#""a", W/"b,c" ,"d""#).collect::<Vec<_>>(), [t(false, "a"), t(true, "b,c"), t(false, "d")]);
        assert_eq!(tags(r#""a", b, "c""#).collect::<Vec<_>>(), [t(false, "a")]);
        assert_eq!(tags(r#""a b""#).count(), 0, "a space is not an etagc");
        assert_eq!(tags("").count(), 0);
    }

    #[test]
    fn if_match_compares_strongly_and_if_none_match_weakly() {
        assert!(matches_any(r#""x""#, true, Some(r#""x""#), strong));
        assert!(!matches_any(r#"W/"x""#, true, Some(r#""x""#), strong));
        assert!(matches_any(r#"W/"x""#, true, Some(r#""x""#), weak));
        assert!(matches_any("*", true, None, strong), "`*` matches any current representation");
        assert!(!matches_any("*", false, None, weak), "and none when the resource does not exist");
        assert!(!matches_any(r#""x""#, true, None, weak), "a tag never matches a representation without one");
    }

    fn get(conditions: &[(&'static str, &'static str)]) -> Conditions {
        let mut head = http::Request::get("/products/1");
        for (k, v) in conditions {
            head = head.header(*k, *v);
        }
        Conditions::of(&head.body(()).unwrap_or_default().into_parts().0)
    }

    fn ok(headers: &[(&'static str, &'static str)]) -> Response {
        let mut r = crate::body_response(200, "application/json", b"{}".as_slice());
        for (k, v) in headers {
            r.set_header(k, *v).unwrap();
        }
        r
    }

    #[test]
    fn a_matching_if_none_match_answers_304_with_the_validators_and_no_body() {
        let resp = get(&[("if-none-match", r#"W/"other", "v1""#)]).answer(ok(&[
            ("ETag", r#""v1""#),
            ("Content-Length", "2"),
            ("Cache-Control", "max-age=60"),
        ]));
        assert_eq!(resp.status(), 304);
        assert_eq!((resp.content_type(), resp.body().bytes().len()), ("", 0));
        assert_eq!(resp.header("etag").as_deref(), Some(r#""v1""#));
        assert_eq!(resp.header("cache-control").as_deref(), Some("max-age=60"));
        assert_eq!(resp.header("content-length"), None);
    }

    #[test]
    fn a_stale_or_absent_validator_serves_the_response() {
        assert_eq!(get(&[("if-none-match", r#""v0""#)]).answer(ok(&[("ETag", r#""v1""#)])).status(), 200);
        assert_eq!(get(&[("if-none-match", r#""v1""#)]).answer(ok(&[])).status(), 200);
        assert_eq!(get(&[]).answer(ok(&[("ETag", r#""v1""#)])).status(), 200);
    }

    #[test]
    fn if_modified_since_answers_304_unless_if_none_match_is_there() {
        let lm = [("Last-Modified", "Sun, 06 Nov 1994 08:49:37 GMT")];
        assert_eq!(get(&[("if-modified-since", "Sun, 06 Nov 1994 08:49:37 GMT")]).answer(ok(&lm)).status(), 304);
        assert_eq!(get(&[("if-modified-since", "Sat, 05 Nov 1994 08:49:37 GMT")]).answer(ok(&lm)).status(), 200);
        assert_eq!(get(&[("if-modified-since", "not a date")]).answer(ok(&lm)).status(), 200);
        let both = [("if-none-match", r#""v0""#), ("if-modified-since", "Mon, 07 Nov 1994 08:49:37 GMT")];
        assert_eq!(get(&both).answer(ok(&lm)).status(), 200, "If-None-Match decides alone");
    }

    #[test]
    fn only_a_success_to_a_get_or_head_becomes_304() {
        let mut not_found = ok(&[("ETag", r#""v1""#)]);
        *not_found.status_mut() = http::StatusCode::NOT_FOUND;
        assert_eq!(get(&[("if-none-match", r#""v1""#)]).answer(not_found).status(), 404);
        let put = http::Request::put("/products/1").header("if-none-match", r#""v1""#).body(()).unwrap_or_default();
        assert_eq!(Conditions::of(&put.into_parts().0).answer(ok(&[("ETag", r#""v1""#)])).status(), 200);
    }

    fn check(
        method: &'static str,
        conditions: &[(&'static str, &'static str)],
        current: Option<Validators<'_>>,
    ) -> Option<String> {
        let mut request = crate::new_request(method, "/products/1".to_owned(), vec![]).unwrap();
        for (k, v) in conditions {
            request.set_header(k, *v).unwrap();
        }
        let event = RequestEvent::new(Some(crate::server::serve::TestOp), method);
        let ctx = Context::new(&request, crate::server::serve::TestOp, SatisfiedSecurity::default(), &event);
        ctx.preconditions(current).err().map(|e| format!("{} {}", e.status.as_u16(), e.message))
    }

    #[test]
    fn preconditions_refuse_a_change_to_a_representation_the_client_does_not_hold() {
        let v1 = Some(Validators { etag: Some(r#""v1""#), last_modified: Some("Sun, 06 Nov 1994 08:49:37 GMT") });
        assert_eq!(check("PUT", &[("if-match", r#""v1""#)], v1), None);
        assert_eq!(check("PUT", &[("if-match", r#""v0""#)], v1).as_deref(), Some("412 precondition failed: If-Match"));
        assert_eq!(
            check("PUT", &[("if-match", r#"W/"v1""#)], v1).as_deref(),
            Some("412 precondition failed: If-Match")
        );
        assert_eq!(check("PUT", &[("if-match", "*")], None).as_deref(), Some("412 precondition failed: If-Match"));
        assert_eq!(check("PUT", &[("if-none-match", "*")], None), None, "create only if absent");
        assert_eq!(
            check("PUT", &[("if-none-match", "*")], v1).as_deref(),
            Some("412 precondition failed: If-None-Match")
        );
        let older = [("if-unmodified-since", "Sat, 05 Nov 1994 08:49:37 GMT")];
        assert_eq!(check("DELETE", &older, v1).as_deref(), Some("412 precondition failed: If-Unmodified-Since"));
        let with_match = [("if-match", r#""v1""#), ("if-unmodified-since", "Sat, 05 Nov 1994 08:49:37 GMT")];
        assert_eq!(check("DELETE", &with_match, v1), None, "If-Unmodified-Since is ignored beside If-Match");
        assert_eq!(check("GET", &[("if-none-match", r#""v1""#)], v1), None, "the server answers a GET's 304");
        assert_eq!(check("PUT", &[], v1), None);
    }
}
