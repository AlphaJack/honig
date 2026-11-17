//! A browser client generated from a fixture and compiled for `wasm32`. `tests/regen.rs` keeps `tests/e2e` and
//! `examples/shop` fresh.

use std::path::Path;
use std::process::Command;

/// A browser client: client-only module with the `gloo-net` transport. It must
/// compile for `wasm32-unknown-unknown` (where futures are `!Send`). It must still
/// compile natively where the adapter is left out. It needs the wasm32 target:
/// `rustup target add wasm32-unknown-unknown`.
#[test]
#[ignore = "compiles generated crates"]
fn wasm_client() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = root.join("target").join("gen-tests").join("wasm-client");
    // A missing directory is fine: there is nothing to clean up.
    std::fs::remove_dir_all(&out).ok();
    let manifest = r#"[package]
name = "wasm-client"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[target.'cfg(target_arch = "wasm32")'.dependencies]
gloo-net = { version = "0.6", default-features = false, features = ["http"] }
"#;
    let opts = crate::emit::Options {
        server: false,
        tests: false,
        rust: crate::emit::rust::Options::default(),
        ..crate::emit::Options::default()
    };
    // Streams, OAuth flows and webhooks are the client paths that hold futures.
    let spec = root.join("tests").join("fixtures").join("v32").join("shop.yaml");
    let generated = crate::generate::generate(&spec, &opts).unwrap_or_else(|d| panic!("generation failed: {:?}", d.0));
    crate::disk::write_module(
        &out.join("src").join("api"),
        &out.join("src").join("api").join("mod.rs"),
        &generated.files,
    )
    .unwrap();
    std::fs::write(
        out.join("Cargo.toml"),
        crate::manifest::with_runtime(manifest, &[&generated.runtime_features[..], &["gloo-net"]].concat()).unwrap(),
    )
    .unwrap();
    super::common::use_local_runtime(&out.join("Cargo.toml"));
    // `deny(warnings)`: a stray `+ Send` that refines the trait shows up as a lint.
    std::fs::write(
        out.join("src").join("lib.rs"),
        "#![deny(warnings)]\npub mod api;\n#[cfg(target_arch = \"wasm32\")]\npub fn client() -> api::Client<api::runtime::GlooTransport> {\n    api::Client::new(api::runtime::GlooTransport::new(\"/api\"))\n}\n",
    )
    .unwrap();
    for target in [Some("wasm32-unknown-unknown"), None] {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["check", "--quiet"])
            .current_dir(&out)
            .env("CARGO_TARGET_DIR", root.join("target").join("gen-tests").join("target"));
        if let Some(t) = target {
            cmd.args(["--target", t]);
        }
        let status = cmd.status().expect("cargo runs");
        assert!(status.success(), "wasm client does not compile for {}", target.unwrap_or("the host"));
    }
}
