//! What `tests/e2e` contains: every fixture description of this repository generated as a module, and the manifest
//! that compiles them. `tests/regen.rs` compares the crate with it and rewrites what is stale. It lives in the library
//! so that a change to the generator rebuilds the library and one small test, not every unit test.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// Lints that the generated code passes on top of the defaults of clippy. That is pedantic and the nursery and
/// restriction lints that a strict crate turns on. The generated `lib.rs` warns on them. So the clippy run of the
/// workspace checks them.
const LINTS: &[&str] = &[
    "clippy::pedantic",
    "clippy::allow_attributes",
    "clippy::allow_attributes_without_reason",
    "clippy::clone_on_ref_ptr",
    "clippy::indexing_slicing",
    "clippy::let_underscore_untyped",
    "clippy::iter_on_empty_collections",
    "clippy::iter_on_single_items",
    "clippy::map_with_unused_argument_over_ranges",
    "clippy::missing_const_for_fn",
    "clippy::too_long_first_doc_paragraph",
    "clippy::use_self",
    "clippy::wildcard_enum_match_arm",
];

/// Returns every fixture description under `root` (`<version>/<name>.yaml`) as (module, spec) in path order.
///
/// # Errors
/// A folder cannot be listed.
pub fn fixtures(root: &Path) -> Result<Vec<(String, PathBuf)>, Error> {
    let mut out = vec![];
    for dir in crate::disk::dirs_in(root)? {
        let Some(version) = dir.file_name().and_then(|n| n.to_str()).filter(|n| n.starts_with('v')) else { continue };
        for spec in crate::disk::files_in(&dir)? {
            let Some(name) =
                spec.file_stem().and_then(|n| n.to_str()).filter(|_| spec.extension().is_some_and(|e| e == "yaml"))
            else {
                continue;
            };
            out.push((format!("{version}_{}", name.replace('-', "_")), spec));
        }
    }
    out.sort();
    Ok(out)
}

/// Returns what the crate at `e2e` must contain for the descriptions under `root`, as (path relative to it, contents).
///
/// # Errors
/// A file cannot be opened, a fixture does not generate or the `honig` dependency of the crate lacks a feature that
/// the fixtures need.
pub fn e2e_files(e2e: &Path, root: &Path) -> Result<Vec<(PathBuf, String)>, Error> {
    let manifest = crate::settings::Manifest::load(&e2e.join("Cargo.toml"))?;
    // The type features of the crate's `honig` dependency pick the types, as in the crate of a user.
    let libs = crate::settings::feature_libs(&manifest);
    let opts = crate::emit::Options {
        rust: crate::emit::rust::Options { libs, ..Default::default() },
        tests: true,
        ..crate::emit::Options::default()
    };
    let mut files = vec![];
    let mut lib = String::from(
        "//! Every fixture description in `tests/fixtures/<version>/<name>.yaml`, as `<version>_<name>::api`.\n",
    );
    for lint in LINTS {
        lib.push_str(&format!("#![warn({lint})]\n"));
    }
    let mut features = vec![];
    for (module, spec) in fixtures(root)? {
        let sidecar = sidecar(&spec)?;
        let mut libs = opts.rust.libs.clone();
        libs.named.extend(named_formats(sidecar.as_ref())?);
        let opts = crate::emit::Options {
            strict_client: strict_client(sidecar.as_ref()),
            rust: crate::emit::rust::Options { libs, ..opts.rust.clone() },
            ..opts.clone()
        };
        let generated = crate::generate::generate(&spec, &opts)
            .map_err(|problems| Error::Fixture { spec: spec.clone(), problems })?;
        let api = Path::new("src").join(&module).join("api");
        files.extend(generated.files.into_iter().map(|(name, text)| (api.join(name), text)));
        features.extend(generated.runtime_features);
        lib.push_str(&format!("pub mod {module} {{\n    pub mod api;\n}}\n"));
    }
    files.push((Path::new("src").join("lib.rs"), lib));
    features.sort_unstable();
    features.dedup();
    crate::manifest::check_runtime(&manifest, &features)?;
    Ok(files)
}

/// Returns `<fixture>.toml` beside `spec`: the settings of a fixture that differ from the defaults. `None` without one.
fn sidecar(spec: &Path) -> Result<Option<toml_edit::DocumentMut>, Error> {
    let path = spec.with_extension("toml");
    if !crate::disk::is_file(&path) {
        return Ok(None);
    }
    Ok(Some(crate::disk::read(&path)?.parse()?))
}

/// Returns whether the client of a fixture is strict: `true` unless its `client` table sets
/// `allow_unknown_values = true`.
fn strict_client(sidecar: Option<&toml_edit::DocumentMut>) -> bool {
    !sidecar.and_then(|t| t.get("client")?.get("allow_unknown_values")?.as_bool()).unwrap_or(false)
}

/// Returns the types that the `formats` table of a fixture names, such as `formats = { date-time = "String" }`.
fn named_formats(
    sidecar: Option<&toml_edit::DocumentMut>,
) -> Result<Vec<(crate::ir::types::Format, crate::settings::NamedType)>, Error> {
    let Some(table) = sidecar.and_then(|t| t.get("formats")?.as_table_like()) else { return Ok(vec![]) };
    table
        .iter()
        .map(|(key, value)| {
            let format = crate::ir::types::Format::ALL.into_iter().find(|f| f.name() == key);
            let named = value.as_str().and_then(crate::settings::NamedType::new);
            format.zip(named).ok_or_else(|| Error::Setting {
                table: "formats".to_owned(),
                key: key.to_owned(),
                expected: "a format and a Rust type".to_owned(),
                found: value.to_string(),
            })
        })
        .collect()
}
