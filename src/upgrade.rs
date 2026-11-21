//! In any version:
//!
//! - A field that must be a boolean, such as `required`, may hold a YAML 1.1 word: `yes`, `no`, `on`, `off`, `y` or
//!   `n`. honig reads it as `true` or `false`.
//! - An unknown `type` in a schema is ignored.
//! - A `discriminator` whose property is optional and that has no `defaultMapping` gets a warning. A value without the
//!   property decodes into an untyped variant.

mod extensions;
mod v20_to_v30;
mod v30_to_v31;
mod v31_to_v32;

use std::collections::BTreeSet;

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json::join;
use crate::version::Version;

/// Detects the version from the `swagger` / `openapi` field.
pub(crate) fn detect(value: &Value, diags: &mut Diagnostics) -> Option<Version> {
    let Some(obj) = value.as_object() else {
        diags.error("", "document root must be an object");
        return None;
    };
    if let Some(s) = obj.get("swagger") {
        return match s.as_str() {
            Some("2.0") => Some(Version::V20),
            _ => {
                diags.error("/swagger", format!("unsupported Swagger version {s}"));
                None
            }
        };
    }
    let Some(v) = obj.get("openapi") else {
        diags.error("", "missing `openapi` (or `swagger`) version field");
        return None;
    };
    let Some(v) = v.as_str() else {
        diags.error("/openapi", "`openapi` must be a string like \"3.2.0\"");
        return None;
    };
    let family = v.split('.').take(2).collect::<Vec<_>>().join(".");
    match family.as_str() {
        "3.0" => Some(Version::V30),
        "3.1" => Some(Version::V31),
        "3.2" => Some(Version::V32),
        _ => {
            diags.error("/openapi", format!("unsupported OpenAPI version {v}"));
            None
        }
    }
}

/// Fields the specifications define as booleans.
const BOOLEAN_FIELDS: &[&str] = &[
    "required",
    "deprecated",
    "allowEmptyValue",
    "explode",
    "allowReserved",
    "uniqueItems",
    "readOnly",
    "writeOnly",
    "nullable",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "attribute",
    "wrapped",
];

/// Converts YAML 1.1 booleans (`yes`, `no`, `on`, `off`, `y`, `n`) in fields that must be
/// booleans. Documents from YAML 1.1 tools (common for Swagger 2.0) use them.
/// The loader parses YAML 1.2. That keeps them as strings.
fn yaml11_booleans(o: &mut serde_json::Map<String, Value>, ptr: &str, diags: &mut Diagnostics) {
    for key in BOOLEAN_FIELDS {
        let Some(Value::String(s)) = o.get(*key) else { continue };
        let b = match s.to_ascii_lowercase().as_str() {
            "yes" | "y" | "on" => true,
            "no" | "n" | "off" => false,
            _ => continue,
        };
        diags.warning(join(ptr, key), format!("`{s}` is a YAML 1.1 boolean; reading it as {b}"));
        o.insert((*key).into(), Value::Bool(b));
    }
}

