//! Rules for a schema that cannot be what its author meant.

use serde_json::Value;

/// Returns (pointer, message) for each schema of `root` that cannot be what its author meant: bounds that no value
/// meets, an `enum` or `const` value that its `type` refuses, a `default` that its schema refuses, a `pattern` that does
/// not compile.
pub(crate) fn errors(root: &Value) -> Vec<(String, String)> {
    let mut out = super::unsatisfiable::found(root);
    out.extend(super::enum_type::found(root));
    out.extend(super::default_value::found(root));
    out.extend(crate::walk::findings(root, uncompilable_patterns));
    out
}

/// Returns why `pattern` does not compile as generated code compiles it: with `regress`, as ECMA-262 with the `u` flag
/// or else without it. `None` when it compiles.
fn pattern_error(pattern: &str) -> Option<String> {
    regress::Regex::with_flags(pattern, "u").or_else(|_| regress::Regex::new(pattern)).err().map(|e| e.to_string())
}

/// Pushes an error for each `pattern` and `patternProperties` key of `s` that does not compile: a type that claims a
/// pattern it cannot check would accept every value.
fn uncompilable_patterns(s: &crate::walk::Obj, ptr: &str, out: &mut Vec<(String, String)>) {
    let own = s.get("pattern").and_then(Value::as_str).map(|p| (crate::json::join(ptr, "pattern"), p));
    let keys = s.get("patternProperties").and_then(Value::as_object).into_iter().flat_map(serde_json::Map::keys);
    let keys = keys.map(|p| (crate::json::join(&crate::json::join(ptr, "patternProperties"), p), p.as_str()));
    for (at, p) in own.into_iter().chain(keys) {
        if let Some(e) = pattern_error(p) {
            out.push((at, format!("pattern `{p}` is not an ECMA-262 regular expression ({e})")));
        }
    }
}
