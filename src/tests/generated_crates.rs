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
    crate::disk::write_module(&out.join("src").join("api"), &generated.files).unwrap();
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

/// The date and time formats with `jiff`: a `time` keeps its offset and its leap second in `runtime::FullTime`.
#[test]
#[ignore = "compiles generated crates"]
fn jiff_dates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = root.join("target").join("gen-tests").join("jiff-dates");
    // A missing directory is fine: there is nothing to clean up.
    std::fs::remove_dir_all(&out).ok();
    let manifest = r#"[package]
name = "jiff-dates"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
jiff = { version = "1", features = ["serde"] }
"#;
    let doc = super::common::spec(
        "3.2.0",
        &serde_json::json!({}),
        &serde_json::json!({"schemas": {"Shift": {
            "type": "object",
            "required": ["day", "opens", "starts", "local", "ends"],
            "properties": {
                "day": {"type": "string", "format": "date"},
                "opens": {"type": "string", "format": "time"},
                "starts": {"type": "string", "format": "date-time"},
                "local": {"type": "string", "format": "date-time-local"},
                "ends": {"type": "string", "format": "time-local"},
            },
        }}}),
    );
    let libs = crate::settings::FormatLibs { dates: Some(crate::settings::DateLib::Jiff), ..Default::default() };
    let opts = crate::emit::Options {
        client: false,
        server: false,
        tests: false,
        rust: crate::emit::rust::Options { libs, ..Default::default() },
        ..crate::emit::Options::default()
    };
    let spec = super::common::write("jiff-dates", &doc, &[]);
    let generated = crate::generate::generate(&spec, &opts).unwrap_or_else(|d| panic!("generation failed: {:?}", d.0));
    assert!(generated.runtime_features.contains(&"jiff"), "{:?}", generated.runtime_features);
    crate::disk::write_module(&out.join("src").join("api"), &generated.files).unwrap();
    std::fs::write(
        out.join("Cargo.toml"),
        crate::manifest::with_runtime(manifest, &generated.runtime_features).unwrap(),
    )
    .unwrap();
    super::common::use_local_runtime(&out.join("Cargo.toml"));
    std::fs::write(
        out.join("src").join("lib.rs"),
        r##"#![deny(warnings)]
pub mod api;

#[test]
fn a_shift_keeps_every_offset_as_sent() {
    let text = r#"{"day":"2024-02-29","opens":"23:59:60Z","starts":"2024-02-29T08:30:06+02:00","local":"2024-02-29T08:30:06","ends":"17:00:00"}"#;
    let shift: api::types::Shift = serde_json::from_str(text).unwrap();
    assert!(shift.opens.leap_second);
    assert_eq!(shift.starts, "2024-02-29T06:30:06Z".parse::<jiff::Timestamp>().unwrap());
    assert_eq!(serde_json::to_value(&shift).unwrap()["opens"], "23:59:60Z");
    assert!(serde_json::from_str::<api::types::Shift>(&text.replace("23:59:60Z", "10:00:00")).is_err());
}
"##,
    )
    .unwrap();
    let status = Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", root.join("target").join("gen-tests").join("target"))
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the jiff crate does not build or its test fails");
}
