//! Whether a body's `Content-Type` is one the description declares.

use crate::ClientError;

/// Is the `Content-Type` of a body one that the operation declares? A body that states none passes. Many clients
/// leave it out. The body then decodes or not on its own.
pub(crate) fn declared_media(actual: &str, declared: &[&str]) -> bool {
    // Bodies are read as UTF-8: one that says it is not would be read wrong (JSON is UTF-8 by RFC 8259).
    if !utf8_charset(actual) {
        return false;
    }
    // A client with only a range (`image/*`) sends the bytes as `application/octet-stream`. The generated client
    // does this too. No `Content-Type` may hold a wildcard.
    let unnamed = media_essence(actual).eq_ignore_ascii_case("application/octet-stream")
        && declared.iter().any(|d| d.contains('*'));
    actual.trim().is_empty() || unnamed || declared.iter().any(|d| media_matches(actual, d))
}

/// Is a response's `Content-Type` one the description declares for its status?
///
/// A response should state its type when it has a body. A request need not. The client decodes by what was declared.
/// A body of another type would be read as the wrong one. A body without a type is read as the one type declared
/// for its status (RFC 9110 section 8.3 lets a recipient assume one). Small text answers often omit it: `OK` from a
/// health check or `Not Found`. With several declared there is nothing to tell them apart. `body` is `None` for a
/// stream because its body is not read yet.
///
/// # Errors
/// [`crate::ClientError::UnexpectedMedia`] with what was sent and what is declared.
pub fn check_response_media(
    status: u16,
    content_type: &str,
    body: Option<&[u8]>,
    declared: &[&str],
) -> Result<(), ClientError> {
    let fits = if content_type.trim().is_empty() {
        declared.len() == 1 || body.is_some_and(<[u8]>::is_empty)
    } else {
        declared_media(content_type, declared)
    };
    if fits {
        return Ok(());
    }
    Err(ClientError::UnexpectedMedia {
        status,
        content_type: content_type.to_owned(),
        declared: declared.join(", "),
        body: bytes::Bytes::copy_from_slice(body.unwrap_or_default()),
    })
}

/// Returns whether the `charset` of a `Content-Type` is UTF-8 (or its subset US-ASCII). No `charset` also counts.
fn utf8_charset(content_type: &str) -> bool {
    content_type.split(';').skip(1).filter_map(|p| p.trim().split_once('=')).all(|(k, v)| {
        !k.trim().eq_ignore_ascii_case("charset")
            || ["utf-8", "utf8", "us-ascii"].iter().any(|ok| v.trim().trim_matches('"').eq_ignore_ascii_case(ok))
    })
}

/// Returns the response media type to send by RFC 9110 section 12.5.1.
///
/// That is the index in `offered` (in the order of the description) of the type that the `Accept` of the request
/// prefers. `None` when it accepts none of them.
///
/// The most specific range that matches a type gives its quality. `application/json` comes before `application/*`
/// and that comes before `*/*`. A quality of `0` refuses it. Among equal qualities the order of the description
/// decides. A request without `Accept` or with one that does not parse gets the first offered type.
#[must_use]
pub fn negotiate(accept: Option<&str>, offered: &[&str]) -> Option<usize> {
    // The ranges are parsed again for each offered type and not collected: a server offers one or two.
    let ranges = match accept {
        Some(a) if a.split(',').find_map(range).is_some() => move || a.split(',').filter_map(range),
        _ => return (!offered.is_empty()).then_some(0),
    };
    let quality = |media: &str| {
        let essence = media_essence(media);
        let (kind, sub) = essence.split_once('/').unwrap_or((essence, ""));
        ranges()
            .filter(|(k, s, _)| {
                // A range that the description offers (`image/*`) takes any type of it as `Accept` ranges do.
                (*k == "*" || kind == "*" || k.eq_ignore_ascii_case(kind))
                    && (*s == "*" || sub == "*" || s.eq_ignore_ascii_case(sub))
            })
            // Specificity: an exact type, then `type/*` and then `*/*`.
            .max_by_key(|(k, s, _)| (*k != "*", *s != "*"))
            .map(|(.., q)| q)
            .unwrap_or_default()
    };
    let mut best: Option<(usize, u16)> = None;
    for (i, media) in offered.iter().enumerate() {
        let q = quality(media);
        if q > 0 && best.is_none_or(|(_, b)| q > b) {
            best = Some((i, q));
        }
    }
    best.map(|(i, _)| i)
}

/// `q=1` in thousandths. A range without `q` has it.
const FULL: u16 = 1000;
/// RFC 9110 section 12.4.2: a `qvalue` has at most three decimals.
const DECIMALS: usize = 3;

/// Returns one range of an `Accept` header: type, subtype and quality in thousandths. `None` when it does not parse.
fn range(part: &str) -> Option<(&str, &str, u16)> {
    let mut params = part.split(';');
    let (kind, sub) = params.next()?.trim().split_once('/')?;
    if kind.is_empty() || sub.is_empty() {
        return None;
    }
    let mut q = FULL;
    for p in params {
        if let Some(("q" | "Q", v)) = p.trim().split_once('=') {
            q = quality(v.trim())?;
        }
    }
    Some((kind, sub, q))
}

