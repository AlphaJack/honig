//! | OpenAPI 3.1 | OpenAPI 3.2 |
//! | --- | --- |
//! | array-form `items` (JSON Schema 2019-09 and older) | `prefixItems` |
//! | `dependencies` (JSON Schema draft 7 and older) | `dependentRequired` and `dependentSchemas` |
//! | an object where an array is required | an array of its values |
//! | `xml.attribute` and `xml.wrapped` | `xml.nodeType`. With both, `attribute` wins |
//! | `x-tagGroups` | `tags[].parent`, only when the mapping is unambiguous |
//! | registered `x-oai-*` extensions | the 3.2 fields they carry: `$self`, `additionalOperations`, `itemSchema`, `itemEncoding`, `prefixEncoding`, `encoding`, `dataValue`, `serializedValue`, the `name` of a server, the `summary` of a response and the `deprecated` and device authorization flow of a security scheme |
//!
//! What is lost:
//!
//! - `allowReserved` on a parameter that is not in the query. It was inactive in 3.1 and would change meaning in 3.2.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::diag::Diagnostics;
use crate::json;
use crate::json::join;
use crate::walk::{self, Obj, Visit};

pub(super) fn upgrade(mut doc: Value, diags: &mut Diagnostics) -> Value {
    walk::walk(&mut doc, &mut Migrate { diags });
    if let Some(root) = doc.as_object_mut() {
        super::extensions::promote(root, &[("x-oai-$self", "$self")], "", diags);
        let mut op_tags = BTreeSet::new();
        collect_op_tags(root, &mut op_tags);
        tag_groups(root, &op_tags);
        root.insert("openapi".into(), json!("3.2.0"));
    }
    doc
}

struct Migrate<'a> {
    diags: &'a mut Diagnostics,
}

/// Runs only the repairs. Used for documents that are already 3.2.
pub(super) fn repair_all(mut doc: Value, diags: &mut Diagnostics) -> Value {
    struct Repair<'a>(&'a mut Diagnostics);
    impl Visit for Repair<'_> {
        fn schema(&mut self, s: &mut Obj, ptr: &str) {
            repair(s, ptr, self.0);
            // A MUST in 3.2. For upgraded documents it gets a warning and is not enforced.
            if let Some(Value::Object(d)) = s.get("discriminator")
                && !d.contains_key("defaultMapping")
                && let Some(prop) = d.get("propertyName").and_then(Value::as_str)
                && discriminator_maybe_optional(s, prop)
            {
                self.0.error(
                    join(ptr, "discriminator"),
                    format!("discriminator property `{prop}` is optional, so `defaultMapping` is required"),
                );
            }
        }
    }
    walk::walk(&mut doc, &mut Repair(diags));
    doc
}

