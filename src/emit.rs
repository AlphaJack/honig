//! Code emission: the IR to source files with one module per target language.

pub(crate) mod rust;

#[derive(Debug, Clone)]
#[non_exhaustive]
/// What to generate whatever the language. Start from [`Options::default`] and set the fields you need.
pub struct Options {
    /// Emit the client.
    pub client: bool,
    /// Emit the server.
    pub server: bool,
    /// Emit tests that decode the description's examples into the generated types.
    pub tests: bool,
    /// Objects without `additionalProperties` keep unknown keys in `extra`.
    pub keep_unknown: bool,
    /// `false` with `client = { allow_unknown_values = true }`: a response may contain an enum value, a property or a
    /// tagged union member that the description does not list.
    pub strict_client: bool,
    /// Server operations may be left out: each defaults to `501 Not Implemented`. `false`: every one must be written.
    pub allow_unimplemented: bool,
    /// The only operations that the client calls and the server serves, by `operationId` (or their generated name).
    /// `None` means every one. The module contains only those and the types that they reach.
    pub operations: Option<Vec<String>>,
    /// Overlay files applied in order to the description before it is parsed.
    pub overlays: Vec<std::path::PathBuf>,
    /// What only Rust output needs: set from the crate's `Cargo.toml` by the command line.
    pub(crate) rust: rust::Options,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            client: true,
            server: true,
            tests: false,
            keep_unknown: false,
            strict_client: true,
            allow_unimplemented: false,
            operations: None,
            overlays: vec![],
            rust: rust::Options::default(),
        }
    }
}
