//! A schema that no value of its type can meet: `{type: integer, minimum: 10, maximum: 5}`, `{type: string, minLength:
//! 5, maxLength: 2}`. The generated type refuses every value, so the description cannot be what its author meant.

use serde_json::{Number, Value};

use crate::walk::Obj;

/// Returns (pointer, message) for each schema of `root` that no value of its type meets, where a value must meet it.
pub(crate) fn found(root: &Value) -> Vec<(String, String)> {
    crate::walk::findings(root, rule).into_iter().filter(|(ptr, _)| must_meet(root, ptr)).collect()
}

/// Does a value that the document lets through have to meet the schema at `ptr`? Not under `not`, `if`, `then` or
/// `else`, whose keyword turns what it accepts around, nor in a branch of a union with others, an optional property, or
/// the schema of items, of other names or of names: that place may stay empty. `{not: {enum: []}}` accepts every value,
/// and `properties: {x: {enum: []}}` only forbids `x`.
fn must_meet(root: &Value, ptr: &str) -> bool {
    let mut cur = root;
    let mut tokens = ptr.split('/').skip(1).map(crate::json::unescape_token);
    while let Some(t) = tokens.next() {
        let Some(next) = child(cur, &t) else { return true };
        match t.as_str() {
            "not"
            | "if"
            | "then"
            | "else"
            | "items"
            | "prefixItems"
            | "additionalProperties"
            | "patternProperties"
            | "propertyNames"
            | "dependentSchemas"
            | "unevaluatedItems"
            | "unevaluatedProperties"
            | "contentSchema" => {
                return false;
            }
            "anyOf" | "oneOf" if next.as_array().is_some_and(|a| a.len() > 1) => return false,
            "properties" => {
                let Some(name) = tokens.next() else { return true };
                let required = cur.get("required").and_then(Value::as_array);
                if !required.is_some_and(|r| r.iter().any(|x| x.as_str() == Some(name.as_str()))) {
                    return false;
                }
                let Some(property) = next.get(&name) else { return true };
                cur = property;
                continue;
            }
            // A map of names: the next token is a name, not a keyword.
            "schemas" | "$defs" | "definitions" | "headers" | "content" | "responses" | "callbacks" | "pathItems"
            | "links" | "encoding" | "requestBodies" | "parameters"
                if next.is_object() =>
            {
                let Some(name) = tokens.next() else { return true };
                let Some(named) = next.get(&name) else { return true };
                cur = named;
                continue;
            }
            _ => {}
        }
        cur = next;
    }
    true
}

