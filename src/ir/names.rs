//! Names. Every name from the description becomes a [`Name`]: words each target language spells in its own case,
//! with its own keywords escaped.

use std::collections::BTreeSet;

/// A name as lowercase ASCII words. It never starts with a digit and its words never start or end with `_`. So
/// every target can spell it as an identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Name(Vec<String>);

impl Name {
    /// Creates the words of `s` split at non-alphanumerics and camelCase boundaries.
    #[must_use]
    pub(crate) fn new(s: &str) -> Self {
        let words: Vec<String> = special(s)
            .map_or_else(|| words(s).iter().map(|w| ascii_only(&w.to_lowercase())).collect(), |w| vec![w.to_owned()]);
        Self::of(words)
    }

    /// Returns these words made fit for an identifier. `_` is trimmed off the ends. None gives `value`. A leading digit
    /// gets an `n` before it.
    fn of(mut words: Vec<String>) -> Self {
        if let Some(first) = words.first_mut() {
            *first = first.trim_start_matches('_').to_owned();
        }
        if let Some(last) = words.last_mut() {
            *last = last.trim_end_matches('_').to_owned();
        }
        words.retain(|w| !w.is_empty());
        if words.is_empty() {
            words.push("value".to_owned());
        }
        if words[0].starts_with(|c: char| c.is_ascii_digit()) {
            words[0].insert(0, 'n');
        }
        Self(words)
    }

    /// Returns the words in lowercase.
    #[must_use]
    pub(crate) fn words(&self) -> &[String] {
        &self.0
    }

    /// This name followed by `more`'s words: a name for something inside this one.
    #[must_use]
    pub(crate) fn and(&self, more: &Self) -> Self {
        Self(self.0.iter().chain(&more.0).cloned().collect())
    }

    /// This name with `n` after its last word: the `n`th of a kind (`Item2`).
    #[must_use]
    pub(crate) fn numbered(&self, n: usize) -> Self {
        let mut words = self.0.clone();
        if let Some(last) = words.last_mut() {
            last.push_str(&n.to_string());
        }
        Self(words)
    }

    /// Returns the words after `prefix` when this name starts with all of its words and has more.
    #[must_use]
    pub(crate) fn after(&self, prefix: &Self) -> Option<Self> {
        let rest = self.0.strip_prefix(prefix.0.as_slice()).filter(|r| !r.is_empty())?;
        Some(Self(rest.to_vec()))
    }

    /// Returns whether this is `generic` followed by a number (`variant3`).
    #[must_use]
    pub(crate) fn is_numbered(&self, generic: &str) -> bool {
        matches!(self.0.as_slice(), [w] if w.strip_prefix(generic).is_some_and(crate::text::is_digits))
    }

    /// Returns whether this name is `word` alone.
    #[must_use]
    pub(crate) fn is(&self, word: &str) -> bool {
        matches!(self.0.as_slice(), [w] if w == word)
    }

    /// Returns the words that every name of `names` starts with and the rest of each. The cut stops when each keeps at
    /// least one.
    #[must_use]
    pub(crate) fn without_common_prefix(names: &[&Self]) -> Option<Vec<Self>> {
        let first = names.first()?;
        let shortest = names.iter().map(|n| n.0.len()).min()?.saturating_sub(1);
        let mut len = (0..shortest).take_while(|&i| names.iter().all(|n| n.0[i] == first.0[i])).count();
        // What is left must still start like a name.
        while len > 0 && names.iter().any(|n| n.0[len].starts_with(|c: char| c.is_ascii_digit())) {
            len -= 1;
        }
        (len > 0).then(|| names.iter().map(|n| Self(n.0[len..].to_vec())).collect())
    }

    /// Returns what two names that collide in the spelling of some target share. That is the words capitalized and run
    /// together, the spelling that keeps the least (`x_1` and `x1` are both `X1`).
    pub(crate) fn key(&self) -> String {
        self.0.iter().map(|w| capitalize(w)).collect()
    }
}

/// Splits into words at non-alphanumerics and camelCase boundaries.
pub(crate) fn words(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if let Some(&prev) = cur.chars().last().as_ref() {
            let next = chars.get(i + 1).copied();
            // A plural acronym keeps its `s`: `PIDs` is `pids` and not `pi` + `ds`.
            let plural = next == Some('s') && !chars.get(i + 2).copied().is_some_and(char::is_lowercase);
            let boundary = ((prev.is_lowercase() || prev.is_ascii_digit()) && c.is_uppercase())
                || (prev.is_uppercase() && c.is_uppercase() && next.is_some_and(char::is_lowercase) && !plural);
            if boundary {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn ascii_only(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
}

fn special(s: &str) -> Option<&'static str> {
    match s {
        "+1" => Some("plus1"),
        "-1" => Some("minus1"),
        "+" => Some("plus"),
        "-" => Some("minus"),
        "*" => Some("star"),
        "" => Some("empty"),
        _ => None,
    }
}

/// Returns `w` with its first letter uppercase.
pub(crate) fn capitalize(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Names a target language keeps for itself: no generated name uses one.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Reserved<'a> {
    /// Names types avoid.
    pub types: &'a [&'a str],
    /// Names operations avoid.
    pub operations: &'a [&'a str],
}

