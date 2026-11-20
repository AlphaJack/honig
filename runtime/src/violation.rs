//! Where a value breaks its schema and why.

use serde::Serialize;

/// A constraint a value breaks: where (JSON pointer) and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// JSON pointer into the request, e.g. `/body/tags/0`.
    pub path: String,
    /// What is wrong.
    pub message: String,
}
