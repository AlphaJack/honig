//! Structure-aware traversal of an `OpenAPI` document stored as a `Value`. It covers 3.x and the 2.0 sections that
//! bundling sees.
//!
//! Upgrades need to visit "every schema" or "every parameter". Those live in a
//! dozen places. This walker knows them all. So each upgrade step only writes
//! the per-node transform.

use serde_json::{Map, Value};

use crate::json::join;

/// A JSON object.
pub(crate) type Obj = Map<String, Value>;

/// Callbacks for node kinds. All default to no-ops. A callback runs before its
/// children are visited. So it may restructure them.
pub(crate) trait Visit {
    /// Called for every schema object and its subschemas.
    fn schema(&mut self, _schema: &mut Obj, _ptr: &str) {}
    /// Called for every inline Parameter Object.
    fn parameter(&mut self, _param: &mut Obj, _ptr: &str) {}
    /// Called for every inline Header Object.
    fn header(&mut self, _header: &mut Obj, _ptr: &str) {}
    /// Called for every inline Media Type Object.
    fn media_type(&mut self, _media: &mut Obj, _ptr: &str) {}
    /// Called for every inline Path Item Object (paths, webhooks, callbacks, `components.pathItems`).
    fn path_item(&mut self, _item: &mut Obj, _ptr: &str) {}
    /// Called for every Encoding Object by name, by position and nested.
    fn encoding(&mut self, _encoding: &mut Obj, _ptr: &str) {}
    /// Called for every inline Example Object (`examples` maps and `components.examples`).
    fn example(&mut self, _example: &mut Obj, _ptr: &str) {}
    /// Called for every Server Object of the document, a path item or an operation.
    fn server(&mut self, _server: &mut Obj, _ptr: &str) {}
    /// Called for every inline Response Object.
    fn response(&mut self, _response: &mut Obj, _ptr: &str) {}
    /// Called for every inline Security Scheme Object.
    fn security_scheme(&mut self, _scheme: &mut Obj, _ptr: &str) {}
}

const SCHEMA_MAPS: &[&str] = &["properties", "patternProperties", "$defs", "dependentSchemas", "definitions"];
const SCHEMA_ARRAYS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
const SCHEMA_SINGLES: &[&str] = &[
    "items",
    "contains",
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "propertyNames",
    "not",
    "if",
    "then",
    "else",
    "contentSchema",
];
const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace", "query"];

/// Visits every schema, parameter and media type of a 3.x document or of a 2.0 one before its upgrade.
pub(crate) fn walk(doc: &mut Value, v: &mut dyn Visit) {
    let Some(root) = doc.as_object_mut() else { return };
    servers(root, "", v);
    if let Some(paths) = root.get_mut("paths").and_then(Value::as_object_mut) {
        for (k, item) in paths.iter_mut() {
            if !k.starts_with("x-") {
                path_item(item, &join("/paths", k), v);
            }
        }
    }
    if let Some(hooks) = root.get_mut("webhooks").and_then(Value::as_object_mut) {
        for (k, item) in hooks.iter_mut() {
            path_item(item, &join("/webhooks", k), v);
        }
    }
    if let Some(c) = root.get_mut("components").and_then(Value::as_object_mut) {
        components(c, v);
    }
    // The sections of 2.0. Bundling sees them before the upgrade moves them under `components`.
    each_entry(root, "definitions", "", &mut |s, p| schema(s, p, v));
    each_entry(root, "parameters", "", &mut |s, p| parameter(s, p, v));
    each_entry(root, "responses", "", &mut |s, p| response(s, p, v));
}

fn each_entry(obj: &mut Obj, key: &str, base: &str, f: &mut dyn FnMut(&mut Value, &str)) {
    if let Some(m) = obj.get_mut(key).and_then(Value::as_object_mut) {
        let base = join(base, key);
        for (k, val) in m.iter_mut() {
            if !k.starts_with("x-") {
                f(val, &join(&base, k));
            }
        }
    }
}

fn components(c: &mut Obj, v: &mut dyn Visit) {
    let base = "/components";
    each_entry(c, "schemas", base, &mut |s, p| schema(s, p, v));
    each_entry(c, "parameters", base, &mut |s, p| parameter(s, p, v));
    each_entry(c, "headers", base, &mut |s, p| header(s, p, v));
    each_entry(c, "requestBodies", base, &mut |s, p| body_like(s, p, v));
    each_entry(c, "responses", base, &mut |s, p| response(s, p, v));
    each_entry(c, "mediaTypes", base, &mut |s, p| media_type(s, p, v));
    each_entry(c, "callbacks", base, &mut |s, p| callback(s, p, v));
    each_entry(c, "pathItems", base, &mut |s, p| path_item(s, p, v));
    each_entry(c, "examples", base, &mut |e, p| example(e, p, v));
    each_entry(c, "securitySchemes", base, &mut |s, p| {
        if let Some(obj) = s.as_object_mut().filter(|o| !o.contains_key("$ref")) {
            v.security_scheme(obj, p);
        }
    });
}

