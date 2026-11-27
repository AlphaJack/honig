//! Schemas move from the 3.0 dialect to JSON Schema 2020-12.
//!
//! | OpenAPI 3.0 | OpenAPI 3.1 |
//! | --- | --- |
//! | `nullable: true` | `"null"` in `type`, in `enum` or in an `anyOf` branch |
//! | boolean `exclusiveMinimum` and `exclusiveMaximum` | their numeric form |
//! | `example` in a schema | `examples: [example]` |
//! | `x-jsonschema-*` extensions | the JSON Schema keywords they carry |
//! | `x-oai-license-identifier` | the license `identifier` |
//!
//! What is lost:
//!
//! - `allowEmptyValue` and `allowReserved` on a header. 3.0 allowed them there without effect. 3.1 has neither.
//! - A registered extension whose field is also set. The field wins.

use serde_json::{Value, json};

use crate::diag::Diagnostics;
use crate::walk::{self, Obj, Visit};

pub(super) fn upgrade(mut doc: Value, diags: &mut Diagnostics) -> Value {
    walk::walk(&mut doc, &mut Schemas { diags });
    if let Some(root) = doc.as_object_mut() {
        if let Some(Value::Object(license)) = root.get_mut("info").and_then(|i| i.get_mut("license")) {
            super::extensions::promote(license, &[("x-oai-license-identifier", "identifier")], "/info/license", diags);
        }
        root.insert("openapi".into(), json!("3.1.1"));
    }
    doc
}

/// JSON Schema keywords a 3.0 schema can only carry as registered `x-jsonschema-*` extensions; 3.1 has them all.
const JSONSCHEMA: &[(&str, &str)] = &[
    ("x-jsonschema-$anchor", "$anchor"),
    ("x-jsonschema-contains", "contains"),
    ("x-jsonschema-minContains", "minContains"),
    ("x-jsonschema-maxContains", "maxContains"),
    ("x-jsonschema-contentEncoding", "contentEncoding"),
    ("x-jsonschema-contentMediaType", "contentMediaType"),
    ("x-jsonschema-contentSchema", "contentSchema"),
    ("x-jsonschema-dependentSchemas", "dependentSchemas"),
    ("x-jsonschema-if", "if"),
    ("x-jsonschema-then", "then"),
    ("x-jsonschema-else", "else"),
    ("x-jsonschema-examples", "examples"),
    ("x-jsonschema-patternProperties", "patternProperties"),
    ("x-jsonschema-propertyNames", "propertyNames"),
    ("x-jsonschema-unevaluatedProperties", "unevaluatedProperties"),
];

struct Schemas<'a> {
    diags: &'a mut Diagnostics,
}

impl Visit for Schemas<'_> {
    fn schema(&mut self, s: &mut Obj, ptr: &str) {
        // Before the children are walked. So the promoted subschemas (`if`, `contains`, ...) are upgraded too.
        super::extensions::promote(s, JSONSCHEMA, ptr, self.diags);
        nullable(s);
        for dropped in ref_siblings(s) {
            self.diags.warning(
                crate::json::join(ptr, &dropped),
                format!("`{dropped}` next to `$ref` has no effect in OpenAPI 3.0 and is left out: put it in the referenced schema or in an `allOf`"),
            );
        }
        exclusive(s, "exclusiveMinimum", "minimum");
        exclusive(s, "exclusiveMaximum", "maximum");
        if let Some(example) = s.remove("example") {
            s.entry("examples").or_insert_with(|| json!([example]));
        }
    }

    fn header(&mut self, h: &mut Obj, _ptr: &str) {
        // A 3.0 Header Object accepts these two Parameter Object fields with no effect on a header. 3.1 has neither.
        h.remove("allowEmptyValue");
        h.remove("allowReserved");
    }
}

fn nullable(s: &mut Obj) {
    let Some(flag) = s.remove("nullable") else { return };
    if flag != Value::Bool(true) {
        return;
    }
    let null = json!("null");
    match s.get_mut("type") {
        Some(Value::String(t)) => {
            let t = std::mem::take(t);
            s.insert("type".into(), json!([t, "null"]));
        }
        Some(Value::Array(ts)) => {
            if !ts.contains(&null) {
                ts.push(null);
            }
        }
        _ => {
            // No `type`: nullability must be added as a union branch.
            if let Some(Value::Array(branches)) = s.get_mut("oneOf") {
                branches.push(json!({"type": "null"}));
            } else if let Some(Value::Array(branches)) = s.get_mut("anyOf") {
                branches.push(json!({"type": "null"}));
            } else if s.contains_key("$ref") || s.contains_key("allOf") {
                // `{$ref: X, nullable: true}` and `{allOf: [X], nullable: true}`
                // are the common 3.0 idioms for "nullable X".
                let mut inner = Obj::new();
                for key in ["$ref", "allOf"] {
                    if let Some(v) = s.remove(key) {
                        inner.insert(key.into(), v);
                    }
                }
                s.insert("anyOf".into(), json!([Value::Object(inner), {"type": "null"}]));
            }
            // Otherwise the schema has no type constraint and already accepts null.
        }
    }
    if let Some(Value::Array(values)) = s.get_mut("enum")
        && !values.contains(&Value::Null)
    {
        values.push(Value::Null);
    }
}

