//! `pattern` and `patternProperties`: ECMA-262 regular expressions with Unicode semantics as JSON Schema reads them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The pattern of one check site. It is compiled on first use. Lookups after that take no lock.
#[derive(Debug, Default)]
pub struct Pattern(OnceLock<Option<regress::Regex>>);

impl Pattern {
    /// Creates a pattern that compiles on first use, for a `static`.
    #[must_use]
    pub const fn new() -> Self {
        Self(OnceLock::new())
    }
}

/// Does `s` match `pattern` compiled once into `re`? A pattern that `regress` cannot compile matches nothing.
#[must_use]
pub fn pattern_matches(re: &Pattern, pattern: &str, s: &str) -> bool {
    is_match(re.0.get_or_init(|| compile(pattern)).as_ref(), s)
}

/// Returns [`pattern_matches`] for patterns known only at run time (schema fragments and `patternProperties`). Each is
/// compiled once per pattern and thread and looked up without a lock.
#[must_use]
pub fn pattern_matches_dyn(pattern: &str, s: &str) -> bool {
    thread_local! {
        static CACHE: RefCell<BTreeMap<String, Option<regress::Regex>>> = const { RefCell::new(BTreeMap::new()) };
    }
    CACHE.with_borrow_mut(|cache| {
        if let Some(re) = cache.get(pattern) {
            return is_match(re.as_ref(), s);
        }
        is_match(cache.entry(pattern.to_owned()).or_insert_with(|| compile(pattern)).as_ref(), s)
    })
}

/// Returns whether `pattern` compiles as an ECMA-262 regular expression. JSON Schema patterns are ECMA-262 with
/// Unicode semantics (`u`). Some only compile without them, such as `[\-\_]` with escapes that Unicode mode forbids.
/// They are checked as plain ECMA-262 as most validators do.
pub(crate) fn compiles(pattern: &str) -> bool {
    compile(pattern).is_some()
}

fn compile(pattern: &str) -> Option<regress::Regex> {
    regress::Regex::with_flags(pattern, "u").or_else(|_| regress::Regex::new(pattern)).ok()
}

/// A pattern that does not compile matches nothing: a value that it should judge is never taken unchecked.
/// `generate` refuses such a pattern, so generated code never has one.
fn is_match(re: Option<&regress::Regex>, s: &str) -> bool {
    re.is_some_and(|re| re.find(s).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_matches_anywhere_and_one_that_does_not_compile_matches_nothing() {
        static RE: Pattern = Pattern::new();
        assert!(pattern_matches(&RE, "^[a-z]+$", "abc") && !pattern_matches(&RE, "^[a-z]+$", "ABC"));
        assert!(pattern_matches_dyn(r"[\-\_]", "_"), "plain ECMA-262 when Unicode mode refuses it");
        assert!(!pattern_matches_dyn("(", "anything"));
    }

    #[test]
    fn a_pattern_compiles_with_or_without_unicode() {
        assert!(compiles("^[a-z]+$") && compiles(r"[\-\_]"));
        assert!(!compiles("(a"));
    }
}
