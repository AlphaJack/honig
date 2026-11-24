//! The code conventions in CONTRIBUTING that a test can check. They are parsed from the sources with `syn` in one
//! binary.

#![expect(clippy::disallowed_methods, reason = "parses the sources")]

use std::{
    fs,
    path::{Path, PathBuf},
};

mod ascii;
mod book;
mod duplicates;
mod errors;
mod expects;
mod literals;
mod names;
mod paths;
mod scaling;
mod sizes;
mod string_code;
mod systems;

/// The workspace's folder.
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The source folders of every package: `src`, `tests` and `benches` of the generator and of honig.
fn source_dirs() -> Vec<PathBuf> {
    let packages = [root(), root().join("honig")];
    packages.iter().flat_map(|p| [p.join("src"), p.join("tests"), p.join("benches")]).filter(|d| d.is_dir()).collect()
}

/// Each `.rs` file under `dir` with its path from the folder of the workspace. Fixtures are descriptions.
/// `tests/e2e/src` is what they generate. The clippy lints in its `lib.rs` check it.
fn sources(dir: &Path, found: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() && !path.ends_with("fixtures") && !path.ends_with(Path::new("e2e").join("src")) {
            sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            // With `/` on every OS: the conventions match prefixes such as `src/emit/`.
            let file = path.strip_prefix(root()).unwrap_or(&path).display().to_string().replace('\\', "/");
            found.push((file, fs::read_to_string(&path).unwrap()));
        }
    }
}

/// Every source file of the workspace as its path and its text.
fn all_sources() -> Vec<(String, String)> {
    let mut found = Vec::new();
    for dir in source_dirs() {
        sources(&dir, &mut found);
    }
    found
}

/// A test or a bench: a file below a `tests` or `benches` folder or a `tests.rs`. Tests may repeat themselves, name
/// numbers and fail with text.
fn is_test_file(file: &str) -> bool {
    file.ends_with("tests.rs") || file.split('/').any(|c| c == "tests" || c == "benches")
}

/// `#[test]` or `#[cfg(test)]`.
fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| match &a.meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) => list.path.is_ident("cfg") && list.tokens.to_string() == "test",
        syn::Meta::NameValue(_) => false,
    })
}
