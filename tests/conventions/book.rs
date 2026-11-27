//! The Rust of the book is compiled and run. Every ```` ```rust ```` block of `docs/book/src` only includes an anchor
//! of a file that a test builds (`{{#include ../../../tests/e2e/tests/v32_shop.rs:client}}`). So an example cannot
//! drift from the code that it shows. It comes from the running example, the catalog of `examples/shop`, so the
//! examples of every page call one API.

use std::fs;

use super::root;

/// The target of an `{{#include}}` line, or of a `{{#rustdoc_include}}` line that hides the rest of its file: `a.rs:x`.
fn included(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("{{#include ").or_else(|| line.strip_prefix("{{#rustdoc_include "))?;
    rest.strip_suffix("}}")
}

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
            (Some((_, written)), code) if !code.is_empty() && included(code).is_none() => *written = true,
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
        let Some(target) = included(line).and_then(|t| t.strip_prefix("../../../")) else {
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
                {{#include ../../../tests/fixtures/v32/shop.yaml:x}}\n```\n\n```rust\n\
                {{#rustdoc_include ../../../tests/e2e/tests/v32_shop/client.rs:feed}}\n```\n";
    let found =
        ["p.md:6: tests/e2e/tests/v32_shop/client.rs:media", "p.md:14: tests/e2e/tests/v32_shop/client.rs:feed"];
    assert_eq!(elsewhere("p.md", page), found);
}

#[test]
fn only_includes_pass() {
    let page = "```rust\n{{#include a.rs:x}}\n```\n\n```rust\nlet x = 1;\n```\n\n```toml\nx = 1\n```\n\n\
                ```rust\n{{#rustdoc_include a.rs:y}}\n```\n";
    assert_eq!(hand_written("p.md", page), ["p.md:5"]);
}

/// The text of every page of the book and of the two READMEs that a person writes, by its path from the root.
fn written_pages() -> Vec<(String, String)> {
    let book = root().join("docs").join("book").join("src");
    let mut pages: Vec<std::path::PathBuf> = fs::read_dir(&book)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "md"))
        .collect();
    pages.extend([root().join("README.md"), root().join("honig").join("README.md")]);
    pages
        .into_iter()
        .map(|p| (p.strip_prefix(root()).unwrap().display().to_string(), fs::read_to_string(&p).unwrap()))
        .collect()
}

/// The text of `path` from the root.
fn source(path: &str) -> String {
    fs::read_to_string(root().join(path)).unwrap()
}

/// The version that a manifest of the workspace asks of `krate`, as written: `0.33`, `=1.0.0-alpha.11`.
fn required(krate: &str) -> Option<String> {
    let manifests = ["Cargo.toml", "honig/Cargo.toml", "tests/e2e/Cargo.toml", "examples/shop/Cargo.toml"];
    manifests.iter().find_map(|m| {
        source(m).lines().find_map(|line| {
            let rest = line.strip_prefix(krate)?.trim_start().strip_prefix('=')?.trim_start();
            let rest = rest.strip_prefix('"').or_else(|| rest.split("version = \"").nth(1))?;
            rest.split('"').next().map(str::to_owned)
        })
    })
}

/// Each `` `crate` version `` in `text`, such as `` `opentelemetry` 0.33 ``.
fn versions(text: &str) -> Vec<(String, String)> {
    let mut found = vec![];
    for (i, _) in text.match_indices("` ") {
        let (Some(before), Some(after)) = (text.get(..i), text.get(i + 2..)) else { continue };
        let Some(krate) = before.rfind('`').and_then(|open| before.get(open + 1..)) else { continue };
        let version: String =
            after.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '=' | '-')).collect();
        let version = version.trim_end_matches(['.', '-']);
        let is_version =
            version.trim_start_matches('=').starts_with(|c: char| c.is_ascii_digit()) && version.contains('.');
        if is_version && krate.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') {
            found.push((krate.to_owned(), version.to_owned()));
        }
    }
    found
}

