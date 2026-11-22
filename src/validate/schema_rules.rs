//! Rules for a schema that cannot be what its author meant.

use serde_json::Value;

/// Returns (pointer, message) for each schema of `root` that cannot be what its author meant: bounds that no value
/// meets, an `enum` or `const` value that its `type` refuses.
pub(crate) fn errors(root: &Value) -> Vec<(String, String)> {
    let mut out = super::unsatisfiable::found(root);
    out.extend(super::enum_type::found(root));
    out
}
