//! Examples and defaults that their own schema rejects. A description should not contradict itself: Svix's
//! `featureFlags` has `example: []` beside `minItems: 1`, Meilisearch's `GetLogs.mode` has `default: "Human"` beside
//! `enum: [human, json, profile]`. 3.2 says an example SHOULD match its schema and JSON Schema calls a `default` an
//! annotation, so each mismatch is a warning. The runtime's own evaluator decides, so the check agrees with what a
//! generated server accepts.

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json::{Doc, join};
use crate::walk::{self, Obj, Visit};

/// Values that inlining `$ref`s may build for one schema. A recursive schema would otherwise grow without end. Past the budget a reference reads as `true`: it accepts anything.
const INLINE_BUDGET: usize = 20_000;

/// Nesting that inlining follows. Deeper references read as `true`, as past the budget.
const INLINE_DEPTH: usize = 24;

/// Reports every `default`, `example` and `examples` value that its schema rejects.
pub(crate) fn check(doc: &Doc, diags: &mut Diagnostics) {
    let mut found = Found { doc, pairs: vec![] };
    walk::walk(&mut doc.root.clone(), &mut found);
    for (schema, value, ptr) in found.pairs {
        let mut budget = INLINE_BUDGET;
        let schema = inline(doc, &schema, &mut budget, 0);
        let mut out = vec![];
        crate::validate::interpreter::check_schema(&mut out, &runtime::InstancePath::Root, &value, &schema);
        for v in out {
            let at = if v.path.is_empty() { String::new() } else { format!(" at `{}`", v.path) };
            diags.warning(&ptr, format!("the value does not match its schema{at}: {}", v.message));
        }
    }
}

/// (schema, value, pointer to the value) of every example and default.
struct Found<'a> {
    doc: &'a Doc,
    pairs: Vec<(Value, Value, String)>,
}

impl Found<'_> {
    /// Checks the examples of a parameter, header or media type against its `schema`.
    fn holder(&mut self, o: &Obj, ptr: &str) {
        let Some(schema) = o.get("schema") else { return };
        if let Some(e) = o.get("example") {
            self.pairs.push((schema.clone(), e.clone(), join(ptr, "example")));
        }
        for (k, e) in o.get("examples").and_then(Value::as_object).into_iter().flatten() {
            let e = match e.get("$ref").and_then(Value::as_str) {
                Some(r) => self.doc.get(r).unwrap_or(e),
                None => e,
            };
            let at = join(&join(ptr, "examples"), k);
            for key in ["value", "dataValue"] {
                if let Some(v) = e.get(key) {
                    self.pairs.push((schema.clone(), v.clone(), join(&at, key)));
                }
            }
        }
    }
}

impl Visit for Found<'_> {
    fn schema(&mut self, s: &mut Obj, ptr: &str) {
        let own = || {
            let mut own = s.clone();
            for k in ["default", "example", "examples"] {
                own.remove(k);
            }
            Value::Object(own)
        };
        if let Some(d) = s.get("default") {
            self.pairs.push((own(), d.clone(), join(ptr, "default")));
        }
        if let Some(e) = s.get("example") {
            self.pairs.push((own(), e.clone(), join(ptr, "example")));
        }
        for (i, e) in s.get("examples").and_then(Value::as_array).into_iter().flatten().enumerate() {
            self.pairs.push((own(), e.clone(), join(&join(ptr, "examples"), &i.to_string())));
        }
    }

    fn parameter(&mut self, p: &mut Obj, ptr: &str) {
        self.holder(p, ptr);
    }

    fn header(&mut self, h: &mut Obj, ptr: &str) {
        self.holder(h, ptr);
    }

    fn media_type(&mut self, m: &mut Obj, ptr: &str) {
        self.holder(m, ptr);
    }
}