impl Visit for Migrate<'_> {
    fn path_item(&mut self, item: &mut Obj, ptr: &str) {
        // Before the walker visits operations. So the promoted ones are migrated too.
        super::extensions::promote(item, &[("x-oai-additionalOperations", "additionalOperations")], ptr, self.diags);
    }

    fn media_type(&mut self, m: &mut Obj, ptr: &str) {
        let pairs = [
            ("x-oai-itemSchema", "itemSchema"),
            ("x-oai-itemEncoding", "itemEncoding"),
            ("x-oai-prefixEncoding", "prefixEncoding"),
        ];
        super::extensions::promote(m, &pairs, ptr, self.diags);
    }

    fn encoding(&mut self, e: &mut Obj, ptr: &str) {
        let pairs = [("x-oai-encoding", "encoding"), ("x-oai-itemEncoding", "itemEncoding")];
        super::extensions::promote(e, &pairs, ptr, self.diags);
    }

    fn example(&mut self, e: &mut Obj, ptr: &str) {
        let pairs = [("x-oai-dataValue", "dataValue"), ("x-oai-serializedValue", "serializedValue")];
        super::extensions::promote(e, &pairs, ptr, self.diags);
    }

    fn server(&mut self, s: &mut Obj, ptr: &str) {
        super::extensions::promote(s, &[("x-oai-name", "name")], ptr, self.diags);
    }

    fn response(&mut self, r: &mut Obj, ptr: &str) {
        super::extensions::promote(r, &[("x-oai-summary", "summary")], ptr, self.diags);
    }

    fn security_scheme(&mut self, s: &mut Obj, ptr: &str) {
        super::extensions::promote(s, &[("x-oai-deprecated", "deprecated")], ptr, self.diags);
        let Some(Value::Object(flows)) = s.get_mut("flows") else { return };
        let at = join(ptr, "flows");
        super::extensions::promote(flows, &[("x-oai-deviceAuthorization", "deviceAuthorization")], &at, self.diags);
        for (name, flow) in flows.iter_mut() {
            if let Value::Object(flow) = flow {
                let pairs = [("x-oai-deviceAuthorizationUrl", "deviceAuthorizationUrl")];
                super::extensions::promote(flow, &pairs, &join(&at, name), self.diags);
            }
        }
    }

    fn schema(&mut self, s: &mut Obj, ptr: &str) {
        repair(s, ptr, self.diags);
        if let Some(Value::Object(xml)) = s.get_mut("xml") {
            let attribute = xml.remove("attribute") == Some(Value::Bool(true));
            let wrapped = xml.remove("wrapped") == Some(Value::Bool(true));
            if attribute && wrapped {
                self.diags
                    .warning(join(ptr, "xml"), "`wrapped` and `attribute` cannot both be true; using `attribute`");
            }
            if !xml.contains_key("nodeType") {
                if attribute {
                    xml.insert("nodeType".into(), json!("attribute"));
                } else if wrapped {
                    xml.insert("nodeType".into(), json!("element"));
                }
            }
        }
        if let Some(Value::Object(d)) = s.get("discriminator")
            && !d.contains_key("defaultMapping")
            && let Some(prop) = d.get("propertyName").and_then(Value::as_str)
            && discriminator_maybe_optional(s, prop)
        {
            self.diags.warning(
                join(ptr, "discriminator"),
                format!(
                    "discriminator property `{prop}` is optional and has no `defaultMapping`; \
                     values without it fall back to an untyped variant"
                ),
            );
        }
    }

    fn parameter(&mut self, p: &mut Obj, ptr: &str) {
        // Before 3.2 `allowReserved` only applied to query parameters and
        // `style: cookie` did not exist. Both are invalid in the source document.
        if p.get("in").and_then(Value::as_str) != Some("query") && p.remove("allowReserved").is_some() {
            self.diags.error(
                join(ptr, "allowReserved"),
                "`allowReserved` is only allowed on query parameters before OpenAPI 3.2",
            );
        }
        if p.get("style").and_then(Value::as_str) == Some("cookie") {
            self.diags.error(join(ptr, "style"), "`style: cookie` is new in OpenAPI 3.2");
        }
    }
}

/// Repair invalid shapes that are common in real-world specs with a warning each.
/// Every document passes through this step whatever its source version.
const KNOWN_TYPES: &[&str] = &["null", "boolean", "object", "array", "number", "integer", "string"];

