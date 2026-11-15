//! Generates a Rust client and server from an `OpenAPI` description. 2.0, 3.0, 3.1 and 3.2 are each upgraded to 3.2
//! first.
//!
//! [`run`] is `cargo honig` as a function. The command line is the interface: this crate has no other public API.
//!
//! ```no_run
//! let code = cargo_honig::run(["generate".to_owned()]);
//! # drop(code);
//! ```
//!
//! The pipeline in order: load, bundle refs, upgrade to 3.2, typed model, validate, IR and emit.

mod cli;
mod diag;
mod disk;
mod emit;
mod error;
mod fixtures;
mod generate;
mod ir;
mod json;
mod load;
mod manifest;
mod merge;
#[cfg(feature = "overlays")]
mod overlay;
mod parse;
mod resolve;
mod schema_ids;
mod settings;
mod spec;
mod text;
mod upgrade;
mod validate;
mod version;
mod walk;

#[cfg(test)]
mod tests;

pub use cli::{run, usage};

/// What the benches and `tests/regen.rs` call. Not part of the API: any release may change it.
#[doc(hidden)]
pub mod __private {
    pub use crate::emit::Options;
    pub use crate::error::Error;
    pub use crate::fixtures::{e2e_files, fixtures};
    pub use crate::generate::{generate, generate_all};
}