fn path_item(item: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = item.as_object_mut() else { return };
    if !obj.contains_key("$ref") {
        v.path_item(obj, ptr);
    }
    servers(obj, ptr, v);
    params_array(obj, ptr, v);
    for m in METHODS {
        if let Some(op) = obj.get_mut(*m) {
            operation(op, &join(ptr, m), v);
        }
    }
    each_entry(obj, "additionalOperations", ptr, &mut |op, p| operation(op, p, v));
}

fn params_array(obj: &mut Obj, ptr: &str, v: &mut dyn Visit) {
    if let Some(ps) = obj.get_mut("parameters").and_then(Value::as_array_mut) {
        let base = join(ptr, "parameters");
        for (i, p) in ps.iter_mut().enumerate() {
            parameter(p, &format!("{base}/{i}"), v);
        }
    }
}

fn operation(op: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = op.as_object_mut() else { return };
    servers(obj, ptr, v);
    params_array(obj, ptr, v);
    if let Some(body) = obj.get_mut("requestBody") {
        body_like(body, &join(ptr, "requestBody"), v);
    }
    each_entry(obj, "responses", ptr, &mut |r, p| response(r, p, v));
    each_entry(obj, "callbacks", ptr, &mut |cb, p| callback(cb, p, v));
}

fn callback(cb: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = cb.as_object_mut() else { return };
    for (k, item) in obj.iter_mut() {
        if !k.starts_with("x-") && k != "$ref" {
            path_item(item, &join(ptr, k), v);
        }
    }
}

fn parameter(p: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = p.as_object_mut() else { return };
    if obj.contains_key("$ref") {
        return;
    }
    v.parameter(obj, ptr);
    header_like(obj, ptr, v);
}

fn header(h: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = h.as_object_mut() else { return };
    if obj.contains_key("$ref") {
        return;
    }
    v.header(obj, ptr);
    header_like(obj, ptr, v);
}

fn header_like(obj: &mut Obj, ptr: &str, v: &mut dyn Visit) {
    if let Some(s) = obj.get_mut("schema") {
        schema(s, &join(ptr, "schema"), v);
    }
    each_entry(obj, "examples", ptr, &mut |e, p| example(e, p, v));
    each_entry(obj, "content", ptr, &mut |m, p| media_type(m, p, v));
}

fn body_like(b: &mut Value, ptr: &str, v: &mut dyn Visit) {
    if let Some(obj) = b.as_object_mut() {
        each_entry(obj, "content", ptr, &mut |m, p| media_type(m, p, v));
    }
}

fn response(r: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = r.as_object_mut() else { return };
    if !obj.contains_key("$ref") {
        v.response(obj, ptr);
    }
    // A 2.0 response contains its schema directly.
    if let Some(s) = obj.get_mut("schema") {
        schema(s, &join(ptr, "schema"), v);
    }
    each_entry(obj, "headers", ptr, &mut |h, p| header(h, p, v));
    each_entry(obj, "content", ptr, &mut |m, p| media_type(m, p, v));
}

fn media_type(m: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = m.as_object_mut() else { return };
    if obj.contains_key("$ref") {
        return;
    }
    v.media_type(obj, ptr);
    for key in ["schema", "itemSchema"] {
        if let Some(s) = obj.get_mut(key) {
            schema(s, &join(ptr, key), v);
        }
    }
    each_entry(obj, "examples", ptr, &mut |e, p| example(e, p, v));
    positional_encodings(obj, ptr, v);
    each_entry(obj, "encoding", ptr, &mut |e, p| encoding(e, p, v));
}

fn encoding(e: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = e.as_object_mut() else { return };
    v.encoding(obj, ptr);
    each_entry(obj, "headers", ptr, &mut |h, p| header(h, p, v));
    each_entry(obj, "encoding", ptr, &mut |e, p| encoding(e, p, v));
    positional_encodings(obj, ptr, v);
}

