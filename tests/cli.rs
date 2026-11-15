//! The command line as users run it: exit codes and what each command prints.

#![expect(clippy::disallowed_methods, reason = "runs the binary and sets up its files")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cargo-honig")).args(args).output().unwrap()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("v32").join(name)
}

#[test]
fn version_prints_the_package_and_its_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    let expected = format!("{} {}\n", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn help_lists_every_command() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for command in ["init", "generate", "check", "validate", "upgrade", "bundle"] {
        assert!(help.contains(&format!("  {command} ")), "{command} missing from:\n{help}");
    }
}

#[test]
fn an_unknown_command_exits_with_2() {
    let out = run(&["frobnicate"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.starts_with("error: unrecognized subcommand 'frobnicate'"), "{err}");
}

#[test]
fn validate_accepts_a_valid_description() {
    let spec = fixture("shop.yaml");
    let out = run(&["validate", spec.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn bundle_prints_the_same_document_as_json_or_yaml() {
    let spec = fixture("shop.yaml");
    let bundle = |format: &str| {
        let out = run(&["bundle", spec.to_str().unwrap(), "--format", format]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let json: serde_json::Value = serde_json::from_str(&bundle("json")).unwrap();
    let yaml: serde_json::Value = serde_saphyr::from_str(&bundle("yaml")).unwrap();
    assert_eq!(json, yaml);
    assert_eq!(json["openapi"], "3.2.0");
}
