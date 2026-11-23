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

/// A crate of its own under the target directory: a `Cargo.toml`, an empty `src/lib.rs` and the `shop` description.
fn scratch_crate(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nedition = \"2024\"\n")).unwrap();
    std::fs::write(dir.join("src").join("lib.rs"), "").unwrap();
    std::fs::copy(fixture("shop.yaml"), dir.join("openapi.yaml")).unwrap();
    dir
}

/// The runtime features that the code generated from the `shop` description needs.
const SHOP: &[&str] = &["client", "server", "pkce", "oauth", "cookie", "base64", "idn", "multipart", "xml"];

/// Adds the runtime of this checkout to the crate in `dir` with `features`, as `cargo add` would.
fn add_runtime(dir: &Path, features: &[&str]) {
    let manifest = dir.join("Cargo.toml");
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let features: Vec<String> = features.iter().map(|f| format!("\"{f}\"")).collect();
    let line = format!(
        "\n[dependencies]\nhonig = {{ version = \"={}\", path = {:?}, features = [{}] }}\n",
        env!("CARGO_PKG_VERSION"),
        runtime.display().to_string(),
        features.join(", ")
    );
    std::fs::write(&manifest, std::fs::read_to_string(&manifest).unwrap() + &line).unwrap();
}

fn in_crate(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let manifest = dir.join("Cargo.toml");
    let out = run(&[args, &["--manifest-path", manifest.to_str().unwrap()]].concat());
    (out.status.code(), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn init_then_generate_writes_a_module_that_check_finds_up_to_date() {
    let dir = scratch_crate("cli_init");
    let (code, out, err) = in_crate(&dir, &["init", "openapi.yaml"]);
    assert_eq!(code, Some(0), "{err}");
    let version = env!("CARGO_PKG_VERSION");
    for line in [
        "added [package.metadata.honig.api]".to_owned(),
        format!("next: cargo add honig@={version} -F client,server"),
        "next: cargo add serde -F derive".to_owned(),
        "next: cargo add serde_json".to_owned(),
        "next: cargo honig generate".to_owned(),
    ] {
        assert!(out.contains(&line), "{line} missing from:\n{out}");
    }
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("[package.metadata.honig.api]\ndescriptions = [\"openapi.yaml\"]"), "{manifest}");
    assert!(!manifest.contains("[dependencies]"), "init adds no dependency:\n{manifest}");

    let (code, _, err) = in_crate(&dir, &["init", "openapi.yaml"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("already has [package.metadata.honig]"), "{err}");

    // The description needs more features than `init` can know.
    let (code, _, err) = in_crate(&dir, &["generate"]);
    assert_eq!(code, Some(2));
    assert!(err.contains(&format!("is missing. Run `cargo add honig@={version} -F client,server,")), "{err}");
    add_runtime(&dir, SHOP);

    let (code, _, err) = in_crate(&dir, &["check"]);
    assert_eq!(code, Some(1), "check passed before generate");
    assert!(err.contains("stale generated code in") && err.contains("run `cargo honig generate`"), "{err}");

    let (code, out, err) = in_crate(&dir, &["generate"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("generated ") && out.contains(" operations, "), "{out}");
    assert!(err.contains("hint: add `pub mod api;` to src/lib.rs"), "{err}");
    assert!(dir.join("src").join("api").join("mod.rs").is_file(), "generate wrote no module");

    let (code, out, err) = in_crate(&dir, &["check"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.contains("up to date: "), "{out}");
}

#[test]
fn quiet_prints_no_progress_and_no_hints() {
    let dir = scratch_crate("cli_quiet");
    assert_eq!(in_crate(&dir, &["init", "openapi.yaml", "--quiet"]), (Some(0), String::new(), String::new()));
    add_runtime(&dir, SHOP);
    assert_eq!(in_crate(&dir, &["generate", "--quiet"]), (Some(0), String::new(), String::new()));
    assert_eq!(in_crate(&dir, &["check", "--quiet"]), (Some(0), String::new(), String::new()));
}

#[test]
fn check_finds_a_module_file_edited_after_generate() {
    let dir = scratch_crate("cli_edited");
    assert_eq!(in_crate(&dir, &["init", "openapi.yaml"]).0, Some(0));
    add_runtime(&dir, SHOP);
    assert_eq!(in_crate(&dir, &["generate"]).0, Some(0));
    let types = dir.join("src").join("api").join("types.rs");
    let text = std::fs::read_to_string(&types).unwrap();
    std::fs::write(&types, text + "\npub struct Extra;\n").unwrap();
    let (code, _, err) = in_crate(&dir, &["check", "api"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("types.rs"), "{err}");
}

#[test]
fn validate_takes_a_module_or_every_module_of_the_manifest() {
    let dir = scratch_crate("cli_validate");
    assert_eq!(in_crate(&dir, &["init", "openapi.yaml"]).0, Some(0));
    for args in [&["validate"][..], &["validate", "api"]] {
        let (code, out, err) = in_crate(&dir, args);
        assert_eq!(code, Some(0), "{args:?}: {err}");
        assert!(out.starts_with("ok: ") && out.trim_end().ends_with("openapi.yaml"), "{args:?}: {out}");
    }
    std::fs::write(dir.join("openapi.yaml"), "openapi: 3.2.0\npaths: {}\n").unwrap();
    let (code, out, err) = in_crate(&dir, &["validate"]);
    assert_eq!(code, Some(1), "an invalid description passed: {out}");
    assert!(err.contains("error"), "{err}");
}

#[test]
fn a_workspace_generates_its_members_and_names_a_missing_one() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli_workspace");
    std::fs::remove_dir_all(&root).ok();
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("Cargo.toml"), "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n").unwrap();
    let member = scratch_crate("cli_workspace_member");
    std::fs::rename(&member, root.join("member")).unwrap();
    let member = root.join("member");
    assert_eq!(in_crate(&member, &["init", "openapi.yaml"]).0, Some(0));
    add_runtime(&member, SHOP);

    let (code, _, err) = in_crate(&root, &["generate", "--package", "nope"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("has no member `nope` with [package.metadata.honig.*]"), "{err}");
    let (code, _, err) = in_crate(&root, &["generate", "other"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("has no member with the module `other`"), "{err}");

    assert_eq!(in_crate(&root, &["check"]).0, Some(1));
    let (code, _, err) = in_crate(&root, &["generate", "--package", "cli_workspace_member"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(member.join("src").join("api").join("mod.rs").is_file());
    assert_eq!(in_crate(&root, &["check", "api"]).0, Some(0));
}

/// A description with a warning: a link to an operation that no file of it describes.
const WITH_WARNING: &str = "openapi: 3.2.0
info: {title: t, version: \"1\"}
paths:
  /a:
    get:
      operationId: one
      responses: {\"200\": {description: ok, links: {next: {operationId: elsewhere}}}}
";

/// Two path templates that differ only in their parameter names: an error in the description.
const SAME_TEMPLATE: &str = "openapi: 3.2.0
info: {title: t, version: \"1\"}
paths:
  /a/{x}:
    parameters: [{name: x, in: path, required: true, schema: {type: string}}]
    get: {operationId: one, responses: {\"204\": {description: ok}}}
  /a/{y}:
    parameters: [{name: y, in: path, required: true, schema: {type: string}}]
    get: {operationId: two, responses: {\"204\": {description: ok}}}
";

#[test]
fn generate_reports_errors_warnings_and_hints_and_never_edits_the_manifest() {
    let dir = scratch_crate("cli_hints");
    std::fs::write(dir.join("openapi.yaml"), WITH_WARNING).unwrap();
    std::fs::write(dir.join("src").join("lib.rs"), "pub mod api;\n").unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"cli_hints\"\nedition = \"2024\"\n\n[package.metadata.honig.api]\ndescriptions = [\"openapi.yaml\"]\n",
    )
    .unwrap();
    add_runtime(&dir, &["client"]);
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    // A feature that the code needs is missing: nothing is written, and the error says the line that adds it.
    let (code, _, err) = in_crate(&dir, &["generate"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("lacks the features that generated code needs. Run `cargo add honig -F server`"), "{err}");
    assert!(!dir.join("src").join("api").exists(), "a module was written");
    std::fs::write(dir.join("Cargo.toml"), manifest.replace("[\"client\"]", "[\"client\", \"server\"]")).unwrap();
    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();

    let (code, _, err) = in_crate(&dir, &["generate"]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join("Cargo.toml")).unwrap(), manifest, "generate edited Cargo.toml");
    for line in [
        "no operation has operationId `elsewhere`",
        "hint: run `cargo add serde -F derive`",
        "hint: run `cargo add serde_json`",
    ] {
        assert!(err.contains(line), "{line} missing from:\n{err}");
    }
    assert!(!err.contains("pub mod api"), "src/lib.rs declares the module:\n{err}");

    let (code, out, err) = in_crate(&dir, &["generate", "--quiet"]);
    assert_eq!(code, Some(0), "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(!err.contains("warning:") && !err.contains("hint:"), "{err}");

    // An error in the description fails `generate` and leaves the module as it was.
    let module = std::fs::read_to_string(dir.join("src").join("api").join("operations.rs")).unwrap();
    std::fs::write(dir.join("openapi.yaml"), SAME_TEMPLATE).unwrap();
    let (code, _, err) = in_crate(&dir, &["generate"]);
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("are the same templated path"), "{err}");
    assert_eq!(std::fs::read_to_string(dir.join("src").join("api").join("operations.rs")).unwrap(), module);
}

#[test]
fn generate_names_a_package_or_output_that_does_not_fit() {
    let dir = scratch_crate("cli_misfit");
    assert_eq!(in_crate(&dir, &["init", "openapi.yaml"]).0, Some(0));
    add_runtime(&dir, SHOP);
    let (code, _, err) = in_crate(&dir, &["generate", "--package", "other"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("is `cli_misfit`, not `other`"), "{err}");
    assert_eq!(in_crate(&dir, &["check", "--package", "cli_misfit"]).0, Some(1), "its own name is fine");

    let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let twice =
        manifest + "\n[package.metadata.honig.again]\ndescriptions = [\"openapi.yaml\"]\noutput = \"src/api\"\n";
    std::fs::write(dir.join("Cargo.toml"), twice).unwrap();
    let (code, _, err) = in_crate(&dir, &["generate"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("Give each module its own `output`"), "{err}");
}
