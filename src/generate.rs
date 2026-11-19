//! End-to-end generation: spec file to formatted module files on disk.

use std::path::Path;

use crate::diag::Diagnostics;
use crate::emit::Options;
use crate::emit::rust::{self, needs};

#[derive(Debug)]
#[non_exhaustive]
/// The result of generating a module.
pub struct Generated {
    /// Files to write as (path relative to the module directory, contents).
    pub files: Vec<(String, String)>,
    /// Every diagnostic from loading and building.
    pub diagnostics: Diagnostics,
    /// Number of generated operations.
    pub operations: usize,
    /// Number of generated named types.
    pub types: usize,
    /// Runtime features the description needs whatever the crate depends on: the codecs it calls.
    pub runtime_features: Vec<&'static str>,
}

/// Returns load, validate, build the IR and emit formatted files.
///
/// Fails when the document cannot be parsed, or when generated code could not hold it: a `pattern` that does not
/// compile, or a server whose security scheme does not resolve. Other validation errors are reported but not fatal.
pub fn generate(spec: &Path, opts: &Options) -> Result<Generated, Diagnostics> {
    generate_all(&[spec.to_path_buf()], opts)
}

/// Returns [`generate`] from several root documents merged into one: their paths, webhooks, components and tags.
///
/// # Errors
/// As [`generate`]. Also when two files describe the same operation or give one component name two values.
pub fn generate_all(specs: &[std::path::PathBuf], opts: &Options) -> Result<Generated, Diagnostics> {
    let loaded = load(specs, opts)?;
    let built = ir(&loaded, opts);
    let trees = emit(&built, opts);
    print(&loaded, built, trees, opts)
}

/// A description loaded, bundled and checked: what [`ir()`] builds from.
///
/// [`generate_all`] runs the phases in turn: [`load()`], [`ir()`], [`emit()`] and [`print()`]. The scaling bench
/// measures each alone.
#[derive(Debug)]
pub struct Loaded {
    doc: crate::json::Doc,
    diagnostics: Diagnostics,
    from_v20: bool,
}

/// The types and operations of a description: what [`emit()`] turns into syntax trees.
#[derive(Debug)]
pub struct Ir {
    api: crate::ir::Api,
    diagnostics: Diagnostics,
}

/// The files of a module as syntax trees: what [`print()`] writes as Rust source.
#[derive(Debug)]
pub struct Trees(rust::Trees);

/// Loads `specs` merged into one description, bundles their references and checks what generated code cannot hold.
///
/// # Errors
/// As [`generate_all`].
pub fn load(specs: &[std::path::PathBuf], opts: &Options) -> Result<Loaded, Diagnostics> {
    let loaded = crate::load::load_paths_with(specs, &opts.overlays);
    let mut diagnostics = loaded.diagnostics.clone();
    if loaded.model.is_none() {
        return Err(diagnostics);
    }
    let Some(doc) = loaded.doc() else { return Err(diagnostics) };
    // A requirement on a scheme the generator cannot see cannot be checked: generating
    // anyway would serve the operation without it.
    let unresolved = crate::resolve::unresolved_security(&doc.root);
    if opts.server && !unresolved.is_empty() {
        for (ptr, key) in unresolved {
            diagnostics.error(
                ptr,
                format!("security scheme `{key}` does not resolve: refusing to generate a server that would skip it"),
            );
        }
        return Err(diagnostics);
    }
    // A `pattern` that does not compile cannot be checked: its type would take every value. Validation reported each
    // as an error.
    if crate::validate::v32::has_uncompilable_pattern(&doc.root, "") {
        return Err(diagnostics);
    }
    let from_v20 = loaded.source == Some(crate::version::Version::V20);
    Ok(Loaded { doc, diagnostics, from_v20 })
}

/// Builds the types and operations of `loaded`.
pub fn ir(loaded: &Loaded, opts: &Options) -> Ir {
    let mut diagnostics = Diagnostics::default();
    let api = crate::ir::build_with(
        &loaded.doc,
        &mut diagnostics,
        opts.keep_unknown,
        loaded.from_v20,
        rust::names::RESERVED,
        &opts.rust.crates,
        !opts.strict_client,
    );
    Ir { api, diagnostics }
}

/// Returns the files of the module of `built` as syntax trees.
pub fn emit(built: &Ir, opts: &Options) -> Trees {
    Trees(rust::trees(&built.api, opts))
}

