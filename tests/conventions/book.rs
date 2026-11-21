//! The Rust of the book is compiled and run. Every ```` ```rust ```` block of `docs/book/src` only includes an anchor
//! of a file that a test builds (`{{#include ../../../tests/e2e/tests/v32_shop.rs:client}}`). So an example cannot
//! drift from the code that it shows.

use std::fs;

use super::root;

/// The `file:line` of each Rust block in `text` that contains anything but `{{#include}}` lines.
fn hand_written(file: &str, text: &str) -> Vec<String> {
    let mut found = vec![];
    let mut block: Option<(usize, bool)> = None;
    for (i, line) in text.lines().enumerate() {
        match (&mut block, line.trim_start()) {
            (None, fence) if fence.starts_with("```rust") => block = Some((i + 1, false)),
            (Some((start, written)), "```") => {
                if *written {
                    found.push(format!("{file}:{start}"));
                }
                block = None;
            }
            (Some((_, written)), code) if !code.is_empty() && !code.starts_with("{{#include ") => *written = true,
            _ => {}
        }
    }
    found
}

#[test]
fn rust_in_the_book_comes_from_tested_files() {
    let book = root().join("docs").join("book").join("src");
    let mut found = vec![];
    for entry in fs::read_dir(&book).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            let file = path.strip_prefix(root()).unwrap().display().to_string();
            found.extend(hand_written(&file, &fs::read_to_string(&path).unwrap()));
        }
    }
    assert!(
        found.is_empty(),
        "Rust blocks written in the book, not included from a tested file:\n{}",
        found.join("\n")
    );
}

#[test]
fn only_includes_pass() {
    let page = "```rust\n{{#include a.rs:x}}\n```\n\n```rust\nlet x = 1;\n```\n\n```toml\nx = 1\n```\n";
    assert_eq!(hand_written("p.md", page), ["p.md:5"]);
}