/// Drops the keywords next to `$ref`, which up to 3.0 (JSON Schema draft-04/05) are ignored.
/// In 2020-12 they would apply, so dropping them keeps them meaning nothing. Annotations
/// stay because they never change a type. Runs after `nullable`. 3.0 tooling
/// honours that next to `$ref` and it is converted first.
///
/// Returns the keywords that it dropped: a limit that its author may have meant.
fn ref_siblings(s: &mut Obj) -> Vec<String> {
    const KEEP: &[&str] =
        &["$ref", "description", "summary", "title", "deprecated", "readOnly", "writeOnly", "examples", "default"];
    if !s.contains_key("$ref") {
        return vec![];
    }
    let dropped: Vec<String> =
        s.keys().filter(|k| !KEEP.contains(&k.as_str()) && !k.starts_with("x-")).cloned().collect();
    s.retain(|k, _| KEEP.contains(&k.as_str()) || k.starts_with("x-"));
    dropped
}

pub(super) fn exclusive(s: &mut Obj, flag_key: &str, bound_key: &str) {
    match s.get(flag_key) {
        Some(Value::Bool(true)) => {
            s.remove(flag_key);
            if let Some(bound) = s.remove(bound_key) {
                s.insert(flag_key.into(), bound);
            }
        }
        Some(Value::Bool(false)) => {
            s.remove(flag_key);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(schema: &Value) -> Value {
        let mut doc = json!({"openapi": "3.0.3", "components": {"schemas": {"S": schema}}});
        doc = upgrade(doc, &mut Diagnostics::default());
        doc["components"]["schemas"]["S"].clone()
    }

    /// `x-jsonschema-*` keywords become JSON Schema keywords. Their subschemas are upgraded as well.
    #[test]
    fn registered_jsonschema_extensions_become_keywords() {
        let s = up(&json!({
            "type": "object",
            "x-jsonschema-if": {"properties": {"kind": {"enum": ["a"]}}},
            "x-jsonschema-then": {"required": ["a"], "properties": {"a": {"type": "string", "nullable": true}}},
            "x-jsonschema-propertyNames": {"pattern": "^[a-z]+$"},
            "x-jsonschema-examples": [{"kind": "a"}],
            "example": {"ignored": true}
        }));
        assert_eq!(s["if"], json!({"properties": {"kind": {"enum": ["a"]}}}));
        assert_eq!(s["then"]["properties"]["a"]["type"], json!(["string", "null"]), "promoted subschemas are upgraded");
        assert_eq!(s["propertyNames"]["pattern"], "^[a-z]+$");
        assert_eq!(s["examples"], json!([{"kind": "a"}]), "the registered `examples` wins over `example`");
        assert!(s.as_object().is_some_and(|o| o.keys().all(|k| !k.starts_with("x-jsonschema-"))), "{s}");
        let mut doc = json!({"openapi": "3.0.3", "info": {"title": "t", "version": "1",
            "license": {"name": "MIT", "x-oai-license-identifier": "MIT"}}, "paths": {}});
        doc = upgrade(doc, &mut Diagnostics::default());
        assert_eq!(doc["info"]["license"], json!({"name": "MIT", "identifier": "MIT"}));
    }

    #[test]
    fn nullable_type() {
        assert_eq!(up(&json!({"type": "string", "nullable": true})), json!({"type": ["string", "null"]}));
        assert_eq!(up(&json!({"type": "string", "nullable": false})), json!({"type": "string"}));
    }

    #[test]
    fn nullable_enum_gets_null_value() {
        assert_eq!(
            up(&json!({"type": "string", "enum": ["a"], "nullable": true})),
            json!({"type": ["string", "null"], "enum": ["a", null]})
        );
    }

    #[test]
    fn nullable_ref_becomes_any_of() {
        assert_eq!(
            up(&json!({"allOf": [{"$ref": "#/components/schemas/A"}], "nullable": true, "description": "d"})),
            json!({"description": "d", "anyOf": [{"allOf": [{"$ref": "#/components/schemas/A"}]}, {"type": "null"}]})
        );
    }

    /// A keyword next to a 3.0 `$ref` is left out with a warning that names it: its author may have meant a limit.
    #[test]
    fn a_dropped_ref_sibling_is_a_warning() {
        let mut diags = Diagnostics::default();
        let doc =
            json!({"components": {"schemas": {"B": {"$ref": "#/components/schemas/A", "maxLength": 3, "x-k": 1}}}});
        upgrade(doc, &mut diags);
        let at: Vec<&str> = diags.iter().map(|d| d.pointer.as_str()).collect();
        assert_eq!(at, ["/components/schemas/B/maxLength"]);
    }

    #[test]
    fn ref_siblings_are_ignored_as_in_v30() {
        assert_eq!(
            up(&json!({"$ref": "#/components/schemas/A", "type": "string", "maxLength": 3, "description": "d"})),
            json!({"$ref": "#/components/schemas/A", "description": "d"})
        );
    }

    #[test]
    fn exclusive_bounds_become_numbers() {
        assert_eq!(
            up(
                &json!({"type": "integer", "minimum": 1, "exclusiveMinimum": true, "maximum": 9, "exclusiveMaximum": false})
            ),
            json!({"type": "integer", "exclusiveMinimum": 1, "maximum": 9})
        );
    }

    #[test]
    fn example_becomes_examples() {
        assert_eq!(up(&json!({"type": "string", "example": "x"})), json!({"type": "string", "examples": ["x"]}));
    }

    #[test]
    fn nested_schemas_are_upgraded() {
        let out = up(&json!({"type": "object", "properties": {"a": {"type": "integer", "nullable": true}}}));
        assert_eq!(out["properties"]["a"], json!({"type": ["integer", "null"]}));
    }
}