fn repair(s: &mut Obj, ptr: &str, diags: &mut Diagnostics) {
    // `nullable` is OpenAPI 3.0 only. In 3.1 and later it is an unknown keyword: `null` is not allowed. Generators
    // that emit 3.1 keep writing it, so the generated type rejecting `null` comes as a surprise.
    if s.get("nullable") == Some(&Value::Bool(true)) {
        diags.warning(
            join(ptr, "nullable"),
            "`nullable` is not an OpenAPI 3.1 keyword and has no effect: to allow null, add \"null\" to `type`",
        );
    }
    // `type: file` is Swagger 2.0 only but leaks into 3.x documents.
    let is_file = |v: &Value| v.as_str() == Some("file");
    let has_file_type = match s.get("type") {
        Some(Value::Array(ts)) => ts.iter().any(is_file),
        Some(t) => is_file(t),
        None => false,
    };
    if has_file_type {
        diags.warning(join(ptr, "type"), "`type: file` is not valid in OpenAPI 3; using string + binary format");
        match s.get_mut("type") {
            Some(Value::Array(ts)) => ts.iter_mut().filter(|t| is_file(t)).for_each(|t| *t = json!("string")),
            _ => {
                s.insert("type".into(), json!("string"));
            }
        }
        s.entry("format").or_insert_with(|| json!("binary"));
    }
    // An unknown type name such as `type: application/json`.
    let known = |v: &Value| v.as_str().is_some_and(|t| KNOWN_TYPES.contains(&t));
    let bad = match s.get("type") {
        Some(Value::Array(ts)) => ts.iter().any(|t| !known(t)),
        Some(t) => !known(t),
        None => false,
    };
    if bad {
        diags.error(join(ptr, "type"), format!("unknown type {}: use one of {}", s["type"], KNOWN_TYPES.join(", ")));
    }
    // Draft-04 positional `items: [A, B]` to `prefixItems`.
    if matches!(s.get("items"), Some(Value::Array(_)))
        && let Some(items) = s.remove("items")
    {
        diags.warning(join(ptr, "items"), "array-form `items` is JSON Schema 2019-09 or older; using `prefixItems`");
        s.entry("prefixItems").or_insert(items);
    }
    // Up to 2019-09 `additionalItems` constrains the items after the
    // positional ones. That is `items` in 2020-12. Next to a schema-form `items` it was
    // ignored. So it is dropped.
    if let Some(extra) = s.remove("additionalItems")
        && s.contains_key("prefixItems")
        && !s.contains_key("items")
    {
        s.insert("items".into(), extra);
    }
    // Draft 7 `dependencies`, which 2019-09 split: a list of names is `dependentRequired`, a schema `dependentSchemas`.
    if let Some(Value::Object(deps)) = s.remove("dependencies") {
        diags.warning(
            join(ptr, "dependencies"),
            "`dependencies` is JSON Schema draft 7 or older; using `dependentRequired` and `dependentSchemas`",
        );
        for (name, dep) in deps {
            let key = if dep.is_array() { "dependentRequired" } else { "dependentSchemas" };
            if let Value::Object(m) = s.entry(key).or_insert_with(|| json!({})) {
                m.entry(name).or_insert(dep);
            }
        }
    }
    // Draft-04 boolean `exclusiveMinimum` / `exclusiveMaximum` mean nothing in
    // 2020-12 but still show up in 3.1 and 3.2 documents.
    for (key, bound) in [("exclusiveMinimum", "minimum"), ("exclusiveMaximum", "maximum")] {
        if s.get(key).is_some_and(Value::is_boolean) {
            diags.warning(
                join(ptr, key),
                format!("a boolean `{key}` is OpenAPI 3.0: it is read as the number of `{bound}` that it qualifies"),
            );
        }
    }
    super::v30_to_v31::exclusive(s, "exclusiveMinimum", "minimum");
    super::v30_to_v31::exclusive(s, "exclusiveMaximum", "maximum");
    // Keywords that must be arrays but are written as maps: use the values.
    for key in ["allOf", "anyOf", "oneOf", "examples"] {
        if let Some(Value::Object(m)) = s.get(key) {
            diags.warning(join(ptr, key), format!("`{key}` must be an array, found an object; using its values"));
            let values: Vec<Value> = m.values().cloned().collect();
            s.insert(key.into(), Value::Array(values));
        }
    }
}

/// Returns true when `prop` is provably not required. Unknown (a `$ref` in `allOf`) counts as required.
fn discriminator_maybe_optional(s: &Obj, prop: &str) -> bool {
    let requires = |o: &Obj| {
        o.get("required").and_then(Value::as_array).is_some_and(|r| r.iter().any(|v| v.as_str() == Some(prop)))
    };
    if requires(s) {
        return false;
    }
    if let Some(all) = s.get("allOf").and_then(Value::as_array) {
        for m in all.iter().filter_map(Value::as_object) {
            if m.contains_key("$ref") || requires(m) {
                return false;
            }
        }
    }
    // A pure oneOf/anyOf of refs: what it requires is up to its variants, which may all require it.
    let has_ref_variants = ["oneOf", "anyOf"]
        .iter()
        .filter_map(|k| s.get(*k).and_then(Value::as_array))
        .flatten()
        .any(|v| v.get("$ref").is_some());
    !has_ref_variants
}