/// Applies [`yaml11_booleans`] to every object of the document except data.
fn normalize_booleans(v: &mut Value, ptr: &str, parent: &str, diags: &mut Diagnostics) {
    match v {
        Value::Object(o) => {
            yaml11_booleans(o, ptr, diags);
            for (k, c) in o.iter_mut() {
                if !crate::json::is_data(parent, k) {
                    normalize_booleans(c, &join(ptr, k), k, diags);
                }
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter_mut().enumerate() {
                normalize_booleans(c, &format!("{ptr}/{i}"), parent, diags);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Upgrades to 3.2. Returns `None` if the document cannot be upgraded.
pub(crate) fn to_v32(value: Value, diags: &mut Diagnostics) -> Option<Value> {
    let mut v = value;
    let mut version = detect(&v, diags)?;
    normalize_booleans(&mut v, "", "", diags);
    if version == Version::V32 {
        return Some(recursive_refs(v31_to_v32::repair_all(v, diags), diags));
    }
    loop {
        (v, version) = match version {
            Version::V20 => (v20_to_v30::upgrade(v, diags), Version::V30),
            Version::V30 => (v30_to_v31::upgrade(v, diags), Version::V31),
            Version::V31 => (v31_to_v32::upgrade(v, diags), Version::V32),
            Version::V32 => return Some(recursive_refs(v, diags)),
        };
    }
}

/// Returns JSON Schema 2019-09 `$recursiveRef: "#"` becomes a `$ref` to the schema that it
/// lands on. 2019-09 section 8.2.4.2: when the initial target sets
/// `$recursiveAnchor: true` the reference lands on the *outermost* schema
/// of the dynamic scope that also sets it. [`extend_recursive`] first copies
/// each extended schema into the schema that extends it. So that rule becomes
/// lexical. Then each `$recursiveRef` points at its nearest enclosing schema
/// with `$recursiveAnchor: true` (or `$id`).
fn recursive_refs(mut doc: Value, diags: &mut Diagnostics) -> Value {
    use crate::walk::{self, Obj, Visit};

    /// Pointers of every schema a `$recursiveRef` can land on.
    struct Anchors(Vec<String>);
    impl Visit for Anchors {
        fn schema(&mut self, s: &mut Obj, ptr: &str) {
            if s.get("$recursiveAnchor") == Some(&Value::Bool(true)) || s.contains_key("$id") {
                self.0.push(ptr.to_owned());
            }
        }
    }
    struct Rewrite<'a> {
        anchors: Vec<String>,
        diags: &'a mut Diagnostics,
    }
    impl Visit for Rewrite<'_> {
        fn schema(&mut self, s: &mut Obj, ptr: &str) {
            s.remove("$recursiveAnchor");
            let Some(Value::String(r)) = s.remove("$recursiveRef") else { return };
            if r != "#" {
                // Any other value resolves statically like `$ref`.
                s.entry("$ref").or_insert(Value::String(r));
                return;
            }
            match nearest(&self.anchors, ptr) {
                Some(a) => {
                    s.entry("$ref").or_insert_with(|| Value::String(format!("#{}", fragment(a))));
                }
                None => self.diags.error(
                    join(ptr, "$recursiveRef"),
                    "`$recursiveRef: \"#\"` has no enclosing schema with `$recursiveAnchor: true` or `$id`",
                ),
            }
        }
    }

    if !crate::json::mentions(&doc, "$recursiveRef") {
        return doc;
    }
    extend_recursive(&mut doc, diags);
    let mut anchors = Anchors(vec![]);
    walk::walk(&mut doc, &mut anchors);
    walk::walk(&mut doc, &mut Rewrite { anchors: anchors.0, diags });
    doc
}

/// Returns the longest pointer in `anchors` that is `ptr` or encloses it.
fn nearest<'a>(anchors: &'a [String], ptr: &str) -> Option<&'a String> {
    anchors.iter().filter(|a| ptr == a.as_str() || ptr.starts_with(&format!("{a}/"))).max_by_key(|a| a.len())
}

/// Rounds of unfolding in [`extend_recursive`]. A chain unfolds one level a round. Real chains are short.
const MAX_PASSES: usize = 8;

/// Makes dynamic scope lexical. A `$ref` inside a schema E with
/// `$recursiveAnchor: true` that reaches a `$recursiveRef` of another
/// anchored schema R evaluates R in the dynamic scope of E. The recursion of R lands on
/// E (the outer one). Replace that `$ref` with a copy of its target as an
/// `allOf` member. That is what `$ref` next to siblings means. Remove the own
/// anchor of the copy. E is then the nearest anchor of every
/// `$recursiveRef` in it. Chains (F extends E extends R) unfold one level a
/// round.
fn extend_recursive(doc: &mut Value, diags: &mut Diagnostics) {
    use crate::json::{Doc, pointer};
    use crate::walk::{self, Obj, Visit};

    #[derive(Default)]
    struct Sites {
        /// Schemas with `$recursiveAnchor: true`.
        anchored: Vec<String>,
        /// (pointer, `$ref`) of every schema with a `$ref`.
        refs: Vec<(String, String)>,
    }
    impl Visit for Sites {
        fn schema(&mut self, s: &mut Obj, ptr: &str) {
            if s.get("$recursiveAnchor") == Some(&Value::Bool(true)) {
                self.anchored.push(ptr.to_owned());
            }
            if let Some(r) = s.get("$ref").and_then(Value::as_str) {
                self.refs.push((ptr.to_owned(), r.to_owned()));
            }
        }
    }

    /// Does the schema at `ptr` (following `$ref`s) hold a `$recursiveRef`
    /// whose own resource is anchored? Then it resolves dynamically.
    fn reaches(index: &Doc, ptr: &str, anchored: &[String], seen: &mut BTreeSet<String>) -> bool {
        fn go(index: &Doc, v: &Value, at: &str, anchored: &[String], seen: &mut BTreeSet<String>) -> bool {
            match v {
                Value::Object(o) => {
                    if o.contains_key("$recursiveRef") && nearest(anchored, at).is_some() {
                        return true;
                    }
                    if let Some(t) = o.get("$ref").and_then(Value::as_str).and_then(|r| index.local_pointer(r))
                        && seen.insert(t.clone())
                        && pointer(&index.root, &t).is_some_and(|tv| go(index, tv, &t, anchored, seen))
                    {
                        return true;
                    }
                    o.iter().any(|(k, c)| !crate::json::is_data("", k) && go(index, c, &join(at, k), anchored, seen))
                }
                Value::Array(a) => {
                    a.iter().enumerate().any(|(i, c)| go(index, c, &format!("{at}/{i}"), anchored, seen))
                }
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
            }
        }
        pointer(&index.root, ptr).is_some_and(|v| go(index, v, ptr, anchored, seen))
    }

    for _ in 0..MAX_PASSES {
        let mut sites = Sites::default();
        walk::walk(doc, &mut sites);
        let index = Doc::new(doc.clone());
        let mut changed = false;
        for (ptr, r) in &sites.refs {
            let Some(outer) = nearest(&sites.anchored, ptr) else { continue };
            let Some(target) = index.local_pointer(r) else { continue };
            let within = |a: &str, b: &str| a == b || a.starts_with(&format!("{b}/"));
            // Inside its own resource or an enclosing one. Static and dynamic agree.
            if within(&target, outer) || within(outer, &target) {
                continue;
            }
            if !reaches(&index, &target, &sites.anchored, &mut BTreeSet::from([target.clone()])) {
                continue;
            }
            let Some(mut copy) = pointer(&index.root, &target).cloned() else { continue };
            if let Value::Object(c) = &mut copy {
                c.remove("$recursiveAnchor");
                c.remove("$id");
            }
            if let Some(Value::Object(site)) = doc.pointer_mut(ptr) {
                site.remove("$ref");
                match site.get_mut("allOf") {
                    Some(Value::Array(all)) => all.push(copy),
                    _ => {
                        site.insert("allOf".into(), Value::Array(vec![copy]));
                    }
                }
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
    diags.warning("", "`$recursiveRef` extensions nest more than 8 deep: deeper ones resolve to the extended schema");
}

/// Returns a JSON pointer as a URI fragment: characters outside RFC 3986 `pchar`
/// percent-encoded.
fn fragment(ptr: &str) -> String {
    let mut out = String::with_capacity(ptr.len());
    for b in ptr.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A schema inside a callback is upgraded as any other: 3.0 `nullable` becomes a `null` type.
    #[test]
    fn a_callback_schema_is_upgraded() {
        let doc = json!({
            "openapi": "3.0.3",
            "info": {"title": "t", "version": "1"},
            "paths": {"/a": {"post": {"responses": {"204": {"description": "ok"}}, "callbacks": {"done": {
                "{$request.body#/url}": {"post": {
                    "requestBody": {"content": {"application/json": {"schema": {"type": "string", "nullable": true}}}},
                    "responses": {"204": {"description": "ok"}}
                }},
                "x-note": {"post": {"requestBody": {"content": {"application/json": {"schema": {"nullable": true}}}}}}
            }}}}}
        });
        let mut d = Diagnostics::default();
        let up = to_v32(doc, &mut d).unwrap();
        let op = &up["paths"]["/a"]["post"]["callbacks"]["done"]["{$request.body#/url}"]["post"];
        assert_eq!(op["requestBody"]["content"]["application/json"]["schema"], json!({"type": ["string", "null"]}));
        let note = &up["paths"]["/a"]["post"]["callbacks"]["done"]["x-note"];
        let kept = json!({"post": {"requestBody": {"content": {"application/json": {"schema": {"nullable": true}}}}}});
        assert_eq!(note, &kept, "an extension is left alone");
    }

    /// A 3.0 response header leaves the two Parameter Object fields that 3.1 has no more.
    #[test]
    fn a_header_drops_the_fields_of_a_parameter() {
        let doc = json!({
            "openapi": "3.0.3",
            "info": {"title": "t", "version": "1"},
            "paths": {"/a": {"get": {"responses": {"200": {"description": "ok", "headers": {
                "X-A": {"schema": {"type": "string"}, "allowEmptyValue": true, "allowReserved": true}
            }}}}}}
        });
        let mut d = Diagnostics::default();
        let up = to_v32(doc, &mut d).unwrap();
        let h = &up["paths"]["/a"]["get"]["responses"]["200"]["headers"]["X-A"];
        assert_eq!(h, &json!({"schema": {"type": "string"}}));
    }

    #[test]
    fn yaml_1_1_booleans_in_boolean_fields() {
        let doc = json!({
            "swagger": "2.0",
            "info": {"title": "t", "version": "1"},
            "paths": {"/a": {"get": {
                "parameters": [{"in": "query", "name": "y", "type": "string", "required": "yes"}],
                "responses": {"200": {"description": "ok", "examples": {"application/json": {"required": "yes"}}}}
            }}}
        });
        let mut d = Diagnostics::default();
        let up = to_v32(doc, &mut d).unwrap();
        let p = &up["paths"]["/a"]["get"]["parameters"][0];
        assert_eq!((p["name"].clone(), p["required"].clone()), (json!("y"), json!(true)));
        // Example data is left alone.
        assert_eq!(
            up["paths"]["/a"]["get"]["responses"]["200"]["content"]["application/json"]["example"]["required"],
            "yes"
        );
    }

    #[test]
    fn swagger_reaches_32_and_parses() {
        let doc = json!({
            "swagger": "2.0",
            "info": {"title": "t", "version": "1"},
            "host": "x.com",
            "paths": {"/a": {"get": {
                "parameters": [{"in": "query", "name": "n", "type": "integer", "x-nullable": true}],
                "responses": {"200": {"description": "ok", "schema": {"type": "string"}}}
            }}}
        });
        let mut d = Diagnostics::default();
        let up = to_v32(doc, &mut d).unwrap();
        assert_eq!(up["openapi"], "3.2.0");
        assert_eq!(up["paths"]["/a"]["get"]["parameters"][0]["schema"]["type"], json!(["integer", "null"]));
        crate::spec::from_value(up).unwrap();
    }
}