/// Returns the names that a target reserves as [`unique`] records them. It is `taken` to start from.
#[must_use]
pub(crate) fn reserved(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| Name::new(n).key()).collect()
}

/// Makes `name` unique within `taken` by appending 2, 3, ... to its last word. Then record it.
pub(crate) fn unique(name: Name, taken: &mut BTreeSet<String>) -> Name {
    let key = name.key();
    if taken.insert(key.clone()) {
        return name;
    }
    // The key of `name.numbered(i)` is the key of `name` and `i`: no word is capitalized again for each try.
    let mut i = 2;
    while !taken.insert(format!("{key}{i}")) {
        i += 1;
    }
    name.numbered(i)
}

/// Returns a unique name within `taken` for each `(name, wire)`, in the order given. Colliding names are numbered in
/// the sorted order of their wire keys, so a reorder of the group changes no name. A text key is `(length, text)`:
/// `name` keeps `Name` and `-name` gets `Name2`.
pub(crate) fn unique_sorted<W: Ord>(group: Vec<(Name, W)>, taken: &mut BTreeSet<String>) -> Vec<Name> {
    let mut order: Vec<usize> = (0..group.len()).collect();
    order.sort_by(|&a, &b| group.get(a).map(|g| &g.1).cmp(&group.get(b).map(|g| &g.1)));
    let mut names: Vec<Option<Name>> = vec![None; group.len()];
    let mut group: Vec<Option<Name>> = group.into_iter().map(|(n, _)| Some(n)).collect();
    for i in order {
        if let (Some(slot), Some(Some(name))) = (names.get_mut(i), group.get_mut(i).map(Option::take)) {
            *slot = Some(unique(name, taken));
        }
    }
    names.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(s: &str) -> Vec<String> {
        Name::new(s).words().to_vec()
    }

    #[test]
    fn words_of_names() {
        assert_eq!(of("orderId"), ["order", "id"]);
        assert_eq!(of("X-Rate-Limit"), ["x", "rate", "limit"]);
        assert_eq!(of("HTTPServer"), ["http", "server"]);
        assert_eq!(of("getAccount3PIDs"), ["get", "account3", "pids"]);
        assert_eq!(of("listURLsByHost"), ["list", "urls", "by", "host"]);
        assert_eq!(of("123abc"), ["n123abc"]);
        assert_eq!(of("+1"), ["plus1"]);
        assert_eq!(of("-1"), ["minus1"]);
        assert_eq!(of("+"), ["plus"]);
        assert_eq!(of("getAToB"), ["get", "a", "to", "b"]);
        assert_eq!(of("$ref"), ["ref"]);
        assert_eq!(of("h2Stream"), ["h2", "stream"]);
        assert_eq!(of("KindVariant2Variant2"), ["kind", "variant2", "variant2"]);
        assert_eq!(of(""), ["empty"]);
        assert_eq!(of("$$"), ["value"]);
    }

    /// The words that every name starts with are cut, as long as each keeps one that does not start with a digit.
    #[test]
    fn names_lose_their_common_prefix() {
        let cut = |names: &[&str]| {
            let names: Vec<Name> = names.iter().map(|n| Name::new(n)).collect();
            Name::without_common_prefix(&names.iter().collect::<Vec<_>>())
                .map(|n| n.iter().map(Name::key).collect::<Vec<_>>())
        };
        assert_eq!(cut(&["itemBook", "itemGadget"]), Some(vec!["Book".to_owned(), "Gadget".to_owned()]));
        assert_eq!(cut(&["item_1", "item_2"]), None);
    }

    #[test]
    fn names_unique_in_their_least_spelling() {
        let mut taken = BTreeSet::new();
        assert_eq!(unique(Name::new("x_1"), &mut taken).words(), ["x", "1"]);
        assert_eq!(unique(Name::new("x1"), &mut taken).words(), ["x12"]);
        assert_eq!(unique(Name::new("fooBar"), &mut taken).words(), ["foo", "bar"]);
        assert_eq!(unique(Name::new("foo_bar"), &mut taken).words(), ["foo", "bar2"]);
    }
}
