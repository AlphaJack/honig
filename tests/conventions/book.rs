//! The Rust of the book is compiled and run. Every ```` ```rust ```` block of `docs/book/src` only includes an anchor
//! of a file that a test builds (`{{#include ../../../tests/e2e/tests/v32_shop.rs:client}}`). So an example cannot
//! drift from the code that it shows. It comes from the running example, the catalog of `examples/shop`, so the
//! examples of every page call one API.

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

/// Where the Rust of the running example is: the shop itself and the tests of its `catalog` fixture.
const RUNNING_EXAMPLE: [&str; 2] = ["examples/shop/", "tests/e2e/tests/v32_catalog/"];

/// The Rust includes that call another API, each with why.
const OTHER_APIS: [(&str, &str); 6] = [
    ("tests/e2e/tests/v32_shop/client.rs:device-flow", "the catalog accepts only an API key"),
    ("tests/e2e/tests/v32_scopes.rs:problem-instance", "the error body of the catalog is its own, not RFC 9457"),
    ("tests/e2e/tests/v32_scopes.rs:scopes", "the API key of the catalog grants no scopes"),
    ("tests/e2e/tests/v32_security.rs:checkout", "the catalog has one API key, not two credentials at once"),
    ("tests/e2e/tests/v32_security.rs:optional-rate-limit", "every operation of the catalog needs its key"),
    ("tests/e2e/tests/v32_kitchen_sink.rs:whole-schema", "the catalog has no keyword that a Rust type cannot say"),
];

/// The `file:line: target` of each Rust include in `text` that is neither from the running example nor listed in
/// [`OTHER_APIS`].
fn elsewhere(file: &str, text: &str) -> Vec<String> {
    let mut found = vec![];
    let mut rust = false;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_start();
        if line.starts_with("```") {
            rust = line.starts_with("```rust");
            continue;
        }
        let Some(target) = line.strip_prefix("{{#include ../../../").and_then(|t| t.strip_suffix("}}")) else {
            continue;
        };
        let known = RUNNING_EXAMPLE.iter().any(|dir| target.starts_with(dir))
            || OTHER_APIS.iter().any(|(other, _)| *other == target);
        if rust && !known {
            found.push(format!("{file}:{}: {target}", i + 1));
        }
    }
    found
}

#[test]
fn rust_in_the_book_calls_the_running_example() {
    let book = root().join("docs").join("book").join("src");
    let mut found = vec![];
    for entry in fs::read_dir(&book).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            let file = path.strip_prefix(root()).unwrap().display().to_string();
            found.extend(elsewhere(&file, &fs::read_to_string(&path).unwrap()));
        }
    }
    assert!(found.is_empty(), "Rust in the book that calls another API than the catalog:\n{}", found.join("\n"));
}

#[test]
fn only_the_running_example_passes() {
    let page = "```rust\n{{#include ../../../examples/shop/src/lib.rs:catalog}}\n```\n\n```rust\n\
                {{#include ../../../tests/e2e/tests/v32_shop/client.rs:media}}\n```\n\n```yaml\n\
                {{#include ../../../tests/fixtures/v32/shop.yaml:x}}\n```\n";
    assert_eq!(elsewhere("p.md", page), ["p.md:6: tests/e2e/tests/v32_shop/client.rs:media"]);
}

#[test]
fn only_includes_pass() {
    let page = "```rust\n{{#include a.rs:x}}\n```\n\n```rust\nlet x = 1;\n```\n\n```toml\nx = 1\n```\n";
    assert_eq!(hand_written("p.md", page), ["p.md:5"]);
}