/// Walks the `prefixEncoding` items and `itemEncoding` of a Media Type Object or a nested Encoding Object.
fn positional_encodings(obj: &mut Obj, ptr: &str, v: &mut dyn Visit) {
    if let Some(arr) = obj.get_mut("prefixEncoding").and_then(Value::as_array_mut) {
        let base = join(ptr, "prefixEncoding");
        for (i, e) in arr.iter_mut().enumerate() {
            encoding(e, &format!("{base}/{i}"), v);
        }
    }
    if let Some(e) = obj.get_mut("itemEncoding") {
        encoding(e, &join(ptr, "itemEncoding"), v);
    }
}

fn example(e: &mut Value, ptr: &str, v: &mut dyn Visit) {
    if let Some(obj) = e.as_object_mut().filter(|o| !o.contains_key("$ref")) {
        v.example(obj, ptr);
    }
}

/// Walks the `servers` list of the document, a path item or an operation.
fn servers(obj: &mut Obj, ptr: &str, v: &mut dyn Visit) {
    if let Some(arr) = obj.get_mut("servers").and_then(Value::as_array_mut) {
        let base = join(ptr, "servers");
        for (i, s) in arr.iter_mut().enumerate() {
            if let Some(s) = s.as_object_mut() {
                v.server(s, &format!("{base}/{i}"));
            }
        }
    }
}

/// Visits a schema) and every subschema under it.
pub(crate) fn schema(s: &mut Value, ptr: &str, v: &mut dyn Visit) {
    let Some(obj) = s.as_object_mut() else { return };
    v.schema(obj, ptr);
    for key in SCHEMA_MAPS {
        each_entry_all(obj, key, ptr, &mut |s, p| schema(s, p, v));
    }
    for key in SCHEMA_ARRAYS {
        if let Some(arr) = obj.get_mut(*key).and_then(Value::as_array_mut) {
            let base = join(ptr, key);
            for (i, s) in arr.iter_mut().enumerate() {
                schema(s, &format!("{base}/{i}"), v);
            }
        }
    }
    for key in SCHEMA_SINGLES {
        match obj.get_mut(*key) {
            // Draft-04 positional `items: [A, B]` still shows up in the wild.
            Some(Value::Array(arr)) if *key == "items" => {
                let base = join(ptr, key);
                for (i, s) in arr.iter_mut().enumerate() {
                    schema(s, &format!("{base}/{i}"), v);
                }
            }
            Some(s) => schema(s, &join(ptr, key), v),
            None => {}
        }
    }
}

/// Walks each entry as `each_entry` does, but property names starting with `x-` are real schemas.
fn each_entry_all(obj: &mut Obj, key: &str, base: &str, f: &mut dyn FnMut(&mut Value, &str)) {
    if let Some(m) = obj.get_mut(key).and_then(Value::as_object_mut) {
        let base = join(base, key);
        for (k, val) in m.iter_mut() {
            f(val, &join(&base, k));
        }
    }
}

/// Runs `rule` on every schema of `root`. It pushes (pointer, message) for what is wrong with a schema.
pub(crate) fn findings(root: &Value, rule: fn(&Obj, &str, &mut Vec<(String, String)>)) -> Vec<(String, String)> {
    struct Each(fn(&Obj, &str, &mut Vec<(String, String)>), Vec<(String, String)>);
    impl Visit for Each {
        fn schema(&mut self, s: &mut Obj, ptr: &str) {
            (self.0)(s, ptr, &mut self.1);
        }
    }
    let mut each = Each(rule, vec![]);
    walk(&mut root.clone(), &mut each);
    each.1
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Count(Vec<String>, Vec<String>);
    impl Visit for Count {
        fn schema(&mut self, _: &mut Obj, ptr: &str) {
            self.0.push(ptr.to_owned());
        }
        fn parameter(&mut self, _: &mut Obj, ptr: &str) {
            self.1.push(ptr.to_owned());
        }
    }

    #[test]
    fn finds_schemas_everywhere() {
        let mut doc = json!({
            "paths": {"/a": {
                "parameters": [{"name": "p", "in": "query", "schema": {"type": "string"}}],
                "get": {"responses": {"200": {"content": {"application/json": {"schema": {
                    "type": "object", "properties": {"x-y": {"items": {"type": "integer"}}}
                }}}}}}
            }},
            "components": {"schemas": {"A": {"allOf": [{"type": "object"}]}}}
        });
        let mut c = Count(vec![], vec![]);
        walk(&mut doc, &mut c);
        assert_eq!(c.1, ["/paths/~1a/parameters/0"]);
        assert!(c.0.contains(
            &"/paths/~1a/get/responses/200/content/application~1json/schema/properties/x-y/items".to_owned()
        ));
        assert!(c.0.contains(&"/components/schemas/A/allOf/0".to_owned()));
        assert_eq!(c.0.len(), 6);
    }
}
