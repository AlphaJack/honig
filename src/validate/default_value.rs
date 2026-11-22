//! A `default` that its own schema refuses: `{maximum: 100, default: 500}`. A field that the sender leaves out decodes
//! as the default, so an absent field would hold a value that no sender could have written.

use serde_json::Value;

use crate::walk::Obj;

/// Returns (pointer, message) for each `default` of `root` that its schema refuses.
pub(crate) fn found(root: &Value) -> Vec<(String, String)> {
    crate::walk::findings(root, rule)
}

/// Pushes why the `default` of `s` breaks its schema.
fn rule(s: &Obj, ptr: &str, out: &mut Vec<(String, String)>) {
    let Some(default) = s.get("default") else { return };
    let at = crate::json::join(ptr, "default");
    let mut refuse = |why: String| out.push((at.clone(), format!("the default `{default}` {why}")));
    if let Some(Value::Array(values)) = s.get("enum")
        && !values.contains(default)
    {
        refuse("is not one of the `enum` values".to_owned());
    }
    if let Some(constant) = s.get("const")
        && constant != default
    {
        refuse(format!("is not the `const` `{constant}`"));
    }
    let bound = |key: &str| s.get(key).and_then(Value::as_number);
    match default {
        Value::Number(n) => {
            let below = |key: &str, exclusive: bool| {
                bound(key).is_some_and(|b| {
                    super::unsatisfiable::less(n, b) || (exclusive && !super::unsatisfiable::less(b, n))
                })
            };
            let above = |key: &str, exclusive: bool| {
                bound(key).is_some_and(|b| {
                    super::unsatisfiable::less(b, n) || (exclusive && !super::unsatisfiable::less(n, b))
                })
            };
            for (key, exclusive) in [("minimum", false), ("exclusiveMinimum", true)] {
                if below(key, exclusive) {
                    refuse(format!("is below `{key}`"));
                }
            }
            for (key, exclusive) in [("maximum", false), ("exclusiveMaximum", true)] {
                if above(key, exclusive) {
                    refuse(format!("is above `{key}`"));
                }
            }
        }
        Value::String(text) => size(s, "minLength", "maxLength", text.chars().count(), &mut refuse),
        Value::Array(items) => size(s, "minItems", "maxItems", items.len(), &mut refuse),
        Value::Object(members) => size(s, "minProperties", "maxProperties", members.len(), &mut refuse),
        Value::Null | Value::Bool(_) => {}
    }
}

/// Refuses a `size` that is outside `min` and `max` of `s`.
fn size(s: &Obj, min: &str, max: &str, size: usize, refuse: &mut impl FnMut(String)) {
    let limit = |key: &str| s.get(key).and_then(Value::as_u64);
    if limit(min).is_some_and(|m| (size as u64) < m) {
        refuse(format!("is shorter than `{min}`"));
    }
    if limit(max).is_some_and(|m| (size as u64) > m) {
        refuse(format!("is longer than `{max}`"));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn count(schema: &Value) -> usize {
        found(&json!({"openapi": "3.2.0", "components": {"schemas": {"S": schema}}})).len()
    }

    #[test]
    fn a_default_outside_its_schema_is_reported() {
        assert_eq!(count(&json!({"type": "integer", "maximum": 100, "default": 500})), 1);
        assert_eq!(count(&json!({"type": "integer", "exclusiveMinimum": 1, "default": 1})), 1);
        assert_eq!(count(&json!({"type": "string", "maxLength": 3, "default": "four"})), 1);
        assert_eq!(count(&json!({"type": "array", "minItems": 2, "default": [1]})), 1);
        assert_eq!(count(&json!({"type": "string", "enum": ["a", "b"], "default": "c"})), 1);
        assert_eq!(count(&json!({"const": "x", "default": "y"})), 1);
        assert_eq!(count(&json!({"type": "integer", "minimum": 1, "maximum": 100, "default": 100})), 0);
        assert_eq!(count(&json!({"type": "string", "maxLength": 4, "default": "four"})), 0);
        assert_eq!(count(&json!({"type": "string", "maximum": 3, "default": "any"})), 0, "bounds of numbers only");
    }
}