/// Returns the member `token` of an object, or the item at that index of an array.
fn child<'v>(v: &'v Value, token: &str) -> Option<&'v Value> {
    match v {
        Value::Object(o) => o.get(token),
        Value::Array(a) => token.parse::<usize>().ok().and_then(|i| a.get(i)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
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

pub(super) fn less(a: &Number, b: &Number) -> bool {
    match (a.as_i64(), b.as_i64()) {
        (Some(a), Some(b)) => a < b,
        _ => a.as_f64().zip(b.as_f64()).is_some_and(|(a, b)| a < b),
    }
}

fn equal(a: &Number, b: &Number) -> bool {
    !less(a, b) && !less(b, a)
}

/// The least and the greatest value of an integer `format` (the OpenAPI format registry), as its generated type holds them.
fn format_range(format: &str) -> Option<(i128, i128)> {
    Some(match format {
        "int8" => (i8::MIN.into(), i8::MAX.into()),
        "int16" => (i16::MIN.into(), i16::MAX.into()),
        "int32" => (i32::MIN.into(), i32::MAX.into()),
        "int64" | "double-int" => (i64::MIN.into(), i64::MAX.into()),
        "uint8" => (0, u8::MAX.into()),
        "uint16" => (0, u16::MAX.into()),
        "uint32" => (0, u32::MAX.into()),
        "uint64" | "uint" => (0, u64::MAX.into()),
        _ => return None,
    })
}

/// Returns an integer bound of `s` as the least or greatest value that it lets through: an exclusive one is one past it.
fn whole(s: &Obj, key: &str, exclusive: bool, up: bool) -> Option<i128> {
    let n = number(s, key)?;
    let v = n.as_i128()?;
    Some(match (exclusive, up) {
        (false, _) => v,
        (true, true) => v.checked_add(1)?,
        (true, false) => v.checked_sub(1)?,
    })
}

/// Pushes what is wrong with the bounds of `s`.
fn rule(s: &Obj, ptr: &str, out: &mut Vec<(String, String)>) {
    let members = |k: &str| s.get(k).and_then(Value::as_array);
    if members("allOf").is_some_and(|m| m.contains(&Value::Bool(false))) {
        out.push((crate::json::join(ptr, "allOf"), "`allOf` has a member `false`: no value meets it".to_owned()));
    }
    for key in ["anyOf", "oneOf"] {
        if members(key).is_some_and(|m| !m.is_empty() && m.iter().all(|b| *b == Value::Bool(false))) {
            out.push((crate::json::join(ptr, key), format!("every branch of `{key}` is `false`: no value meets it")));
        }
    }
    if s.get("enum").and_then(Value::as_array).is_some_and(Vec::is_empty) {
        out.push((crate::json::join(ptr, "enum"), "`enum: []` lists no value: no value meets it".to_owned()));
    }
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
    if only(s, &["integer"]) {
        // The least and the greatest integer that the bounds let through, within the range of the `format`.
        let range = s.get("format").and_then(Value::as_str).and_then(|f| format_range(f).map(|r| (f, r)));
        let low =
            [whole(s, "minimum", false, true), whole(s, "exclusiveMinimum", true, true)].into_iter().flatten().max();
        let high =
            [whole(s, "maximum", false, false), whole(s, "exclusiveMaximum", true, false)].into_iter().flatten().min();
        if let Some((format, (lo, hi))) = range
            && (low.is_some_and(|l| l > hi) || high.is_some_and(|h| h < lo))
        {
            out.push((ptr.to_owned(), format!("the bounds leave no value of `format: {format}` ({lo} to {hi})")));
        }
        // An integer `multipleOf`: the least multiple at or above the low bound is past the high one.
        let step = number(s, "multipleOf").and_then(|m| m.as_i64()).filter(|m| *m > 0).map(i128::from);
        if let (Some(m), Some(l), Some(h)) = (step, low, high)
            && l <= h
            && l.div_euclid(m).checked_mul(m).map(|k| if k < l { k + m } else { k }).is_some_and(|k| k > h)
        {
            out.push((ptr.to_owned(), format!("no multiple of `multipleOf` {m} lies from {l} to {h}")));
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

    /// A bound outside the range of the integer `format`, or bounds between which no multiple of `multipleOf` lies, leave
    /// no value.
    #[test]
    fn a_format_range_or_a_multiple_left_empty_is_reported() {
        assert_eq!(count(&json!({"type": "integer", "format": "uint8", "maximum": -1})), 1);
        assert_eq!(count(&json!({"type": "integer", "format": "int8", "minimum": 200})), 1);
        assert_eq!(count(&json!({"type": "integer", "format": "uint8", "exclusiveMaximum": 0})), 1);
        assert_eq!(count(&json!({"type": "integer", "minimum": 1, "maximum": 4, "multipleOf": 5})), 1);
        assert_eq!(count(&json!({"type": "integer", "format": "uint8", "maximum": 300})), 0, "values remain");
        assert_eq!(count(&json!({"type": "integer", "minimum": 1, "maximum": 5, "multipleOf": 5})), 0, "5 remains");
        assert_eq!(count(&json!({"type": "integer", "minimum": -4, "maximum": -1, "multipleOf": 5})), 1);
        assert_eq!(count(&json!({"type": "integer", "minimum": -5, "maximum": -1, "multipleOf": 5})), 0, "-5 remains");
    }

    /// Every integer `format` has its own range: a bound one past it leaves no value, the bound at its edge leaves one.
    #[test]
    fn each_integer_format_has_its_range() {
        let past = [
            ("int16", json!({"minimum": 32_768})),
            ("int32", json!({"minimum": 2_147_483_648_u64})),
            ("int64", json!({"minimum": 9_223_372_036_854_775_808_u64})),
            ("double-int", json!({"minimum": 9_223_372_036_854_775_808_u64})),
            ("uint16", json!({"minimum": 65_536})),
            ("uint32", json!({"minimum": 4_294_967_296_u64})),
            ("uint64", json!({"maximum": -1})),
            ("uint", json!({"maximum": -1})),
        ];
        for (format, mut bound) in past {
            bound["type"] = json!("integer");
            bound["format"] = json!(format);
            assert_eq!(count(&bound), 1, "{bound}");
        }
        assert_eq!(count(&json!({"type": "integer", "format": "uint16", "minimum": 65_535})), 0, "65535 remains");
    }
}
