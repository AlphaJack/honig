//! The name of the package is written once in its `Cargo.toml`. Code takes it from `env!("CARGO_PKG_NAME")`. Each
//! binary sees the library through one `extern crate <name> as generator;`.

use std::fs;

use super::root;

/// The package name of the generator from its manifest.
fn package_name() -> String {
    let manifest: toml_edit::DocumentMut = fs::read_to_string(root().join("Cargo.toml")).unwrap().parse().unwrap();
    manifest["package"]["name"].as_str().unwrap().to_owned()
}

/// The lines of `source` that write `name`. Except the one that renames the library and `CARGO_BIN_EXE_<name>`. cargo
/// only offers that one under the literal name of the binary.
fn written(source: &str, name: &str) -> Vec<usize> {
    let alias = format!("extern crate {name} as generator;");
    let binary = format!("env!(\"CARGO_BIN_EXE_{name}\")");
    let names = |l: &str| l.replace(&binary, "").contains(name);
    source.lines().enumerate().filter(|(_, l)| names(l) && l.trim() != alias).map(|(i, _)| i + 1).collect()
}

#[test]
fn the_package_name_is_written_once() {
    let name = package_name();
    let sources = super::all_sources();
    let found: Vec<String> = sources
        .iter()
        .flat_map(|(file, source)| written(source, &name).into_iter().map(move |line| format!("{file}:{line}")))
        .collect();
    assert!(found.is_empty(), "`{name}` written out, instead of taken from the package:\n{}", found.join("\n"));
}

#[test]
fn only_the_library_alias_may_name_the_package() {
    let source = "extern crate tool as generator;\nlet x = \"tool-runtime\";\n// tool\nenv!(\"CARGO_BIN_EXE_tool\")\n";
    assert_eq!(written(source, "tool"), [2, 3]);
}
