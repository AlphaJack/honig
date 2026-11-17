//! `uri-reference`, `iri` and `iri-reference`: the grammars of RFC 3986 and RFC 3987.

/// Which characters a reference may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chars {
    /// ASCII only (RFC 3986).
    Uri,
    /// ASCII and the non-ASCII characters of RFC 3987 (`ucschar`, and `iprivate` in a query).
    Iri,
}

/// `unreserved` and `sub-delims` without the letters and digits.
const PLAIN: &str = "-._~!$&'()*+,;=";
/// What a path holds besides `PLAIN`: `pchar`'s `:` and `@`, and the `/` between segments.
const PATH: &str = ":@/";
/// What a query and a fragment hold besides `PLAIN`.
const QUERY: &str = ":@/?";
/// The digits of a percent-encoded octet.
const PCT_DIGITS: usize = 2;
/// The ranges of RFC 3987's `ucschar`.
const UCSCHAR: [(u32, u32); 17] = [
    (0xA0, 0xD7FF),
    (0xF900, 0xFDCF),
    (0xFDF0, 0xFFEF),
    (0x1_0000, 0x1_FFFD),
    (0x2_0000, 0x2_FFFD),
    (0x3_0000, 0x3_FFFD),
    (0x4_0000, 0x4_FFFD),
    (0x5_0000, 0x5_FFFD),
    (0x6_0000, 0x6_FFFD),
    (0x7_0000, 0x7_FFFD),
    (0x8_0000, 0x8_FFFD),
    (0x9_0000, 0x9_FFFD),
    (0xA_0000, 0xA_FFFD),
    (0xB_0000, 0xB_FFFD),
    (0xC_0000, 0xC_FFFD),
    (0xD_0000, 0xD_FFFD),
    (0xE_1000, 0xE_FFFD),
];
/// The ranges of RFC 3987's `iprivate`.
const IPRIVATE: [(u32, u32); 3] = [(0xE000, 0xF8FF), (0xF_0000, 0xF_FFFD), (0x10_0000, 0x10_FFFD)];

/// Is `text` a reference (`URI-reference` or `IRI-reference`) or, when `absolute`, a URI or IRI with a scheme?
pub(crate) fn is_reference(text: &str, chars: Chars, absolute: bool) -> bool {
    let (rest, fragment) = text.split_once('#').map_or((text, None), |(r, f)| (r, Some(f)));
    let (rest, query) = rest.split_once('?').map_or((rest, None), |(r, q)| (r, Some(q)));
    if fragment.is_some_and(|f| !all(f, chars, QUERY, false)) || query.is_some_and(|q| !all(q, chars, QUERY, true)) {
        return false;
    }
    let (scheme, rest) = rest.split_once(':').filter(|(s, _)| is_scheme(s)).map_or((None, rest), |(s, r)| (Some(s), r));
    if absolute && scheme.is_none() {
        return false;
    }
    if let Some(after) = rest.strip_prefix("//") {
        let (authority, path) = after.find('/').map_or((after, ""), |end| after.split_at(end));
        return is_authority(authority, chars) && all(path, chars, PATH, false);
    }
    // Without a scheme the first segment holds no `:`: it would read as one (`path-noscheme`).
    let first_has_colon = rest.split('/').next().is_some_and(|s| s.contains(':'));
    (scheme.is_some() || !first_has_colon) && all(rest, chars, PATH, false)
}

