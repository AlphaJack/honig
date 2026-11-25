//! An `enum`, `const` or `default` value that its schema's `type` does not allow. An unquoted YAML `type: string, enum: [007,
//! 1.10]` is the numbers 7 and 1.1: the type falls back to a JSON value and refuses `"007"`.

use serde_json::Value;

use crate::walk::Obj;

/// Returns (pointer, message) for each `enum` or `const` value of `root` that its `type` does not allow.
pub(crate) fn found(root: &Value) -> Vec<(String, String)> {
    crate::walk::findings(root, rule)
}

/// Is `v` a value of JSON Schema type `t`?
fn is(v: &Value, t: &str) -> bool {
    match t {
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "number" => v.is_number(),
        "integer" => {
            v.as_number().is_some_and(|n| n.is_i64() || n.is_u64() || n.as_f64().is_some_and(|f| f.fract() == 0.0))
        }
        _ => true,
    }
}

/// Returns `v` as the description wrote it where JSON can tell: a YAML `007` loads as the float `7.0` and shows as `7`.
fn shown(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if v.is_f64() && f.fract() == 0.0 => format!("{f:.0}"),
        _ => v.to_string(),
    }
}

/// Pushes each `enum` or `const` value of `s` that its `type` does not allow.
fn rule(s: &Obj, ptr: &str, out: &mut Vec<(String, String)>) {
    let types: Vec<&str> = match s.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => return,
    };
    let one_of = |v: &Value| types.iter().any(|t| is(v, t));
    let listed = s.get("enum").and_then(Value::as_array).into_iter().flatten().map(|v| ("enum", v));
    let single = ["const", "default"].into_iter().filter_map(|key| s.get(key).map(|v| (key, v)));
    for (key, value) in listed.chain(single) {
        if !one_of(value) {
            let at = crate::json::join(ptr, key);
            out.push((
                at,
                format!("`{}` is not of type `{}`: quote it or fix the type", shown(value), types.join("` or `")),
            ));
        }
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
    fn a_value_outside_the_type_is_reported_once() {
        assert_eq!(count(&json!({"type": "string", "enum": [7, 1.1, "ok"]})), 2);
        assert_eq!(count(&json!({"type": "integer", "const": "7"})), 1);
        assert_eq!(count(&json!({"type": ["string", "null"], "enum": ["a", null]})), 0);
        assert_eq!(count(&json!({"type": "integer", "enum": [1, 2.0]})), 0, "2.0 is an integer");
        assert_eq!(count(&json!({"enum": [7, "a"]})), 0, "no type, nothing to contradict");
    }

    /// A YAML `007` loads as the float `7.0`: the message shows `7`, as the description wrote the number.
    #[test]
    fn a_whole_float_shows_as_an_integer() {
        let found = found(
            &json!({"openapi": "3.2.0", "components": {"schemas": {"S": {"type": "string", "enum": [7.0, 1.5]}}}}),
        );
        let shown: Vec<&str> = found.iter().map(|(_, m)| m.split('`').nth(1).unwrap_or_default()).collect();
        assert_eq!(shown, ["7", "1.5"]);
    }
}