/// Prints `trees` as the files of the module, with what [`generate_all`] reports beside them.
///
/// # Errors
/// The trees do not form Rust files: a bug of the generator, reported as an error.
pub fn print(loaded: &Loaded, built: Ir, trees: Trees, opts: &Options) -> Result<Generated, Diagnostics> {
    let mut diagnostics = loaded.diagnostics.clone();
    diagnostics.0.extend(built.diagnostics.0);
    let api = &built.api;
    let files = match rust::print(trees.0, opts.rust.binary) {
        Ok(files) => files,
        Err(e) => {
            diagnostics.error("", format!("{e} (a honig bug: please report it)"));
            return Err(diagnostics);
        }
    };
    diagnostics.0.extend(rust::warnings(api, opts));
    Ok(Generated {
        runtime_features: needs::runtime_features(
            api,
            opts.rust.libs,
            opts.client,
            opts.server,
            needs::Uses {
                patterns: needs::uses_pattern(&loaded.doc.root, ""),
                mail_formats: needs::uses_format(
                    &loaded.doc.root,
                    "",
                    &["email", "idn-email", "hostname", "idn-hostname"],
                ),
            },
        ),
        files,
        diagnostics,
        operations: api.http.operations.len(),
        types: api.types.defs.len(),
    })
}

/// Returns the canonical form for comparing a file on disk with fresh output. So the `cargo fmt` of the user over
/// generated files does not make them stale. It is printed by `prettyplease` and keeps a leading `// @generated` line
/// (syn drops plain comments). Text that does not parse stays as it is.
fn canonical(code: &str) -> String {
    let (header, body) = match code.split_once('\n') {
        Some((first, rest)) if first.starts_with("// @generated") => (format!("{first}\n"), rest),
        _ => (String::new(), code),
    };
    // The one place a string becomes a syntax tree: normalizing a file already
    // on disk for the stale check (and the bridge for user-edited files).
    #[expect(clippy::disallowed_methods, reason = "reads existing files back; generation itself builds trees")]
    let Ok(file) = syn::parse_file(body) else { return code.to_owned() };
    format!("{header}{}", rust::syntax::unparse(file))
}

/// Whether the file on disk holds the fresh `code`. Git on Windows can check it out with CRLF line endings.
fn same(existing: &str, code: &str) -> bool {
    let existing = existing.replace("\r\n", "\n");
    existing == code || canonical(&existing) == canonical(code)
}

/// Returns the files of the module that differ from `files` (missing or changed), the root at `root`, and a root
/// left in the other place (extra).
pub(crate) fn stale(dir: &Path, root: &Path, files: &[(String, String)]) -> Vec<String> {
    let mut out = vec![];
    for (name, code) in files {
        match crate::disk::read(&crate::disk::placed(dir, root, name)) {
            Ok(existing) if same(&existing, code) => {}
            Ok(_) => out.push(format!("{name} (changed)")),
            Err(_) => out.push(format!("{name} (missing)")),
        }
    }
    let inside = dir.join(crate::disk::ROOT_FILE);
    let other = if root == inside { dir.with_extension("rs") } else { inside };
    if crate::disk::is_generated(&other) {
        out.push(format!("{} (extra)", other.display()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{same, stale};
    use crate::disk::{is_file, write_module};

    #[test]
    fn a_file_with_crlf_line_endings_is_not_stale() {
        let code = "// @generated by honig\npub struct A;\n";
        assert!(same(&code.replace('\n', "\r\n"), code));
        assert!(!same("// @generated by honig\r\npub struct B;\r\n", code));
    }

    #[test]
    fn the_root_of_a_module_moves_between_mod_rs_and_a_file_beside_it() {
        let src = std::env::temp_dir().join(format!("honig-module-root-{}", std::process::id())).join("src");
        let dir = src.join("api");
        let (inside, beside) = (dir.join("mod.rs"), src.join("api.rs"));
        let files = vec![
            ("mod.rs".to_owned(), "// @generated by honig\npub mod types;\n".to_owned()),
            ("types.rs".to_owned(), "// @generated by honig\npub struct A;\n".to_owned()),
        ];
        write_module(&dir, &beside, &files).unwrap();
        assert!(is_file(&beside) && is_file(&dir.join("types.rs")) && !is_file(&inside));
        assert_eq!(stale(&dir, &beside, &files), Vec::<String>::new());
        // Back to `mod.rs`: the root beside the directory is the one of an earlier run, so it goes.
        assert_eq!(
            stale(&dir, &inside, &files),
            ["mod.rs (missing)".to_owned(), format!("{} (extra)", beside.display())]
        );
        write_module(&dir, &inside, &files).unwrap();
        assert!(is_file(&inside) && !is_file(&beside));
        assert_eq!(stale(&dir, &inside, &files), Vec::<String>::new());
        // And again: the `mod.rs` in the directory goes.
        write_module(&dir, &beside, &files).unwrap();
        assert!(is_file(&beside) && !is_file(&inside));
    }
}
