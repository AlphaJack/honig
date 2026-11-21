//! Checks on text the passes share.

/// Rust keywords, strict and reserved: no identifier takes one of them as it is.
pub(crate) const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self", "Self",
    "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "abstract", "become",
    "box", "do", "final", "gen", "macro", "override", "priv", "try", "typeof", "unsized", "virtual", "yield", "union",
];

/// Returns whether `s` is one or more ASCII digits.
pub(crate) fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Returns whether `s` is a Rust identifier: a letter or `_` and then letters, digits or `_`.
pub(crate) fn is_identifier(s: &str) -> bool {
    s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_identifier_starts_with_a_letter_or_an_underscore() {
        assert!(super::is_identifier("_a1") && super::is_identifier("a_b"));
        assert!(!super::is_identifier("1a") && !super::is_identifier("a-b") && !super::is_identifier(""));
    }
}
