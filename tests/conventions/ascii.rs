//! Code, comments, docs and messages are plain ASCII. Tests and benches may quote other characters in string
//! literals as input. Fixtures are descriptions and are left out.

use std::fs;

use super::root;

/// Whether every character outside the ASCII range on `line` is inside a string literal.
fn only_in_strings(line: &str) -> bool {
    let (mut in_string, mut escaped) = (false, false);
    for c in line.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            c if !c.is_ascii() && !in_string => return false,
            _ => {}
        }
    }
    true
}

/// The `file:line` of each line of `text` with a character it may not hold.
fn offending(file: &str, text: &str, code: bool) -> Vec<String> {
    let mut in_tests = super::is_test_file(file);
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            in_tests |= code && line.trim() == "#[cfg(test)]";
            !(line.is_ascii() || in_tests && only_in_strings(line))
        })
        .map(|(i, _)| format!("{file}:{}", i + 1))
        .collect()
}

/// The Markdown and TOML files that a reader sees: the sources of the book, the READMEs and the manifests.
fn prose() -> Vec<(String, String)> {
    let mut files: Vec<String> =
        ["README.md", "CONTRIBUTING.md", "Cargo.toml", "runtime/README.md", "runtime/Cargo.toml", "clippy.toml"]
            .map(str::to_owned)
            .to_vec();
    let book = root().join("docs").join("book").join("src");
    for entry in fs::read_dir(&book).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            files.push(path.strip_prefix(root()).unwrap().display().to_string());
        }
    }
    files
        .into_iter()
        .map(|file| {
            let text = fs::read_to_string(file.split('/').fold(root(), |dir, c| dir.join(c))).unwrap();
            (file, text)
        })
        .collect()
}

#[test]
fn text_is_ascii() {
    let mut found: Vec<String> =
        super::all_sources().iter().flat_map(|(file, text)| offending(file, text, true)).collect();
    found.extend(prose().iter().flat_map(|(file, text)| offending(file, text, false)));
    assert!(found.is_empty(), "characters outside ASCII:\n{}", found.join("\n"));
}

#[test]
fn strings_in_tests_may_quote_any_character() {
    assert!(only_in_strings(r#"assert!(email("\u{c2e4}", true), "idn-email");"#));
    assert!(only_in_strings("let s = \"b\u{fc}cher\";"));
    assert!(!only_in_strings("// b\u{fc}cher"));
    assert!(!only_in_strings("let s = \"\\\"\"; // \u{fc}"));
    assert_eq!(offending("src/a.rs", "fn a() {}\n// \u{2014}\n", true), ["src/a.rs:2"]);
    let none: [String; 0] = [];
    assert_eq!(offending("src/tests/a.rs", "let s = \"\u{2014}\";\n", true), none);
    assert_eq!(offending("src/a.rs", "#[cfg(test)]\nlet s = \"\u{2014}\";\n", true), none);
    assert_eq!(offending("README.md", "\u{2014}\n", false), ["README.md:1"]);
}
