//! Descriptions for integration tests built as JSON values and written with
//! `serde_json`. No description is assembled from strings.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// A minimal description with `paths` and `components` filled in.
pub(crate) fn spec(version: &str, paths: &Value, components: &Value) -> Value {
    json!({
        "openapi": version,
        "info": {"title": "t", "version": "1"},
        "paths": paths,
        "components": components,
    })
}

/// Write `doc` as `openapi.json` and each extra file as JSON into a
/// directory of its own. Return the path of the description.
pub(crate) fn write(test: &str, doc: &Value, files: &[(&str, &Value)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("honig-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (name, v) in files {
        std::fs::write(dir.join(name), serde_json::to_vec_pretty(v).expect("serializes")).expect("writes");
    }
    let path = dir.join("openapi.json");
    std::fs::write(&path, serde_json::to_vec_pretty(doc).expect("serializes")).expect("writes");
    path
}

/// The IR of `doc` (written as `test`) and its diagnostics as `pointer: message`.
pub(crate) fn build(test: &str, doc: &Value, files: &[(&str, &Value)]) -> (crate::ir::Api, Vec<String>) {
    let path = write(test, doc, files);
    let loaded = crate::load::load_path(&path);
    let mut diags = loaded.diagnostics.clone();
    let api = crate::ir::build(&loaded.doc().expect("loads"), &mut diags);
    let messages = diags.iter().map(|d| format!("{}: {}", d.pointer, d.message)).collect();
    (api, messages)
}

/// The generated files of `doc` (written as `test`).
pub(crate) fn generate(test: &str, doc: &Value, opts: &crate::emit::Options) -> Vec<(String, String)> {
    crate::generate::generate(&write(test, doc, &[]), opts).expect("generates").files
}

/// One generated file by name.
pub(crate) fn file<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
    files.iter().find(|(n, _)| n.ends_with(name)).map_or_else(|| panic!("no {name}"), |(_, t)| t.as_str())
}

/// Points the crate whose manifest is at `manifest` at the runtime of this checkout next to the pinned version.
/// Otherwise generated crates get the runtime from crates.io.
pub(crate) fn use_local_runtime(manifest: &Path) {
    let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(manifest).unwrap().parse().unwrap();
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    doc["dependencies"]["honig"]["path"] = toml_edit::value(runtime.display().to_string());
    std::fs::write(manifest, doc.to_string()).unwrap();
}

/// `tests/fixtures`: descriptions generated as the modules of `tests/e2e`.
pub(crate) fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

/// `Cargo.toml` for a standalone crate around the generated module.
fn crate_manifest(name: &str, spec_file: &str) -> String {
    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
doctest = false

[dependencies]
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
# Optional adapters: each feature also turns on the adapter of the runtime.
reqwest = {{ version = "0.13", optional = true, default-features = false, features = ["rustls"] }}
axum = {{ version = "0.8", optional = true, default-features = false, features = ["http1", "tokio"] }}

[features]
reqwest = ["dep:reqwest", "honig/reqwest"]
axum = ["dep:axum", "honig/axum"]

[package.metadata.honig.api]
spec = "{spec_file}"
output = "src/api"
"#
    )
}

/// Writes a crate in `dir` around the module generated from `spec`, which lies in `dir`. The fuzz test builds these.
pub(crate) fn new_crate(spec: &Path, dir: &Path, name: &str, opts: &crate::emit::Options) {
    let spec_file = spec.file_name().unwrap().to_str().unwrap();
    let manifest = crate_manifest(name, spec_file);
    let generated = crate::generate::generate(spec, opts).unwrap_or_else(|d| panic!("{name}: {:?}", d.0));
    let manifest = crate::manifest::with_runtime(&manifest, &generated.runtime_features).unwrap();
    std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src").join("lib.rs"), "pub mod api;\npub use api::*;\n").unwrap();
    crate::disk::write_module(&dir.join("src").join("api"), &generated.files).unwrap();
}
