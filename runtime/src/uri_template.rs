//! `uri-template`: the grammar of RFC 6570.

use crate::uri::{is_iprivate, is_ucschar};

/// The operators an expression may start with, the reserved ones included.
const OPERATORS: &str = "+#./;?&=,!@|";
/// What a literal never holds besides controls and a space.
// RFC 6570 section 2.1 with verified erratum 6937: an apostrophe is a literal.
const NOT_LITERAL: &str = "\"%<>\\^`{|}";
/// The most digits of a prefix modifier (`:9999`).
const MAX_LENGTH_DIGITS: usize = 4;
/// The digits of a percent-encoded octet.
const PCT_DIGITS: usize = 2;

/// Is `text` literals, percent-encoded octets and `{expressions}`?
pub(crate) fn is_uri_template(text: &str) -> bool {
    let mut rest = text;
    while let Some(c) = rest.chars().next() {
        let used = match c {
            '{' => match rest.find('}') {
                Some(end) if rest.get(1..end).is_some_and(is_expression) => end + 1,
                _ => return false,
            },
            '%' if rest.get(1..=PCT_DIGITS).is_some_and(hex) => 1 + PCT_DIGITS,
            c if is_literal(c) => c.len_utf8(),
            _ => return false,
        };
        rest = rest.get(used..).unwrap_or_default();
    }
    true
}

fn is_literal(c: char) -> bool {
    if c.is_ascii() {
        return !c.is_ascii_control() && c != ' ' && !NOT_LITERAL.contains(c);
    }
    is_ucschar(c) || is_iprivate(c)
}

/// Checks the grammar `[ operator ] varspec *( "," varspec )`.
fn is_expression(body: &str) -> bool {
    let list = body.strip_prefix(|c| OPERATORS.contains(c)).unwrap_or(body);
    list.split(',').all(is_varspec)
}

/// Returns a variable name and an optional `*` or `:max-length`.
fn is_varspec(spec: &str) -> bool {
    if let Some(name) = spec.strip_suffix('*') {
        return is_varname(name);
    }
    match spec.split_once(':') {
        Some((name, length)) => {
            is_varname(name)
                && (1..=MAX_LENGTH_DIGITS).contains(&length.len())
                && !length.starts_with('0')
                && length.bytes().all(|b| b.is_ascii_digit())
        }
        None => is_varname(spec),
    }
}

/// Returns `varchar *( ["."] varchar )`: letters, digits, `_` and percent-encoded octets, with single dots between
/// them.
fn is_varname(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.contains("..")
        && name.split('.').all(|part| {
            let mut rest = part;
            while let Some(c) = rest.chars().next() {
                let used = match c {
                    '%' if rest.get(1..=PCT_DIGITS).is_some_and(hex) => 1 + PCT_DIGITS,
                    c if c.is_ascii_alphanumeric() || c == '_' => 1,
                    _ => return false,
                };
                rest = rest.get(used..).unwrap_or_default();
            }
            true
        })
}

fn hex(digits: &str) -> bool {
    digits.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
#[expect(clippy::literal_string_with_formatting_args, reason = "a URI template writes its expressions in braces")]
mod tests {
    use super::*;

    #[test]
    fn a_template_is_literals_and_expressions() {
        for good in [
            "http://example.com/dictionary/{term:1}/{term}",
            "http://example.com/dictionary",
            "dictionary/{term:1}/{term}",
            "{+path}/here",
            "{#x,hello,y}",
            "{/list*}",
            "{;x,y,empty}",
            "{?x,y}",
            "{&x}",
            "{var:9999}",
            "{a.b_c%20}",
            "/caf\u{00e9}/{x}",
            "%20{x}",
            "",
        ] {
            assert!(is_uri_template(good), "{good}");
        }
        for bad in [
            "http://example.com/dictionary/{term:1}/{term",
            "{}",
            "{x,}",
            "{var:0}",
            "{var:10000}",
            "{a.}",
            "{..a}",
            "{a..b}",
            "{a b}",
            "a b",
            "a\"b",
            "%2",
            "{x}}",
            "{x*1}",
        ] {
            assert!(!is_uri_template(bad), "{bad}");
        }
    }
}
