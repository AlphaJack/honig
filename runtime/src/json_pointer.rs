//! `json-pointer` (RFC 6901) and `relative-json-pointer` (draft-bhutton-relative-json-pointer-00).

/// Is `text` empty or `/` and reference tokens in which every `~` is `~0` or `~1`?
pub(crate) fn is_json_pointer(text: &str) -> bool {
    let bytes = text.as_bytes();
    (text.is_empty() || text.starts_with('/'))
        && bytes.iter().enumerate().all(|(i, b)| *b != b'~' || matches!(bytes.get(i + 1), Some(b'0' | b'1')))
}

/// Is `text` a count of levels up, an optional index change (`+1` or `-1`), and then `#` or a JSON pointer?
pub(crate) fn is_relative_json_pointer(text: &str) -> bool {
    let (up, rest) = leading_digits(text);
    if !non_negative(up) {
        return false;
    }
    let rest = match rest.strip_prefix(['+', '-']) {
        Some(change) => {
            let (by, rest) = leading_digits(change);
            if !non_negative(by) {
                return false;
            }
            rest
        }
        None => rest,
    };
    rest == "#" || is_json_pointer(rest)
}

fn leading_digits(text: &str) -> (&str, &str) {
    text.split_at(text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len()))
}

/// Returns `0` or digits without a leading zero.
fn non_negative(digits: &str) -> bool {
    digits == "0" || (!digits.is_empty() && !digits.starts_with('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_escapes_every_tilde() {
        for good in ["", "/", "/foo/bar~0/baz~1/%a", "/foo//bar", "/foo/bar/", "/~1.1", "/ ", "/a\u{00e9}"] {
            assert!(is_json_pointer(good), "{good}");
        }
        for bad in ["/foo/bar~", "/~2", "/~-1", "/~~", "#", "#/", "a", "0", "a/a"] {
            assert!(!is_json_pointer(bad), "{bad}");
        }
    }

    #[test]
    fn a_relative_pointer_starts_with_levels_up() {
        for good in ["1", "0/foo/bar", "2/0/baz/1/zip", "0#", "120/foo/bar", "1+1/a", "0-1#"] {
            assert!(is_relative_json_pointer(good), "{good}");
        }
        for bad in ["/foo/bar", "-1/foo/bar", "+1/foo/bar", "0##", "01/a", "01#", "", "1+/a", "a"] {
            assert!(!is_relative_json_pointer(bad), "{bad}");
        }
    }
}
