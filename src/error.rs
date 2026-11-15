//! What can go wrong besides the description itself: files, the `Cargo.toml` of the user, remote references and the
//! generated code. Problems in a description are [`crate::diag::Diagnostic`]s instead.

use std::fmt;
use std::path::PathBuf;

/// A failure outside the description.
#[derive(Debug)]
pub enum Error {
    /// A file could not be read or written.
    Io {
        /// The file.
        path: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The manifest is not valid TOML.
    Toml(toml_edit::TomlError),
    /// `cargo metadata` failed on the manifest.
    Metadata {
        /// The manifest.
        path: PathBuf,
        /// What cargo said.
        problem: String,
    },
    /// A manifest entry that must be a table or a list is not.
    Shape {
        /// The entry such as `[features]`.
        entry: String,
        /// What it must be.
        kind: &'static str,
    },
    /// The manifest has no `[package.metadata.honig]`.
    NoTable(PathBuf),
    /// A table of settings has a key that is no setting: a typo is not ignored.
    UnknownSetting {
        /// The table: `[package.metadata.<name>]` or one of its modules.
        table: String,
        /// The key.
        key: String,
        /// Every setting there is.
        known: Vec<&'static str>,
    },
    /// The modules: a setting outside a module, a name that is no module, two modules on one output.
    Module {
        /// The table: `[package.metadata.<name>]` or one of its modules.
        table: String,
        /// What is wrong and how to fix it.
        problem: String,
    },
    /// The runtime dependency lists a feature that the runtime does not have.
    Feature {
        /// The feature.
        feature: String,
        /// Why it is not set and what sets it.
        why: String,
    },
    /// A setting holds the wrong kind of value.
    Setting {
        /// The table: `[package.metadata.<name>]` or its `derives`.
        table: String,
        /// The setting.
        key: String,
        /// What it must hold.
        expected: String,
        /// What it holds.
        found: String,
    },
    /// A remote reference could not be fetched.
    Fetch {
        /// The reference.
        url: String,
        /// Why.
        problem: String,
    },
    /// A reference with a scheme other than a path, `http` or `https`.
    Scheme(String),
    /// Generated tokens that do not form a Rust file: a bug in the generator.
    Unparsable {
        /// The generated file.
        file: String,
        /// Why.
        source: syn::Error,
    },
    /// A fixture description of this repository does not generate.
    Fixture {
        /// The description.
        spec: PathBuf,
        /// Its problems.
        problems: crate::diag::Diagnostics,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Toml(e) => write!(f, "Cargo.toml: {e}"),
            Self::Metadata { path, problem } => write!(f, "{}: cargo metadata: {problem}", path.display()),
            Self::Shape { entry, kind } => write!(f, "Cargo.toml: {entry} is not a {kind}"),
            Self::NoTable(path) => {
                write!(
                    f,
                    "{} has no [package.metadata.honig] table. Add one, or run `cargo honig init <spec>`.",
                    path.display()
                )
            }
            Self::Setting { table, key, expected, found } => {
                write!(f, "[{table}] `{key}` must be {expected}, found {found}")
            }
            Self::UnknownSetting { table, key, known } => {
                write!(f, "[{table}] has no setting `{key}`. The settings are {}", known.join(", "))
            }
            Self::Module { table, problem } => write!(f, "[{table}] {problem}"),
            Self::Feature { feature, why } => write!(f, "the honig dependency lists feature `{feature}`: {why}"),
            Self::Fetch { url, problem } => write!(f, "{url}: {problem}"),
            Self::Scheme(uri) => write!(f, "unsupported URI scheme in `{uri}`"),
            Self::Unparsable { file, source } => write!(f, "{file}: generated code does not parse: {source}"),
            Self::Fixture { spec, problems } => {
                write!(f, "{}: does not generate:", spec.display())?;
                problems.0.iter().try_for_each(|d| write!(f, "\n{d}"))
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Toml(e) => Some(e),
            Self::Unparsable { source, .. } => Some(source),
            Self::Metadata { .. }
            | Self::Shape { .. }
            | Self::NoTable(_)
            | Self::Setting { .. }
            | Self::UnknownSetting { .. }
            | Self::Module { .. }
            | Self::Feature { .. }
            | Self::Fetch { .. }
            | Self::Scheme(_)
            | Self::Fixture { .. } => None,
        }
    }
}

impl From<toml_edit::TomlError> for Error {
    fn from(e: toml_edit::TomlError) -> Self {
        Self::Toml(e)
    }
}