/// A crate version in the docs is the one that the workspace asks for: a bump that changes behavior shows on the page.
#[test]
fn versions_in_the_docs_are_the_ones_the_workspace_asks_for() {
    let mut wrong = vec![];
    for (page, text) in written_pages() {
        for (krate, version) in versions(&text) {
            let asked = required(&krate);
            if asked.as_deref() != Some(version.as_str()) {
                wrong.push(format!("{page}: `{krate}` {version}, the workspace asks for {asked:?}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn a_version_follows_its_crate_name() {
    let text = "built against `opentelemetry` 0.33. Only `http-cache-reqwest` =1.0.0-alpha.11, and `serde` derives.";
    let found = versions(text);
    assert_eq!(
        found,
        [("opentelemetry".into(), "0.33".into()), ("http-cache-reqwest".into(), "=1.0.0-alpha.11".into())]
    );
}

/// `256` as `256 B`, `16 << 20` as `16 MiB`.
fn size(expr: &str) -> String {
    let (n, shift) = expr.split_once(" << ").map_or((expr, 0), |(n, s)| (n, s.parse().unwrap()));
    let unit = match shift {
        0 => "B",
        10 => "KiB",
        20 => "MiB",
        other => panic!("a shift of {other}"),
    };
    format!("{} {unit}", n.trim())
}

/// The sizes and the steepest growth that the performance page gives are the ones of the benches.
#[test]
fn the_bench_limits_on_the_performance_page_are_the_benches_ones() {
    let page = source("docs/book/src/performance.md");
    let simd = source("honig/benches/simd.rs");
    let sizes = simd.lines().find_map(|l| l.strip_prefix("const SIZES: ")).unwrap();
    let list = sizes.split_once("= [").unwrap().1.trim_end_matches("];");
    let all: Vec<&str> = list.split(", ").collect();
    let range = format!("from {} to {}", size(all[0]), size(all[all.len() - 1]));
    assert!(page.contains(&range), "the page should say {range}");
    let callgrind = source("tests/bench-support/src/callgrind.rs");
    let exponent = callgrind.lines().find_map(|l| l.strip_prefix("const MAX_EXPONENT: f64 = ")).unwrap();
    let steepest = format!("n^{}", exponent.trim_end_matches(';'));
    assert!(page.contains(&steepest), "the page should say {steepest}");
}

/// The Overlay versions that the settings page names are the ones that `overlay::apply` accepts.
#[test]
fn the_overlay_versions_on_the_settings_page_are_the_accepted_ones() {
    let overlay = source("src/overlay.rs");
    let list = overlay.lines().find_map(|l| l.trim().strip_prefix("if ![")).unwrap();
    let all: Vec<&str> =
        list.split("].iter()").next().unwrap().split(", ").map(|v| v.trim_matches(['"', '.'])).collect();
    let range = format!("Overlay {} to {}", all[0], all[all.len() - 1]);
    assert!(source("docs/book/src/settings.md").contains(&range), "the settings page should say {range}");
}

/// Each `api::` or `honig::` path in the docs, such as `api::Nullable::Absent`, without generics or arguments.
fn paths(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .filter(|code| code.starts_with("api::") || code.starts_with("honig::"))
        .map(|code| code.split(['<', '(', ' ', '!']).next().unwrap_or_default().trim_end_matches(':').to_owned())
        .collect()
}

/// Each name in an `api::` or `honig::` path of the docs exists. A member such as `Absent` in `api::Nullable::Absent`
/// is a word of the file that defines `Nullable`, and any other name a word of honig's source or of a generated
/// module. So a rename such as `Response::Success` to `Response::Successful` cannot leave the old name on a page.
#[test]
fn names_in_the_docs_exist() {
    let files: Vec<String> = ["honig/src", "tests/e2e/src", "examples/shop/src/api"]
        .iter()
        .flat_map(|dir| walk(&root().join(dir)))
        .map(|f| fs::read_to_string(f).unwrap())
        .collect();
    let words = |text: &str| -> std::collections::BTreeSet<String> {
        text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map(str::to_owned).collect()
    };
    let everywhere = words(&files.concat());
    let defining = |ty: &str| {
        let defines = |file: &str, kind: &str| {
            let item = format!("pub {kind} {ty}");
            file.match_indices(&item).any(|(at, _)| {
                file.get(at + item.len()..)
                    .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
            })
        };
        files.iter().find(|f| ["enum", "struct", "trait"].iter().any(|k| defines(f, k)))
    };
    let mut missing = vec![];
    for (page, text) in written_pages() {
        for path in paths(&text) {
            let names: Vec<&str> = path.split("::").skip(1).collect();
            for (i, name) in names.iter().enumerate() {
                let known = match i.checked_sub(1).and_then(|p| defining(names[p])) {
                    Some(file) => words(file).contains(*name),
                    None => everywhere.contains(*name),
                };
                if !known {
                    missing.push(format!("{page}: `{path}` has no `{name}`"));
                }
            }
        }
    }
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

/// The `.rs` files under `dir`.
fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = vec![];
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    files
}

#[test]
fn a_path_drops_its_generics_and_arguments() {
    let text = "is `api::Nullable<T>`, `api::Nullable::Value(T)`, `honig::object!` and `cargo add`.";
    assert_eq!(paths(text), ["api::Nullable", "api::Nullable::Value", "honig::object"]);
}
