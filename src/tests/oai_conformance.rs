//! Official OAI schema test suite (branches v3.1-dev and v3.2-dev,
//! `tests/schema/{pass,fail}`), vendored under `tests/fixtures/oai-*`.
//!
//! - every `pass` document loads with no error diagnostics
//! - every `fail` document produces at least one error

use crate::diag::Level;

fn run(version: &str) {
    let root = super::common::fixtures().join(format!("oai-{version}"));
    let mut problems = vec![];
    let mut count = 0;
    for kind in ["pass", "fail"] {
        let mut files: Vec<_> = std::fs::read_dir(root.join(kind)).unwrap().map(|e| e.unwrap().path()).collect();
        files.sort();
        for path in files {
            count += 1;
            // The suite checks documents and not the URLs that they name. Nothing is fetched even with `remote-refs`.
            let loaded = crate::load::load_path_with(&path, crate::resolve::Base::offline(path.parent()), &[]);
            let errors: Vec<String> =
                loaded.diagnostics.iter().filter(|d| d.level == Level::Error).map(ToString::to_string).collect();
            let name = path.file_name().unwrap().to_string_lossy();
            match kind {
                "pass" if !errors.is_empty() => problems.push(format!("pass/{name} has errors: {errors:?}")),
                "fail" if errors.is_empty() => problems.push(format!("fail/{name} was accepted")),
                _ => {}
            }
        }
    }
    assert!(problems.is_empty(), "{} of {count} OAI {version} cases wrong:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn oai_v32_suite() {
    run("v32");
}

#[test]
fn oai_v31_suite() {
    run("v31");
}
