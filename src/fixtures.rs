//! What `tests/e2e` holds: every fixture description of this repository generated as a module, and the manifest
//! that compiles them. `tests/regen.rs` compares the crate with it and rewrites what is stale. It lives in the library
//! so that a change to the generator rebuilds the library and one small test, not every unit test.

use std::path::{Path, PathBuf};

use crate::error::Error;

/// Lints that the generated code passes on top of the defaults of clippy. That is pedantic and the nursery and
/// restriction lints that a strict crate turns on. The generated `lib.rs` warns on them. So the clippy run of the
/// workspace checks them.
const LINTS: &[&str] = &[
    "clippy::pedantic",
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
/// A folder cannot be read.
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

/// Returns what the crate at `e2e` must hold for the descriptions under `root`, as (path relative to it, contents). Its
/// manifest gets the runtime of this checkout with the features that the fixtures need.
///
/// # Errors
/// A file cannot be read or a fixture does not generate.
pub fn e2e_files(e2e: &Path, root: &Path) -> Result<Vec<(PathBuf, String)>, Error> {
    let text = crate::disk::read(&e2e.join("Cargo.toml"))?;
    let parsed = crate::settings::Manifest::load(&e2e.join("Cargo.toml"))?;
    let libs = crate::settings::format_libs(&parsed)?;
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
        let generated = crate::generate::generate(&spec, &opts)
            .map_err(|problems| Error::Fixture { spec: spec.clone(), problems })?;
        let api = Path::new("src").join(&module).join("api");
        files.extend(generated.files.into_iter().map(|(name, text)| (api.join(name), text)));
        features.extend(generated.runtime_features);
        lib.push_str(&format!("pub mod {module} {{\n    pub mod api;\n}}\n"));
    }
    files.push((Path::new("src").join("lib.rs"), lib));
    let manifest = crate::manifest::with_runtime(&text, &features)?;
    let mut doc: toml_edit::DocumentMut = manifest.parse()?;
    doc["dependencies"]["honig"]["path"] = toml_edit::value("../../runtime");
    files.push((PathBuf::from("Cargo.toml"), doc.to_string()));
    Ok(files)
}
