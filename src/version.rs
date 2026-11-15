//! The version families that a description may be written in. Every pass from upgrade on tells them apart.

/// Source version family of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    /// Swagger 2.0.
    V20,
    /// `OpenAPI` 3.0.x.
    V30,
    /// `OpenAPI` 3.1.x.
    V31,
    /// `OpenAPI` 3.2.x.
    V32,
}
