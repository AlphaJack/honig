//! A schema that no value of its type can meet: `{type: integer, minimum: 10, maximum: 5}`, `{type: string, minLength:
//! 5, maxLength: 2}`. The generated type refuses every value, so the description cannot be what its author meant.

use serde_json::{Number, Value};

use crate::walk::Obj;

/// Returns (pointer, message) for each schema of `root` that no value of its type meets.
pub(crate) fn found(root: &Value) -> Vec<(String, String)> {
    crate::walk::findings(root, rule)
}

/// Does the schema accept only values of `kinds`? It has a `type` and every type listed is one of them.
fn only(s: &Obj, kinds: &[&str]) -> bool {
    match s.get("type") {
        Some(Value::String(t)) => kinds.contains(&t.as_str()),
        Some(Value::Array(ts)) => !ts.is_empty() && ts.iter().all(|t| t.as_str().is_some_and(|t| kinds.contains(&t))),
        _ => false,
    }
}

fn number(s: &Obj, key: &str) -> Option<Number> {
    s.get(key).and_then(Value::as_number).cloned()
}

fn less(a: &Number, b: &Number) -> bool {
    match (a.as_i64(), b.as_i64()) {
        (Some(a), Some(b)) => a < b,
        _ => a.as_f64().zip(b.as_f64()).is_some_and(|(a, b)| a < b),
    }
}

fn equal(a: &Number, b: &Number) -> bool {
    !less(a, b) && !less(b, a)
}

/// Pushes what is wrong with the bounds of `s`.
fn rule(s: &Obj, ptr: &str, out: &mut Vec<(String, String)>) {
    let mut pair = |min: &str, max: &str| {
        if let Some((low, high)) = number(s, min).zip(number(s, max))
            && less(&high, &low)
        {
            out.push((ptr.to_owned(), format!("`{min}` {low} is above `{max}` {high}: no value meets both")));
        }
    };
    if only(s, &["string"]) {
        pair("minLength", "maxLength");
    }
    if only(s, &["array"]) {
        pair("minItems", "maxItems");
    }
    if only(s, &["object"]) {
        pair("minProperties", "maxProperties");
    }
    if only(s, &["integer", "number"]) {
        // The lowest bound and the highest: an exclusive one at the same value leaves no number.
        let lows = [("minimum", false), ("exclusiveMinimum", true)];
        let highs = [("maximum", false), ("exclusiveMaximum", true)];
        for (low, low_exclusive) in lows {
            for (high, high_exclusive) in highs {
                let Some((l, h)) = number(s, low).zip(number(s, high)) else { continue };
                if less(&h, &l) || (equal(&l, &h) && (low_exclusive || high_exclusive)) {
                    out.push((ptr.to_owned(), format!("`{low}` {l} and `{high}` {h}: no value meets both")));
                }
            }
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
    fn only_a_contradiction_of_the_type_is_reported() {
        assert_eq!(count(&json!({"type": "integer", "minimum": 10, "maximum": 5})), 1);
        assert_eq!(count(&json!({"type": "integer", "minimum": 5, "exclusiveMaximum": 5})), 1);
        assert_eq!(count(&json!({"type": "number", "exclusiveMinimum": 1.5, "maximum": 1.5})), 1);
        assert_eq!(count(&json!({"type": "string", "minLength": 5, "maxLength": 2})), 1);
        assert_eq!(count(&json!({"type": "array", "minItems": 3, "maxItems": 1})), 1);
        assert_eq!(count(&json!({"type": "object", "minProperties": 2, "maxProperties": 1})), 1);
        assert_eq!(count(&json!({"type": "integer", "minimum": 5, "maximum": 5})), 0, "one value remains");
        assert_eq!(count(&json!({"type": ["string", "null"], "minLength": 5, "maxLength": 2})), 0, "null passes");
        assert_eq!(count(&json!({"minimum": 10, "maximum": 5})), 0, "no type: a string passes");
    }
}
