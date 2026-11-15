//! `sonic-rs`: 1.2x faster to decode and 1.3x faster to encode than `serde_json` without a copy of the input.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The deepest nesting that `sonic-rs` is given in a debug build. It is the limit of `serde_json`. `sonic-rs` stops at
/// 255 levels. A release build holds that on a 512 KiB stack. The larger frames of a debug build overflow a 2 MiB
/// worker stack first. Then one body of nested arrays would abort the server. A deeper body goes to `serde_json`, which
/// refuses it.
const MAX_DEPTH: usize = 128;

/// Creates `body` decoded or `None`.
pub(crate) fn from_slice<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    if cfg!(debug_assertions) && nested_past(body, MAX_DEPTH) {
        return None;
    }
    let mut d = sonic_rs::Deserializer::from_slice(body);
    let v = T::deserialize(crate::integral::Ints(&mut d)).ok()?;
    d.end().ok().map(|()| v)
}

/// What a byte outside a string does to the nesting.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Other,
    Open,
    Close,
    Quote,
}

/// Each byte's [`Class`]: one load and one well-predicted branch for the common byte.
const CLASSES: [Class; 256] = {
    let mut t = [Class::Other; 256];
    t[b'[' as usize] = Class::Open;
    t[b'{' as usize] = Class::Open;
    t[b']' as usize] = Class::Close;
    t[b'}' as usize] = Class::Close;
    t[b'"' as usize] = Class::Quote;
    t
};

/// Returns whether arrays and objects nest deeper than `limit` in `body`. Brackets inside strings do not count. Strings
/// are most of a body. They are skipped with `memchr` to their closing quote.
fn nested_past(body: &[u8], limit: usize) -> bool {
    let mut depth = 0_usize;
    let mut i = 0;
    while let Some(&b) = body.get(i) {
        match CLASSES[usize::from(b)] {
            Class::Other => i += 1,
            Class::Open => {
                depth += 1;
                if depth > limit {
                    return true;
                }
                i += 1;
            }
            Class::Close => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            Class::Quote => i = string_end(body, i + 1),
        }
    }
    false
}

/// Returns the index after the quote that closes a string whose text starts at `from`. Otherwise the end of `body`.
fn string_end(body: &[u8], mut from: usize) -> usize {
    while let Some(at) = body.get(from..).and_then(|rest| memchr::memchr2(b'"', b'\\', rest)) {
        let at = from + at;
        if body.get(at) == Some(&b'"') {
            return at + 1;
        }
        // An escape. The next byte is part of it even if it is a quote.
        from = at + "\\x".len();
    }
    body.len()
}

/// Returns `v` appended to `out`.
pub(crate) fn to_writer<T: Serialize>(out: &mut Vec<u8>, v: &T) -> Result<(), String> {
    sonic_rs::to_writer(out, &crate::finite::Finite(v)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn nesting_counts_brackets_outside_strings_only() {
        assert!(!super::nested_past(br#"{"a":[[1]],"s":"[[[[\"[["}"#, 3));
        assert!(super::nested_past(b"[[[[1]]]]", 3));
        assert!(!super::nested_past(b"[[[1]]][[[1]]]", 3), "siblings do not add up");
    }
}