/// Returns `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
fn is_scheme(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// Checks the grammar `[ userinfo "@" ] host [ ":" port ]`.
fn is_authority(authority: &str, chars: Chars) -> bool {
    let (userinfo, host_port) = authority.split_once('@').map_or((None, authority), |(u, h)| (Some(u), h));
    if userinfo.is_some_and(|u| !all(u, chars, ":", false)) {
        return false;
    }
    let (host, port) = match host_port.strip_prefix('[') {
        Some(literal) => match literal.split_once(']') {
            Some((ip, port)) if is_ip_literal(ip) => ("", port),
            _ => return false,
        },
        None => host_port.rsplit_once(':').map_or((host_port, ""), |(h, p)| (h, p)),
    };
    let port = port.strip_prefix(':').unwrap_or(port);
    let port_ok = port.bytes().all(|b| b.is_ascii_digit()) && (port.is_empty() || host_port.contains(':'));
    port_ok && all(host, chars, "", false)
}

/// Returns an IPv6 address or `IPvFuture` (`v` hex digits `.` and then text).
fn is_ip_literal(ip: &str) -> bool {
    if let Some(future) = ip.strip_prefix(['v', 'V']) {
        return future.split_once('.').is_some_and(|(version, rest)| {
            !version.is_empty()
                && version.bytes().all(|b| b.is_ascii_hexdigit())
                && !rest.is_empty()
                && all(rest, Chars::Uri, ":", false)
        });
    }
    ip.parse::<std::net::Ipv6Addr>().is_ok()
}

/// Does every character of `text` belong: letters, digits, `PLAIN`, `extra`, a percent-encoded octet, and for an IRI
/// `ucschar` (and `iprivate` when `private`)?
fn all(text: &str, chars: Chars, extra: &str, private: bool) -> bool {
    let mut it = text.chars();
    while let Some(c) = it.next() {
        let ok = match c {
            '%' => it.by_ref().take(PCT_DIGITS).filter(char::is_ascii_hexdigit).count() == PCT_DIGITS,
            c if c.is_ascii_alphanumeric() || PLAIN.contains(c) || extra.contains(c) => true,
            c if !c.is_ascii() => chars == Chars::Iri && (is_ucschar(c) || (private && in_ranges(c, &IPRIVATE))),
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

/// Returns a non-ASCII character that an IRI may hold anywhere.
pub(crate) fn is_ucschar(c: char) -> bool {
    in_ranges(c, &UCSCHAR)
}

/// Returns a character of a private-use range.
pub(crate) fn is_iprivate(c: char) -> bool {
    in_ranges(c, &IPRIVATE)
}

fn in_ranges(c: char, ranges: &[(u32, u32)]) -> bool {
    ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&u32::from(c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_uri_reference_is_a_uri_or_a_relative_reference() {
        for good in [
            "http://foo.bar/?baz=qux#quux",
            "//foo.bar/?baz=qux#quux",
            "/abc",
            "abc",
            "#fragment",
            "",
            "?q",
            "../a/b",
            "mailto:user@example.com",
            "http://[2001:db8::7]:8080/c=GB?objectClass?one",
            "http://[v1.fe80::a+en1]/",
            "http://user:pass@host:80/p%20q",
            "urn:oasis:names:specification:docbook:dtd:xml:4.1.2",
        ] {
            assert!(is_reference(good, Chars::Uri, false), "{good}");
        }
        for bad in [
            "\\\\WINDOWS\\fileshare",
            "abc\\def",
            "#frag\\ment",
            "a b",
            "1a:b",
            "http://host:8a/",
            "http://[::1/",
            "http://[zz]/",
            "%2",
            "%g0",
            "http://\u{00e9}.example/",
            "#a#b",
        ] {
            assert!(!is_reference(bad, Chars::Uri, false), "{bad}");
        }
    }

    #[test]
    fn an_iri_takes_non_ascii_characters_and_needs_a_scheme() {
        assert!(is_reference(
            "http://\u{0192}\u{00f8}\u{00f8}.\u{00df}\u{00e5}r/?\u{2202}\u{00e9}\u{0153}=\u{03c0}\u{00ee}x#\u{03c0}\u{00ee}\u{00fc}x",
            Chars::Iri,
            true
        ));
        assert!(is_reference("http://example.com/?\u{e000}", Chars::Iri, true), "iprivate in a query");
        assert!(!is_reference("http://example.com/\u{e000}", Chars::Iri, true), "iprivate outside a query");
        assert!(!is_reference("/abc", Chars::Iri, true), "no scheme");
        assert!(is_reference("/\u{00e2}\u{00e8}c", Chars::Iri, false));
        assert!(!is_reference("http://example.com/\u{fffe}", Chars::Iri, false), "a noncharacter");
    }

    /// RFC 3986 `IPvFuture`: `v`, hex digits, `.` and at least one more character.
    #[test]
    fn a_future_ip_literal_needs_text_after_its_version() {
        assert!(is_reference("http://[v1.x]/", Chars::Uri, true));
        assert!(!is_reference("http://[v1.]/", Chars::Uri, true));
    }
}
