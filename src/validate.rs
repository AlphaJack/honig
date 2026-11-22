//! Validation of a document as written under its own version. Every rule records a diagnostic and continues.
//!
//! A 2.0 document is checked as written ([`v20`]). A 3.x document is upgraded to 3.2 and checked there ([`v32`]),
//! after [`newer_fields`] reports what its own version does not define.

pub(crate) mod default_value;
pub(crate) mod enum_type;
pub(crate) mod overlap;
pub(crate) mod schema_rules;
pub(crate) mod unsatisfiable;
pub(crate) mod v20;
pub(crate) mod v32;

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json;

/// The digits of an HTTP status code and of a range like `4XX`.
pub(crate) const STATUS_DIGITS: usize = "200".len();
use crate::json::join;
use crate::version::Version;

/// Reports fields that a 3.x document uses and that its own version does not define, with
/// pointers into that document. (The upgrades from 3.0 through 3.2 keep every location.)
pub(crate) fn newer_fields(original: &Value, version: Version, diags: &mut Diagnostics) {
    if version == Version::V32 || version == Version::V20 {
        return;
    }
    let in_31: &[(&str, &[&str])] = &[
        ("", &["webhooks", "jsonSchemaDialect"]),
        ("info", &["summary"]),
        ("license", &["identifier"]),
        ("components", &["pathItems"]),
    ];
    let in_32: &[(&str, &[&str])] = &[
        ("", &["$self"]),
        ("pathItem", &["query", "additionalOperations"]),
        ("mediaType", &["itemSchema", "prefixEncoding", "itemEncoding", "description"]),
        ("tag", &["parent", "kind", "summary"]),
        ("server", &["name"]),
        ("response", &["summary"]),
        ("example", &["dataValue", "serializedValue"]),
        ("components", &["mediaTypes"]),
        ("securityScheme", &["deprecated", "oauth2MetadataUrl"]),
        ("flows", &["deviceAuthorization"]),
        ("discriminator", &["defaultMapping"]),
        ("xml", &["nodeType"]),
    ];
    let name = if version == Version::V30 { "3.0" } else { "3.1" };
    let mut sets: Vec<(&str, &str, &str)> = vec![];
    if version == Version::V30 {
        sets.extend(in_31.iter().flat_map(|(k, fs)| fs.iter().map(move |f| (*k, *f, "3.1"))));
    }
    sets.extend(in_32.iter().flat_map(|(k, fs)| fs.iter().map(move |f| (*k, *f, "3.2"))));
    let mut check = |kind: &str, v: &Value, ptr: &str| {
        let Some(o) = v.as_object() else { return };
        for (k, f, since) in &sets {
            if *k == kind && o.contains_key(*f) {
                diags.error(join(ptr, f), format!("`{f}` is OpenAPI {since}; this document is {name}"));
            }
        }
    };
    walk_kinds(original, &mut check);
    // Values that only 3.2 defines.
    walk_values(original, "", &mut |ptr, key, v| match (key, v.as_str()) {
        ("in", Some("querystring")) | ("style", Some("cookie")) => {
            diags.error(ptr, format!("`{key}: {}` is OpenAPI 3.2; this document is {name}", v.as_str().unwrap_or("")));
        }
        _ => {}
    });
}

type KindFn<'a> = dyn FnMut(&str, &Value, &str) + 'a;

/// Calls `f(kind, object, pointer)` for the objects `newer_fields` knows.
fn walk_kinds(root: &Value, f: &mut KindFn<'_>) {
    f("", root, "");
    if let Some(info) = root.get("info") {
        f("info", info, "/info");
        if let Some(l) = info.get("license") {
            f("license", l, "/info/license");
        }
    }
    for (i, t) in json::items(root.get("tags")) {
        f("tag", t, &format!("/tags/{i}"));
    }
    kind_servers(root, "", f);
    for key in ["paths", "webhooks"] {
        for (p, item) in root.get(key).and_then(Value::as_object).into_iter().flatten() {
            kind_path_item(item, &join(&format!("/{key}"), p), f);
        }
    }
    if let Some(c) = root.get("components") {
        kind_components(c, f);
    }
    // Schema-level objects anywhere: discriminators and xml.
    walk_values(root, "", &mut |ptr, key, v| match key {
        "discriminator" => f("discriminator", v, ptr),
        "xml" => f("xml", v, ptr),
        _ => {}
    });
}

fn kind_servers(v: &Value, ptr: &str, f: &mut KindFn<'_>) {
    for (i, s) in json::items(v.get("servers")) {
        f("server", s, &format!("{ptr}/servers/{i}"));
    }
}

fn kind_content(v: &Value, ptr: &str, f: &mut KindFn<'_>) {
    for (m, mt) in json::members(v, "content") {
        let p = join(&join(ptr, "content"), m);
        f("mediaType", mt, &p);
        for (e, ex) in json::members(mt, "examples") {
            f("example", ex, &join(&join(&p, "examples"), e));
        }
    }
}

fn kind_response(r: &Value, ptr: &str, f: &mut KindFn<'_>) {
    f("response", r, ptr);
    kind_content(r, ptr, f);
}

fn kind_operation(op: &Value, ptr: &str, f: &mut KindFn<'_>) {
    kind_servers(op, ptr, f);
    if let Some(b) = op.get("requestBody") {
        kind_content(b, &join(ptr, "requestBody"), f);
    }
    for (code, r) in json::members(op, "responses") {
        kind_response(r, &join(&join(ptr, "responses"), code), f);
    }
    for (i, p) in json::items(op.get("parameters")) {
        let pp = format!("{ptr}/parameters/{i}");
        kind_content(p, &pp, f);
        for (e, ex) in json::members(p, "examples") {
            f("example", ex, &join(&join(&pp, "examples"), e));
        }
    }
}

fn kind_path_item(item: &Value, ptr: &str, f: &mut KindFn<'_>) {
    f("pathItem", item, ptr);
    kind_servers(item, ptr, f);
    for m in FIXED_METHODS {
        if let Some(op) = item.get(*m) {
            kind_operation(op, &join(ptr, m), f);
        }
    }
}

fn kind_components(c: &Value, f: &mut KindFn<'_>) {
    f("components", c, "/components");
    for (n, r) in json::members(c, "responses") {
        kind_response(r, &join("/components/responses", n), f);
    }
    for (n, b) in json::members(c, "requestBodies") {
        kind_content(b, &join("/components/requestBodies", n), f);
    }
    for (n, e) in json::members(c, "examples") {
        f("example", e, &join("/components/examples", n));
    }
    for (n, s) in json::members(c, "securitySchemes") {
        let p = join("/components/securitySchemes", n);
        f("securityScheme", s, &p);
        if let Some(flows) = s.get("flows") {
            f("flows", flows, &join(&p, "flows"));
        }
    }
    for (n, item) in json::members(c, "pathItems") {
        kind_path_item(item, &join("/components/pathItems", n), f);
    }
}

/// Calls `f(pointer, key, value)` for every object member depth first.
fn walk_values(v: &Value, ptr: &str, f: &mut dyn FnMut(&str, &str, &Value)) {
    match v {
        Value::Object(o) => {
            for (k, c) in o {
                let p = join(ptr, k);
                f(&p, k, c);
                walk_values(c, &p, f);
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                walk_values(c, &format!("{ptr}/{i}"), f);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// The HTTP methods a Path Item has a field for; others go in `additionalOperations` (3.2).
pub(crate) const FIXED_METHODS: &[&str] =
    &["get", "put", "post", "delete", "options", "head", "patch", "trace", "query"];
