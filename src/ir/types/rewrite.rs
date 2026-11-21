//! Schema rewrites that the builder types instead of the schema as written. That is the request view without
//! `readOnly` requirements and `if`/`then`/`else` on one property as the `oneOf` that it means.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::json;

/// Returns the values that an `if` tests: `{properties: {P: {const: V}}}` or a string
/// `enum` with an optional `required: [P]`. Returns (P, values).
pub(crate) fn tested_value(cond: &Value) -> Option<(String, Vec<String>)> {
    let c = cond.as_object()?;
    if c.keys().any(|k| k != "properties" && k != "required" && k != "type") {
        return None;
    }
    let props = c.get("properties")?.as_object()?;
    let [(p, test)] = props.iter().collect::<Vec<_>>()[..] else { return None };
    let values = match (test.get("const"), test.get("enum").and_then(Value::as_array)) {
        (Some(Value::String(v)), _) => vec![v.clone()],
        (None, Some(values)) if !values.is_empty() => {
            values.iter().map(json::owned_str).collect::<Option<Vec<String>>>()?
        }
        _ => return None,
    };
    Some((p.clone(), values))
}

/// Marks the branch a conditional union falls back to: values no `if` tests.
pub(crate) const FALLBACK: &str = "x-honig-fallback";

/// Returns `if {P: const V} then {...} else {...}` on the schema itself or in
/// `allOf` members that all test the same property. It becomes the equivalent `oneOf`.
/// There is one branch per tested value set (base properties plus that `then` with `P`
/// pinned to the values). An `Otherwise` branch accepts every other value with
/// the base properties plus the `else` of a lone conditional. `else: false`
/// leaves no fallback. Conditions that cannot be typed stay with runtime
/// validation.
pub(crate) fn conditional_union(o: &serde_json::Map<String, Value>) -> Option<Value> {
    let mut pairs: Vec<(&Value, Option<&Value>, Option<&Value>)> = vec![];
    if let Some(i) = o.get("if") {
        pairs.push((i, o.get("then"), o.get("else")));
    }
    let members = o.get("allOf").and_then(Value::as_array);
    let conditional = |m: &Value| {
        m.as_object()
            .is_some_and(|m| m.contains_key("if") && m.keys().all(|k| matches!(k.as_str(), "if" | "then" | "else")))
    };
    for m in members.into_iter().flatten().filter(|m| conditional(m)) {
        pairs.push((m.get("if")?, m.get("then"), m.get("else")));
    }
    if pairs.is_empty() {
        return None;
    }
    // With several conditionals an `else` overlaps the other branches: it stays a runtime check.
    let otherwise = match pairs.as_slice() {
        [(_, _, e)] => *e,
        _ => None,
    };
    let mut prop = None;
    let mut branches: Vec<(Vec<String>, Option<&Value>)> = vec![];
    let mut seen_values = BTreeSet::new();
    for (cond, then, _) in &pairs {
        let (p, values) = tested_value(cond)?;
        if prop.as_ref().is_some_and(|q| *q != p) || !values.iter().all(|v| seen_values.insert(v.clone())) {
            return None;
        }
        prop = Some(p);
        branches.push((values, *then));
    }
    let prop = prop?;
    let mut base = o.clone();
    for k in ["if", "then", "else"] {
        base.remove(k);
    }
    if let Some(rest) = members.map(|ms| ms.iter().filter(|m| !conditional(m)).cloned().collect::<Vec<_>>()) {
        if rest.is_empty() {
            base.remove("allOf");
        } else {
            base.insert("allOf".into(), Value::Array(rest));
        }
    }
    base.entry("type").or_insert_with(|| Value::String("object".into()));
    let branch = |extra: Option<&Value>, pin: Option<&[String]>| {
        let mut b = base.clone();
        let mut props = json::object(&b, "properties").cloned().unwrap_or_default();
        let mut required: Vec<Value> = b.get("required").and_then(Value::as_array).cloned().unwrap_or_default();
        if let Some(t) = extra.and_then(Value::as_object) {
            props.extend(json::object(t, "properties").cloned().unwrap_or_default());
            required.extend(t.get("required").and_then(Value::as_array).cloned().unwrap_or_default());
        }
        if let Some(values) = pin {
            let pinned = match values {
                [v] => serde_json::json!({ "const": v }),
                _ => serde_json::json!({ "type": "string", "enum": values }),
            };
            props.insert(prop.clone(), pinned);
            required.push(Value::String(prop.clone()));
        }
        let mut seen = BTreeSet::new();
        required.retain(|r| r.as_str().is_some_and(|r| seen.insert(r.to_owned())));
        b.insert("properties".into(), Value::Object(props));
        b.insert("required".into(), Value::Array(required));
        Value::Object(b)
    };
    let mut one_of: Vec<Value> = branches.iter().map(|(values, then)| branch(*then, Some(values))).collect();
    if otherwise != Some(&Value::Bool(false)) {
        let mut fallback = branch(otherwise, None);
        if let Value::Object(f) = &mut fallback {
            f.insert(FALLBACK.into(), Value::Bool(true));
        }
        one_of.push(fallback);
    }
    Some(serde_json::json!({ "oneOf": one_of }))
}