fn collect_op_tags(root: &Obj, out: &mut BTreeSet<String>) {
    fn ops(v: &Value, out: &mut BTreeSet<String>) {
        let Some(item) = v.as_object() else { return };
        for (k, op) in item {
            let candidates: Vec<&Value> = if k == "additionalOperations" {
                op.as_object().map(|m| m.values().collect()).unwrap_or_default()
            } else {
                vec![op]
            };
            for op in candidates {
                if let Some(tags) = op.get("tags").and_then(Value::as_array) {
                    out.extend(json::strs(tags).map(String::from));
                }
            }
        }
    }
    for key in ["paths", "webhooks"] {
        if let Some(m) = root.get(key).and_then(Value::as_object) {
            m.values().for_each(|item| ops(item, out));
        }
    }
}

/// Turns `x-tagGroups: [{name, tags: [..]}]` into parent tags. Bails out (changes
/// nothing) on any ambiguity. The extension is kept.
fn tag_groups(root: &mut Obj, op_tags: &BTreeSet<String>) {
    let Some(Value::Array(groups)) = root.get("x-tagGroups") else { return };
    if groups.is_empty() {
        return;
    }
    let tags: Vec<Value> = match root.get("tags") {
        None => vec![],
        Some(Value::Array(t)) => t.clone(),
        Some(_) => return,
    };
    if !tags.iter().all(|t| t.get("name").and_then(Value::as_str).is_some()) {
        return;
    }
    let name_of = |t: &Value| t["name"].as_str().unwrap_or_default().to_owned();
    let existing: Vec<String> = tags.iter().map(name_of).collect();
    let mut parents: Vec<(String, Vec<String>)> = vec![];
    let mut children = BTreeSet::new();
    for g in groups {
        let Some(g) = g.as_object() else { return };
        let Some(name) = g.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()) else { return };
        let Some(members) = g.get("tags").and_then(Value::as_array) else { return };
        if g.keys().any(|k| k != "name" && k != "tags")
            || parents.iter().any(|(p, _)| p == name)
            || existing.iter().any(|e| e == name)
            || op_tags.contains(name)
        {
            return;
        }
        let mut names = vec![];
        for m in members {
            let Some(m) = m.as_str() else { return };
            let already_parented = tags.iter().any(|t| t["name"] == m && t.get("parent").is_some());
            if !children.insert(m.to_owned()) || already_parented {
                return;
            }
            names.push(m.to_owned());
        }
        parents.push((name.to_owned(), names));
    }
    if parents.iter().any(|(p, _)| children.contains(p)) {
        return;
    }
    let mut out: Vec<Value> = vec![];
    for (parent, members) in &parents {
        out.push(json!({"name": parent}));
        for m in members {
            let mut tag = tags.iter().find(|t| t["name"] == m.as_str()).cloned().unwrap_or_else(|| json!({"name": m}));
            tag["parent"] = json!(parent);
            out.push(tag);
        }
    }
    out.extend(tags.into_iter().filter(|t| !children.contains(&name_of(t))));
    root.insert("tags".into(), Value::Array(out));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(doc: Value) -> (Value, Diagnostics) {
        let mut d = Diagnostics::default();
        (upgrade(doc, &mut d), d)
    }

    /// Every registered `x-oai-*` extension that a 3.1 document can carry on the object that the registry names.
    #[test]
    fn nullable_in_a_3_1_document_is_reported_and_has_no_effect() {
        let (doc, d) = up(json!({
            "openapi": "3.1.0",
            "components": {"schemas": {"Page": {"properties": {"iterator": {"type": "string", "nullable": true}}}}}
        }));
        let warned: Vec<&str> = d.iter().map(|x| x.pointer.as_str()).collect();
        assert_eq!(warned, ["/components/schemas/Page/properties/iterator/nullable"]);
        assert_eq!(doc["components"]["schemas"]["Page"]["properties"]["iterator"]["type"], "string");
    }

    /// A `type` list repairs as a single `type` does: `file` becomes a binary string, an unknown name is an error, and a
    /// list of known names stays without a word.
    #[test]
    fn a_type_list_is_repaired_like_one_type() {
        let (doc, d) = up(json!({"openapi": "3.1.0", "components": {"schemas": {
            "F": {"type": ["file", "null"]},
            "K": {"type": ["string", "null"]},
            "U": {"type": ["string", "application/json"]},
        }}}));
        let warned: Vec<&str> = d.iter().map(|x| x.pointer.as_str()).collect();
        assert_eq!(warned, ["/components/schemas/F/type", "/components/schemas/U/type"]);
        assert_eq!(doc["components"]["schemas"]["F"], json!({"type": ["string", "null"], "format": "binary"}));
        assert_eq!(d.iter().last().map(|x| x.level), Some(crate::diag::Level::Error));
    }

    #[test]
    fn registered_extensions_become_their_fields() {
        let (doc, d) = up(json!({
            "openapi": "3.1.1", "x-oai-$self": "https://example.com/api",
            "servers": [{"url": "https://a", "x-oai-name": "prod"}],
            "paths": {"/docs/{id}": {
                "x-oai-additionalOperations": {"LINK": {"responses": {"204": {"description": "ok"}}}},
                "get": {"responses": {"200": {"description": "ok", "x-oai-summary": "Found", "content": {
                    "text/event-stream": {"x-oai-itemSchema": {"type": "object"}},
                    "multipart/mixed": {
                        "schema": {"type": "array"},
                        "x-oai-prefixEncoding": [{"contentType": "application/json"}],
                        "x-oai-itemEncoding": {"contentType": "multipart/mixed", "x-oai-encoding": {"a": {"contentType": "text/plain"}}},
                        "examples": {"one": {"x-oai-dataValue": [1], "x-oai-serializedValue": "1"}}
                    }
                }}}}
            }},
            "components": {
                "schemas": {"Odd": {"type": "object", "properties": {"x-oai-name": {"type": "string"}}}},
                "securitySchemes": {"tv": {"type": "oauth2", "x-oai-deprecated": true, "flows": {
                    "x-oai-deviceAuthorization": {"x-oai-deviceAuthorizationUrl": "https://d", "tokenUrl": "https://t", "scopes": {}}
                }}}
            }
        }));
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(doc["$self"], "https://example.com/api");
        assert_eq!(doc["servers"][0]["name"], "prod");
        let item = &doc["paths"]["/docs/{id}"];
        assert!(item["additionalOperations"]["LINK"].is_object());
        let ok = &item["get"]["responses"]["200"];
        assert_eq!(ok["summary"], "Found");
        assert_eq!(ok["content"]["text/event-stream"]["itemSchema"], json!({"type": "object"}));
        let mixed = &ok["content"]["multipart/mixed"];
        assert_eq!(mixed["prefixEncoding"][0]["contentType"], "application/json");
        assert_eq!(mixed["itemEncoding"]["encoding"]["a"]["contentType"], "text/plain");
        assert_eq!(mixed["examples"]["one"], json!({"dataValue": [1], "serializedValue": "1"}));
        let tv = &doc["components"]["securitySchemes"]["tv"];
        assert_eq!(tv["deprecated"], true);
        assert_eq!(tv["flows"]["deviceAuthorization"]["deviceAuthorizationUrl"], "https://d");
        // A property named like an extension is a property and not an extension.
        assert!(doc["components"]["schemas"]["Odd"]["properties"]["x-oai-name"].is_object());
    }

    #[test]
    fn a_native_field_wins_over_its_extension() {
        let (doc, d) =
            up(json!({"openapi": "3.1.1", "servers": [{"url": "https://a", "name": "real", "x-oai-name": "ext"}]}));
        assert_eq!(doc["servers"][0], json!({"url": "https://a", "name": "real"}));
        let warned: Vec<String> = d.iter().map(|x| format!("{} {}", x.pointer, x.message)).collect();
        assert_eq!(warned, ["/servers/0/x-oai-name `x-oai-name` is dropped: `name` is set too"]);
    }

    #[test]
    fn xml_flags_become_node_type() {
        let (doc, _) = up(json!({"openapi": "3.1.0", "components": {"schemas": {
            "A": {"xml": {"attribute": true}}, "B": {"xml": {"wrapped": true, "name": "b"}}, "C": {"xml": {"wrapped": false}}
        }}}));
        let s = &doc["components"]["schemas"];
        assert_eq!(s["A"]["xml"], json!({"nodeType": "attribute"}));
        assert_eq!(s["B"]["xml"], json!({"name": "b", "nodeType": "element"}));
        assert_eq!(s["C"]["xml"], json!({}));
        assert_eq!(doc["openapi"], "3.2.0");
    }

    #[test]
    fn inactive_allow_reserved_is_dropped() {
        let (doc, _) = up(json!({"openapi": "3.1.0", "paths": {"/a/{p}": {"parameters": [
            {"name": "p", "in": "path", "allowReserved": true},
            {"name": "q", "in": "query", "allowReserved": true}
        ]}}}));
        let ps = &doc["paths"]["/a/{p}"]["parameters"];
        assert!(ps[0].get("allowReserved").is_none());
        assert_eq!(ps[1]["allowReserved"], true);
    }

    #[test]
    fn optional_discriminator_warns() {
        let (_, d) = up(json!({"openapi": "3.1.0", "components": {"schemas": {"Product": {
            "properties": {"kind": {"type": "string"}},
            "oneOf": [{"properties": {"a": {}}}, {"properties": {"b": {}}}],
            "discriminator": {"propertyName": "kind"}
        }}}}));
        assert_eq!(d.0.len(), 1);
        assert_eq!(d.0[0].pointer, "/components/schemas/Product/discriminator");
    }

    #[test]
    fn tag_groups_become_parents() {
        let (doc, _) = up(json!({
            "openapi": "3.1.0",
            "tags": [{"name": "gadgets", "description": "Gadgets"}, {"name": "misc"}],
            "x-tagGroups": [{"name": "Catalog", "tags": ["gadgets", "books"]}],
            "paths": {"/c": {"get": {"tags": ["gadgets"]}}}
        }));
        assert_eq!(
            doc["tags"],
            json!([
                {"name": "Catalog"},
                {"name": "gadgets", "description": "Gadgets", "parent": "Catalog"},
                {"name": "books", "parent": "Catalog"},
                {"name": "misc"}
            ])
        );
        assert!(doc.get("x-tagGroups").is_some());
    }

    /// A group that cannot become a parent leaves the tags as they are: it has a key of its own, it refers to an
    /// operation's tag, or a member has a parent already.
    #[test]
    fn unsafe_tag_groups_change_nothing() {
        let cases = [
            (json!([{"name": "gadgets"}]), json!([{"name": "A", "tags": ["gadgets"], "x-icon": "a"}])),
            (json!([{"name": "gadgets"}]), json!([{"name": "used", "tags": ["gadgets"]}])),
            (
                json!([{"name": "gadgets", "parent": "misc"}, {"name": "misc"}]),
                json!([{"name": "A", "tags": ["gadgets"]}]),
            ),
            (
                json!([{"name": "a"}, {"name": "b"}]),
                json!([{"name": "A", "tags": ["a"]}, {"name": "A", "tags": ["b"]}]),
            ),
            (json!([{"name": "a"}, {"name": "A"}]), json!([{"name": "A", "tags": ["a"]}])),
        ];
        for (tags, groups) in cases {
            let input = json!({"openapi": "3.1.0", "tags": tags, "x-tagGroups": groups,
                "paths": {"/c": {"get": {"tags": ["used"]}}}});
            let (doc, _) = up(input.clone());
            assert_eq!(doc["tags"], input["tags"], "{groups}");
        }
    }

    /// `attribute` alone, or `wrapped` alone, is no conflict: only both together warn.
    #[test]
    fn one_of_attribute_and_wrapped_is_no_conflict() {
        for xml in [json!({"attribute": true}), json!({"wrapped": true})] {
            let (_, d) =
                up(json!({"openapi": "3.1.0", "components": {"schemas": {"A": {"type": "string", "xml": xml}}}}));
            assert!(d.is_empty(), "{xml}: {d:?}");
        }
    }

    #[test]
    fn ambiguous_tag_groups_change_nothing() {
        let input = json!({
            "openapi": "3.1.0",
            "tags": [{"name": "gadgets"}],
            "x-tagGroups": [{"name": "A", "tags": ["gadgets"]}, {"name": "B", "tags": ["gadgets"]}]
        });
        let (doc, _) = up(input.clone());
        assert_eq!(doc["tags"], input["tags"]);
    }
}
