//! A request target, a query and a `Cookie` header split into their parts and pairs.

use std::borrow::Cow;

/// Splits `path?query` into its parts.
pub fn split_target(target: &str) -> (&str, &str) {
    // Absolute-form targets (`https://host/path` from proxies or operation
    // servers) route by their path.
    let target = match target.split_once("://") {
        Some((scheme, rest)) if !scheme.contains('/') => rest.find('/').and_then(|i| rest.get(i..)).unwrap_or("/"),
        _ => target,
    };
    target.split_once('?').unwrap_or((target, ""))
}

/// Parses a query string into decoded (key, value) pairs.
pub fn parse_query(q: &str) -> Vec<(Cow<'_, str>, Cow<'_, str>)> {
    form_urlencoded::parse(q.as_bytes()).collect()
}

/// Decoded query pairs. They are borrowed from the target unless decoding changed them.
pub type Query<'a> = [(Cow<'a, str>, Cow<'a, str>)];

/// Returns the trimmed (name, value) pairs of a `Cookie` header.
#[cfg(feature = "cookie")]
pub(crate) fn cookie_pairs(header: &str) -> impl Iterator<Item = (&str, &str)> {
    cookie::Cookie::split_parse(header).filter_map(Result::ok).filter_map(|c| Some((c.name_raw()?, c.value_raw()?)))
}

/// Parses a `Cookie` header into (name, value) pairs.
#[cfg(feature = "cookie")]
pub fn parse_cookies(header: &str) -> Vec<(String, String)> {
    cookie_pairs(header).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}
