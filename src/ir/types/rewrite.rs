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

/// Is `key` a keyword that only describes the schema? A branch that drops it loses no check.
fn is_annotation(key: &str) -> bool {
    matches!(key, "title" | "description" | "$comment" | "examples" | "example" | "deprecated")
}

/// Is a `then` or `else` one that the branches of a union can hold? A keyword other than `properties` and `required`
/// (`maxProperties`, `not`) would be dropped by them: the conditional then stays a runtime check as written.
fn plain(t: Option<&Value>) -> bool {
    match t {
        None | Some(Value::Bool(true)) => true,
        Some(Value::Object(t)) => t.keys().all(|k| matches!(k.as_str(), "properties" | "required") || is_annotation(k)),
        Some(_) => false,
    }
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
    if !pairs
        .iter()
        .all(|(_, then, otherwise)| plain(*then) && (plain(*otherwise) || *otherwise == Some(&Value::Bool(false))))
    {
        return None;
    }
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
    let branch = |extra: Option<&Value>, pin: Option<&[String]>| branch(&base, &prop, extra, pin);
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

/// Returns the branch of a conditional union: `base` with the properties and required names of `extra` (a `then` or an
/// `else`) and, with `pin`, `prop` fixed to those values. A property that both describe must meet both.
fn branch(base: &serde_json::Map<String, Value>, prop: &str, extra: Option<&Value>, pin: Option<&[String]>) -> Value {
    let mut b = base.clone();
    let mut props = json::object(&b, "properties").cloned().unwrap_or_default();
    let mut required: Vec<Value> = b.get("required").and_then(Value::as_array).cloned().unwrap_or_default();
    if let Some(t) = extra.and_then(Value::as_object) {
        for (name, schema) in json::object(t, "properties").into_iter().flatten() {
            let both = match props.remove(name) {
                Some(own) => serde_json::json!({ "allOf": [own, schema] }),
                None => schema.clone(),
            };
            props.insert(name.clone(), both);
        }
        required.extend(t.get("required").and_then(Value::as_array).cloned().unwrap_or_default());
    }
    if let Some(values) = pin {
        let pinned = match values {
            [v] => serde_json::json!({ "const": v }),
            _ => serde_json::json!({ "type": "string", "enum": values }),
        };
        props.insert(prop.to_owned(), pinned);
        required.push(Value::String(prop.to_owned()));
    }
    let mut seen = BTreeSet::new();
    required.retain(|r| r.as_str().is_some_and(|r| seen.insert(r.to_owned())));
    b.insert("properties".into(), Value::Object(props));
    b.insert("required".into(), Value::Array(required));
    Value::Object(b)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn union(schema: &Value) -> Option<Value> {
        conditional_union(schema.as_object().expect("an object"))
    }

    fn person(then: &Value) -> Value {
        json!({"type": "object", "properties": {"kind": {"type": "string"}, "name": {"type": "string", "maxLength": 5}},
            "if": {"properties": {"kind": {"const": "person"}}}, "then": then})
    }

    /// A property that the base and the `then` both describe keeps both of its schemas.
    #[test]
    fn a_property_of_the_then_adds_to_the_one_of_the_base() {
        let schema = person(&json!({"properties": {"name": {"minLength": 1}}}));
        let one_of = union(&schema).expect("a union");
        let name = &one_of["oneOf"][0]["properties"]["name"];
        assert_eq!(name, &json!({"allOf": [{"type": "string", "maxLength": 5}, {"minLength": 1}]}));
    }

    /// A keyword of the `then` that a branch cannot hold leaves the conditional as it is: no limit is dropped.
    #[test]
    fn a_then_with_another_keyword_stays_a_runtime_check() {
        assert!(union(&person(&json!({"properties": {"name": {"minLength": 1}}, "maxProperties": 2}))).is_none());
        assert!(union(&person(&json!({"description": "a person"}))).is_some(), "an annotation drops nothing");
    }
}