/// Returns `s` with every local `$ref` replaced by its target. Keywords beside a `$ref` apply too (2020-12), so the two
/// meet in an `allOf`.
pub(crate) fn inline(doc: &Doc, s: &Value, budget: &mut usize, depth: usize) -> Value {
    if *budget == 0 || depth > INLINE_DEPTH {
        return Value::Bool(true);
    }
    *budget -= 1;
    match s {
        Value::Object(o) => {
            if let Some(r) = o.get("$ref").and_then(Value::as_str) {
                let Some(target) = doc.get(r) else { return Value::Bool(true) };
                let target = inline(doc, target, budget, depth + 1);
                let rest: Obj = o.iter().filter(|(k, _)| *k != "$ref").map(|(k, v)| (k.clone(), v.clone())).collect();
                if rest.is_empty() {
                    return target;
                }
                let rest = inline(doc, &Value::Object(rest), budget, depth + 1);
                return serde_json::json!({ "allOf": [target, rest] });
            }
            // Data keywords hold values and not schemas.
            let data = ["default", "example", "examples", "enum", "const"];
            Value::Object(
                o.iter()
                    .map(|(k, v)| {
                        let maps = ["properties", "patternProperties", "$defs", "definitions", "dependentSchemas"];
                        let v = if data.contains(&k.as_str()) {
                            v.clone()
                        } else if let (Value::Object(m), true) = (v, maps.contains(&k.as_str())) {
                            // Maps of schemas: a property may be called `$ref`.
                            Value::Object(
                                m.iter().map(|(n, x)| (n.clone(), inline(doc, x, budget, depth + 1))).collect(),
                            )
                        } else {
                            inline(doc, v, budget, depth + 1)
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(|v| inline(doc, v, budget, depth + 1)).collect()),
        other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)) => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn warnings(doc: Value) -> Vec<String> {
        let mut d = Diagnostics::default();
        check(&Doc::new(doc), &mut d);
        d.iter().map(|x| format!("{} {}", x.pointer, x.message)).collect()
    }

    #[test]
    fn examples_and_defaults_that_their_schema_rejects_are_reported() {
        let doc = json!({
            "openapi": "3.2.0",
            "paths": {"/orders": {"get": {
                "parameters": [{"name": "limit", "in": "query", "schema": {"type": "integer", "maximum": 50}, "example": 80}],
                "responses": {"200": {"description": "ok", "content": {"application/json": {
                    "schema": {"$ref": "#/components/schemas/Order"},
                    "examples": {"paid": {"value": {"status": "paid"}}, "odd": {"value": {"status": "lost"}}}
                }}}}
            }}},
            "components": {"schemas": {
                "Order": {"type": "object", "properties": {"status": {"$ref": "#/components/schemas/Status"}}},
                "Status": {"type": "string", "enum": ["paid", "shipped"], "default": "Paid"},
                "Tags": {"type": "array", "minItems": 1, "items": {"type": "string"}, "example": []}
            }}
        });
        let w = warnings(doc);
        assert_eq!(w.len(), 4, "{w:#?}");
        assert!(w.iter().any(|x| x.starts_with("/paths/~1orders/get/parameters/0/example ")), "{w:#?}");
        assert!(w.iter().any(|x| x.contains("/examples/odd/value") && x.contains("at `/status`")), "{w:#?}");
        assert!(w.iter().any(|x| x.starts_with("/components/schemas/Status/default ")), "{w:#?}");
        assert!(w.iter().any(|x| x.starts_with("/components/schemas/Tags/example ")), "{w:#?}");
    }

    #[test]
    fn a_pattern_decides_which_branch_an_example_matches() {
        // Matrix's `roomIdOrAlias`: an alias matches the `^#` branch and not the `^!` one.
        let doc = json!({"openapi": "3.2.0", "components": {"schemas": {"Room": {
            "oneOf": [{"type": "string", "pattern": "^!"}, {"type": "string", "pattern": "^#"}],
            "example": "#monkeys:matrix.org"
        }}}});
        assert_eq!(warnings(doc), Vec::<String>::new());
    }

    #[test]
    fn a_recursive_schema_is_checked_within_the_budget() {
        let doc = json!({"openapi": "3.2.0", "components": {"schemas": {"Filter": {
            "type": "object",
            "properties": {"must": {"type": "array", "items": {"$ref": "#/components/schemas/Filter"}}},
            "example": {"must": [{"must": []}]}
        }}}});
        assert_eq!(warnings(doc), Vec::<String>::new());
    }
}
