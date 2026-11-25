//! Where a value breaks its schema and why.

use serde::Serialize;

/// A constraint a value breaks: where (JSON pointer) and why. It serializes as a unit of JSON Schema's output format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// JSON pointer into the request, e.g. `/body/tags/0`.
    #[serde(rename = "instanceLocation")]
    pub instance_location: String,
    /// What is wrong.
    pub message: String,
}
