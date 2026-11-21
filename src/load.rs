//! Loading a document from end to end: parse, bundle, upgrade, model and validate.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json::{Doc, Externals};
use crate::spec::OpenApi;
use crate::{parse, resolve, spec, upgrade, validate};

/// Result of loading a document: the model if it parsed and every diagnostic.
#[derive(Debug)]
pub(crate) struct Loaded {
    /// The bundled document as a 3.2 JSON value.
    pub value: Option<Value>,
    /// The typed model if the document parsed.
    pub model: Option<OpenApi>,
    /// Every problem found in order.
    pub diagnostics: Diagnostics,
    /// The text of each `externalValue` loaded with the document.
    pub externals: Externals,
    /// The version that the document was written in before its upgrade to 3.2.
    pub source: Option<crate::version::Version>,
}

impl Loaded {
    fn failed(diagnostics: Diagnostics) -> Self {
        Self { value: None, model: None, diagnostics, externals: Externals::new(), source: None }
    }

    /// Returns a ref-resolving view of the document.
    pub(crate) fn doc(&self) -> Option<Doc> {
        self.value.clone().map(|v| {
            let mut doc = Doc::new(v);
            doc.externals.clone_from(&self.externals);
            doc
        })
    }
}

/// Loads a document from text. External refs resolve against the working directory.
#[cfg(test)]
pub(crate) fn load_str(text: &str) -> Loaded {
    let mut diagnostics = Diagnostics::default();
    match parse::parse_str(text) {
        Ok((v, _)) => process(v, resolve::Base::new(None), diagnostics),
        Err(d) => {
            diagnostics.push(d);
            Loaded::failed(diagnostics)
        }
    }
}

/// Loads a document from a file. External refs resolve against its directory;
/// `http(s)://` refs are fetched when the generator is built with `remote-refs`.
pub(crate) fn load_path(path: &Path) -> Loaded {
    load_path_with(path, resolve::Base::new(path.parent()), &[])
}

/// Returns [`load_path`] that resolves refs from `base`. Tests pass `Base::offline` to check a document without the
/// network. `overlays` apply in order to the document as written before its external refs are bundled.
pub(crate) fn load_path_with(path: &Path, base: resolve::Base<'_>, overlays: &[PathBuf]) -> Loaded {
    let mut diagnostics = Diagnostics::default();
    match parse::load_file(path) {
        Ok((mut v, _)) => {
            for file in overlays {
                match parse::load_file(file) {
                    Ok((o, _)) => apply_overlay(&mut v, &o, file, &mut diagnostics),
                    Err(d) => diagnostics.push(d),
                }
            }
            process(v, base, diagnostics)
        }
        Err(d) => {
            diagnostics.push(d);
            Loaded::failed(diagnostics)
        }
    }
}

/// Loads several root documents as one ([`crate::merge`]). One file loads as [`load_path_with`] does. `overlays`
/// apply to the merged document.
pub(crate) fn load_paths_with(paths: &[PathBuf], overlays: &[PathBuf]) -> Loaded {
    if let [path] = paths {
        return load_path_with(path, resolve::Base::new(path.parent()), overlays);
    }
    let mut diagnostics = Diagnostics::default();
    let Some(mut v) = crate::merge::merge(paths, &mut diagnostics) else { return Loaded::failed(diagnostics) };
    if diagnostics.has_errors() {
        return Loaded::failed(diagnostics);
    }
    for file in overlays {
        match parse::load_file(file) {
            Ok((o, _)) => apply_overlay(&mut v, &o, file, &mut diagnostics),
            Err(d) => diagnostics.push(d),
        }
    }
    process(v, resolve::Base::new(paths.first().and_then(|p| p.parent())), diagnostics)
}

/// Applies the overlay `o`, loaded from `file`, to `doc`.
#[cfg(feature = "overlays")]
fn apply_overlay(doc: &mut Value, o: &Value, file: &Path, diags: &mut Diagnostics) {
    crate::overlay::apply(doc, o, &file.display().to_string(), diags);
}

/// Reports that this build cannot apply the overlay in `file`.
#[cfg(not(feature = "overlays"))]
fn apply_overlay(_: &mut Value, _: &Value, file: &Path, diags: &mut Diagnostics) {
    diags.push(crate::diag::Diagnostic::error(
        "",
        format!(
            "{}: this build cannot apply overlays: it was installed without the default `overlays` feature",
            file.display()
        ),
    ));
}

/// Validates a document as written. This uses its own `OpenAPI` version and
/// pointers into it. It does not report what an upgrade would translate.
///
/// Swagger 2.0 documents are checked against the 2.0 rules (`validate::v20`).
pub(crate) fn validate_path(path: &Path) -> Diagnostics {
    let mut diagnostics = Diagnostics::default();
    let original = match parse::load_file(path) {
        Ok((v, _)) => v,
        Err(d) => {
            diagnostics.push(d);
            return diagnostics;
        }
    };
    let Some(version) = upgrade::detect(&original, &mut diagnostics) else { return diagnostics };
    if version == crate::version::Version::V20 {
        validate::v20::validate(&original, &mut diagnostics);
        return diagnostics;
    }
    let bundled = resolve::bundle_with(original, resolve::Base::new(path.parent()), &mut diagnostics);
    validate::newer_fields(&bundled, version, &mut diagnostics);
    // Upgrading is internal here: only its errors are problems of the document.
    let mut upgrade_notes = Diagnostics::default();
    let Some(value) = upgrade::to_v32(bundled, &mut upgrade_notes) else {
        diagnostics.extend(upgrade_notes);
        return diagnostics;
    };
    for d in upgrade_notes.iter().filter(|d| d.level == crate::diag::Level::Error) {
        diagnostics.push(d.clone());
    }
    match spec::from_value(value.clone()) {
        Ok(model) => validate::v32::validate_as(&model, &Doc::new(value), version, &mut diagnostics),
        Err(d) => diagnostics.push(d),
    }
    diagnostics
}

fn process(value: Value, base: resolve::Base<'_>, mut diagnostics: Diagnostics) -> Loaded {
    let dir = base.dir;
    let value = resolve::bundle_with(value, base, &mut diagnostics);
    let externals = resolve::read_externals(&value, dir);
    // The upgrade reports what is wrong with the version: this only remembers it.
    let source = upgrade::detect(&value, &mut Diagnostics::default());
    let Some(value) = upgrade::to_v32(value, &mut diagnostics) else {
        return Loaded::failed(diagnostics);
    };
    let model = match spec::from_value(value.clone()) {
        Ok(m) => m,
        Err(d) => {
            diagnostics.push(d);
            return Loaded { value: Some(value), model: None, diagnostics, externals, source };
        }
    };
    let doc = Doc::new(value.clone());
    validate::v32::validate(&model, &doc, &mut diagnostics);
    Loaded { value: Some(value), model: Some(model), diagnostics, externals, source }
}
