//! Tests across the pipeline. A description goes in. Diagnostics, the IR or generated text come out.
//!
//! Fixtures under `tests/fixtures/` are descriptions. `tests/e2e` holds the crate generated from them with its
//! hand-written tests. `generated_crates::wasm_client` compiles a crate of its own. So it is ignored by default.

#![expect(clippy::disallowed_methods, reason = "tests set up files and run cargo")]

use std::path::{Path, PathBuf};

mod common;
mod compliance;
mod coverage;
mod dialects;
mod directions;
mod docs;
mod docs_pages;
mod examples;
mod formats;
mod fuzz;
mod generated_crates;
mod generated_shape;
mod generated_text;
mod multi_document;
mod oai_conformance;
#[cfg(feature = "overlays")]
mod overlay;
mod patterns;
mod schema_suite;
mod spec_roundtrip;
mod validate_native;

/// The repository's folder.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}