/// Returns a `qvalue` (`0`, `0.5`, `1.000`) in thousandths.
fn quality(v: &str) -> Option<u16> {
    let (whole, frac) = v.split_once('.').unwrap_or((v, ""));
    if frac.len() > DECIMALS || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let thousandths: u16 = format!("{frac:0<3}").parse().ok()?;
    match whole {
        "0" => Some(thousandths),
        "1" if thousandths == 0 => Some(FULL),
        _ => None,
    }
}

/// Returns a media type without its parameters: `Application/XML; charset=utf-8` gives `Application/XML`. Media types
/// are case-insensitive, so it is compared with `eq_ignore_ascii_case` and not copied in lower case.
pub(crate) fn media_essence(ct: &str) -> &str {
    ct.split(';').next().unwrap_or("").trim()
}

/// Does `text` end with `suffix`, ignoring ASCII case?
#[cfg(feature = "xml")]
fn ends_with_ignore_case(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len() && text.as_bytes()[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix.as_bytes())
}

/// Does a `Content-Type` value match a declared media type (ranges allowed)?
pub fn media_matches(actual: &str, declared: &str) -> bool {
    let (a, d) = (media_essence(actual), media_essence(declared));
    if d == "*/*" || a.eq_ignore_ascii_case(d) {
        return true;
    }
    // `image/*` takes any `image/...`: the part before the `*` with its `/`.
    match d.strip_suffix('*') {
        Some(prefix) if prefix.ends_with('/') => {
            a.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        }
        _ => false,
    }
}

/// Is this part content type JSON?
#[cfg(feature = "multipart")]
pub(crate) fn json_type(ct: &str) -> bool {
    let m = ct.split(';').next().unwrap_or("").trim();
    m == "application/json" || m.ends_with("+json") || m.ends_with("/json")
}

/// Returns an XML media type (`application/xml`, `text/xml` or `+xml`). Parameters are ignored.
#[cfg(feature = "xml")]
pub(crate) fn is_xml_media(ct: &str) -> bool {
    let m = media_essence(ct);
    ends_with_ignore_case(m, "/xml") || ends_with_ignore_case(m, "+xml")
}

#[cfg(test)]
mod tests {
    use super::{media_matches, negotiate};

    const OFFERED: [&str; 2] = ["application/json", "application/jsonl"];

    #[test]
    fn the_preferred_type_wins() {
        assert_eq!(negotiate(None, &OFFERED), Some(0), "no Accept: the first offered");
        assert_eq!(negotiate(Some("application/jsonl"), &OFFERED), Some(1));
        assert_eq!(negotiate(Some("application/json;q=0.5, application/jsonl"), &OFFERED), Some(1));
        assert_eq!(negotiate(Some("*/*"), &OFFERED), Some(0), "a tie goes to the description's order");
        assert_eq!(negotiate(Some("application/*;q=0.2, application/jsonl;q=0.9"), &OFFERED), Some(1));
    }

    #[test]
    fn the_most_specific_range_decides() {
        assert_eq!(negotiate(Some("application/*, application/json;q=0"), &OFFERED), Some(1), "q=0 refuses json");
        assert_eq!(negotiate(Some("text/html"), &OFFERED), None, "nothing acceptable");
        assert_eq!(negotiate(Some("*/*;q=0"), &OFFERED), None);
        assert_eq!(negotiate(Some("Application/JSONL"), &OFFERED), Some(1), "case-insensitive");
    }

    #[test]
    fn an_unreadable_header_is_no_preference() {
        assert_eq!(negotiate(Some("garbage"), &OFFERED), Some(0));
        assert_eq!(negotiate(Some("application/json;q=2"), &OFFERED), Some(0), "a quality over 1 is no range");
        assert_eq!(negotiate(Some(""), &[]), None, "nothing offered");
        assert_eq!(negotiate(Some("application/json;q=0.5, application/jsonl;q=1.5"), &OFFERED), Some(0));
    }

    #[test]
    fn a_quality_of_one_may_have_decimals() {
        for one in ["1", "1.", "1.0", "1.000"] {
            let accept = format!("application/json;q=0.5, application/jsonl;q={one}");
            assert_eq!(negotiate(Some(&accept), &OFFERED), Some(1), "{accept}");
        }
    }

    #[test]
    fn a_range_ends_with_a_slash_and_a_star() {
        assert!(media_matches("image/png", "image/*"));
        assert!(media_matches("IMAGE/png", "image/*"));
        assert!(!media_matches("imagery/png", "image*"), "a star without the slash is no range");
        assert!(!media_matches("text/png", "image/*"));
    }

    /// The most specific range decides: `*/*;q=0` does not refuse a type that `Accept` names.
    #[test]
    fn a_named_type_wins_over_any_type() {
        assert_eq!(negotiate(Some("text/plain, */*;q=0"), &["text/plain"]), Some(0));
        assert_eq!(negotiate(Some("text/plain;q=0, */*"), &["text/plain"]), None);
    }

    #[test]
    fn a_range_has_a_type_and_a_subtype() {
        assert!(super::range("text/").is_none() && super::range("/plain").is_none());
        assert!(super::range("text/plain").is_some());
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn json_types_end_in_json() {
        for json in ["application/json", "application/problem+json", "text/json; charset=utf-8"] {
            assert!(super::json_type(json), "{json}");
        }
        assert!(!super::json_type("text/plain"));
    }
}
