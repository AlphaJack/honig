//! `pattern` and `patternProperties`: ECMA-262 regular expressions with Unicode semantics as JSON Schema reads them.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// The pattern of one check site, with its text. It is compiled on first use. Lookups after that take no lock.
#[derive(Debug)]
pub struct Pattern {
    text: &'static str,
    re: OnceLock<Option<regress::Regex>>,
}

impl Pattern {
    /// Creates a pattern that compiles on first use, for a `static`.
    #[must_use]
    pub const fn new(text: &'static str) -> Self {
        Self { text, re: OnceLock::new() }
    }
}

/// Does `s` match `re`? A pattern that `regress` cannot compile matches nothing.
#[must_use]
pub fn pattern_matches(re: &Pattern, s: &str) -> bool {
    is_match(re.re.get_or_init(|| compile(re.text)).as_ref(), s)
}

/// Returns the text of `re`.
#[must_use]
pub const fn pattern_text(re: &Pattern) -> &'static str {
    re.text
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
        static RE: Pattern = Pattern::new("^[a-z]+$");
        assert!(pattern_matches(&RE, "abc") && !pattern_matches(&RE, "ABC"));
        assert!(pattern_matches_dyn(r"[\-\_]", "_"), "plain ECMA-262 when Unicode mode refuses it");
        assert!(!pattern_matches_dyn("(", "anything"));
    }

    #[test]
    fn a_pattern_compiles_with_or_without_unicode() {
        assert!(compiles("^[a-z]+$") && compiles(r"[\-\_]"));
        assert!(!compiles("(a"));
    }
}

/// Reads the undeclared names of a struct that `patterns[index]` matches, each value decoded as `T`.
///
/// A name that an earlier pattern matches too is checked here but kept by that pattern's map, so it is written once.
/// It is the `deserialize_with` of a flattened field of `patternProperties`.
///
/// # Errors
/// A value that a matching name holds does not decode as `T`.
pub fn pattern_entries<'de, D, T>(d: D, patterns: &[&str], index: usize) -> Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    let mut out = BTreeMap::new();
    for (name, value) in names(d)? {
        if !pattern_matches_dyn(patterns[index], &name) {
            continue;
        }
        let decoded: T = decode(&name, &value)?;
        if patterns.iter().position(|p| pattern_matches_dyn(p, &name)) == Some(index) {
            out.insert(name, decoded);
        }
    }
    Ok(out)
}

/// Reads, from the undeclared names of a struct, those that no pattern of `patterns` matches: each value decodes as
/// `T` (`additionalProperties`).
///
/// # Errors
/// Such a value does not decode as `T`.
pub fn other_entries<'de, D, T>(d: D, patterns: &[&str]) -> Result<BTreeMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    let mut out = BTreeMap::new();
    for (name, value) in names(d)? {
        if !patterns.iter().any(|p| pattern_matches_dyn(p, &name)) {
            let decoded = decode(&name, &value)?;
            out.insert(name, decoded);
        }
    }
    Ok(out)
}

/// Refuses, from the undeclared names of a struct, one that no pattern of `patterns` matches
/// (`additionalProperties: false`).
///
/// # Errors
/// Such a name is present.
pub fn no_other_entries<'de, D: serde::Deserializer<'de>>(d: D, patterns: &[&str]) -> Result<Closed, D::Error> {
    match names(d)?.into_iter().find(|(name, _)| !patterns.iter().any(|p| pattern_matches_dyn(p, name))) {
        Some((name, _)) => Err(serde::de::Error::custom(format!("`{name}` is not allowed"))),
        None => Ok(Closed),
    }
}

/// Returns the value of the undeclared name `name` decoded as `T`. Errors: it does not decode, with the name.
fn decode<'de, T: serde::Deserialize<'de>, E: serde::de::Error>(
    name: &str,
    value: &crate::tagged::Content<'de>,
) -> Result<T, E> {
    value.decode_ref().map_err(|e| E::custom(format!("`{name}`: {e}")))
}

/// The names and values of the object that `d` reads, each value buffered.
fn names<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<(String, crate::tagged::Content<'de>)>, D::Error> {
    struct Names;
    impl<'de> serde::de::Visitor<'de> for Names {
        type Value = Vec<(String, crate::tagged::Content<'de>)>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("an object")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut out = vec![];
            while let Some(entry) = map.next_entry()? {
                out.push(entry);
            }
            Ok(out)
        }
    }
    d.deserialize_map(Names)
}

/// A struct that refuses the names that none of its patterns matches: it holds none of them, and writes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Closed;

impl serde::Serialize for Closed {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serde::ser::SerializeMap::end(s.serialize_map(Some(0))?)
    }
}
