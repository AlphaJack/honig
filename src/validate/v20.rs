//! Swagger 2.0 validation on the document as written.
//!
//! There is no typed 2.0 model: generation upgrades to 3.2 first. Each rule is
//! a MUST or a **Required** field of the 2.0 specification; pointers point
//! into the original document.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::diag::Diagnostics;
use crate::json;
use crate::json::{is_data, join, pointer};

const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch"];
const SCHEMES: &[&str] = &["http", "https", "ws", "wss"];
const LOCATIONS: &[&str] = &["query", "header", "path", "formData", "body"];
const PARAM_TYPES: &[&str] = &["string", "number", "integer", "boolean", "array", "file"];
const ITEM_TYPES: &[&str] = &["string", "number", "integer", "boolean", "array"];
const COLLECTION_FORMATS: &[&str] = &["csv", "ssv", "tsv", "pipes", "multi"];

/// Fixed fields per object for "unknown field" errors (`x-` is always allowed).
const ROOT: &[&str] = &[
    "swagger",
    "info",
    "host",
    "basePath",
    "schemes",
    "consumes",
    "produces",
    "paths",
    "definitions",
    "parameters",
    "responses",
    "securityDefinitions",
    "security",
    "tags",
    "externalDocs",
];
const INFO: &[&str] = &["title", "description", "termsOfService", "contact", "license", "version"];
const PATH_ITEM: &[&str] = &["$ref", "get", "put", "post", "delete", "options", "head", "patch", "parameters"];
const OPERATION: &[&str] = &[
    "tags",
    "summary",
    "description",
    "externalDocs",
    "operationId",
    "consumes",
    "produces",
    "parameters",
    "responses",
    "schemes",
    "deprecated",
    "security",
];
const RESPONSE: &[&str] = &["description", "schema", "headers", "examples"];

/// Validates a Swagger 2.0 document.
pub(crate) fn validate(doc: &Value, diags: &mut Diagnostics) {
    let mut v = V { root: doc, diags, operation_ids: BTreeMap::new() };
    v.run();
}

struct V<'a> {
    root: &'a Value,
    diags: &'a mut Diagnostics,
    operation_ids: BTreeMap<String, String>,
}

impl V<'_> {
    fn run(&mut self) {
        let Some(root) = self.root.as_object() else { return };
        self.fields(root, "", ROOT);
        match root.get("info") {
            Some(Value::Object(info)) => {
                self.fields(info, "/info", INFO);
                for k in ["title", "version"] {
                    self.required_str(info, "/info", k);
                }
                if let Some(Value::Object(l)) = info.get("license") {
                    self.required_str(l, "/info/license", "name");
                }
            }
            _ => self.diags.error("", "`info` is required"),
        }
        if let Some(host) = root.get("host").and_then(Value::as_str)
            && (host.contains("://") || host.contains('/'))
        {
            self.diags.error("/host", format!("`host` must be the host only, without scheme or path: `{host}`"));
        }
        if let Some(base) = root.get("basePath").and_then(Value::as_str)
            && !base.starts_with('/')
        {
            self.diags.error("/basePath", "`basePath` must start with `/`");
        }
        self.schemes(root.get("schemes"), "/schemes");
        self.security_definitions(root.get("securityDefinitions"));
        self.security(root.get("security"), "/security");
        self.tags(root.get("tags"));
        let consumes = root.get("consumes");
        match root.get("paths") {
            Some(Value::Object(paths)) => {
                for (path, item) in paths {
                    if path.starts_with("x-") {
                        continue;
                    }
                    let ptr = join("/paths", path);
                    if !path.starts_with('/') {
                        self.diags.error(&ptr, "a path must start with `/`");
                    }
                    self.path_item(path, item, &ptr, consumes);
                }
            }
            _ => self.diags.error("", "`paths` is required"),
        }
        for (name, p) in json::members(self.root, "parameters") {
            self.parameter(p, &join("/parameters", name));
        }
        for (name, r) in json::members(self.root, "responses") {
            self.response(r, &join("/responses", name));
        }
        for (name, s) in json::members(self.root, "definitions") {
            self.schema(s, &join("/definitions", name));
        }
        self.refs(self.root, "", "");
    }

    /// Checks for unknown fixed fields: 2.0 allows only its own and `x-` ones.
    fn fields(&mut self, o: &Map<String, Value>, ptr: &str, known: &[&str]) {
        for k in o.keys().filter(|k| !k.starts_with("x-") && !known.contains(&k.as_str())) {
            self.diags.refuse_server(join(ptr, k), format!("unknown field `{k}`"));
        }
    }

    fn required_str(&mut self, o: &Map<String, Value>, ptr: &str, key: &str) {
        if !o.get(key).is_some_and(Value::is_string) {
            self.diags.error(ptr, format!("`{key}` is required"));
        }
    }

    fn schemes(&mut self, v: Option<&Value>, ptr: &str) {
        for (i, s) in json::items(v) {
            if !s.as_str().is_some_and(|s| SCHEMES.contains(&s)) {
                self.diags.error(format!("{ptr}/{i}"), format!("scheme {s} must be one of http, https, ws, wss"));
            }
        }
    }

    fn security_definitions(&mut self, v: Option<&Value>) {
        let Some(Value::Object(defs)) = v else { return };
        for (name, d) in defs {
            let ptr = join("/securityDefinitions", name);
            let Some(d) = d.as_object() else { continue };
            let s = |k: &str| d.get(k).and_then(Value::as_str);
            match s("type") {
                Some("basic") => {}
                Some("apiKey") => {
                    if s("name").is_none() {
                        self.diags.error(&ptr, "an `apiKey` scheme needs `name`");
                    }
                    if !matches!(s("in"), Some("query" | "header")) {
                        self.diags.error(join(&ptr, "in"), "an `apiKey` scheme needs `in`: `query` or `header`");
                    }
                }
                Some("oauth2") => {
                    let flow = s("flow");
                    if !matches!(flow, Some("implicit" | "password" | "application" | "accessCode")) {
                        self.diags.error(
                            join(&ptr, "flow"),
                            "an `oauth2` scheme needs `flow`: `implicit`, `password`, `application` or `accessCode`",
                        );
                    }
                    if matches!(flow, Some("implicit" | "accessCode")) && s("authorizationUrl").is_none() {
                        self.diags.error(&ptr, "this flow needs `authorizationUrl`");
                    }
                    if matches!(flow, Some("password" | "application" | "accessCode")) && s("tokenUrl").is_none() {
                        self.diags.error(&ptr, "this flow needs `tokenUrl`");
                    }
                    if !d.get("scopes").is_some_and(Value::is_object) {
                        self.diags.error(&ptr, "an `oauth2` scheme needs `scopes`");
                    }
                }
                _ => self.diags.error(join(&ptr, "type"), "`type` must be `basic`, `apiKey` or `oauth2`"),
            }
        }
    }

    /// Checks that each name is a declared scheme and that only `oauth2` ones list scopes.
    fn security(&mut self, v: Option<&Value>, ptr: &str) {
        let defs = self.root.get("securityDefinitions").and_then(Value::as_object);
        for (i, req) in json::items(v) {
            for (name, scopes) in req.as_object().into_iter().flatten() {
                let at = format!("{ptr}/{i}");
                match defs.and_then(|d| d.get(name)) {
                    None => self.diags.error(at, format!("security scheme `{name}` is not defined")),
                    Some(d) => {
                        let oauth = d.get("type").and_then(Value::as_str) == Some("oauth2");
                        if !oauth && scopes.as_array().is_some_and(|s| !s.is_empty()) {
                            self.diags.error(
                                join(&at, name),
                                format!("`{name}` is not `oauth2`: its scope list must be empty"),
                            );
                        }
                    }
                }
            }
        }
    }

    fn tags(&mut self, v: Option<&Value>) {
        let mut seen = BTreeSet::new();
        for (i, t) in json::items(v) {
            match t.get("name").and_then(Value::as_str) {
                Some(n) if !seen.insert(n) => self.diags.error(format!("/tags/{i}"), format!("duplicate tag `{n}`")),
                Some(_) => {}
                None => self.diags.error(format!("/tags/{i}"), "`name` is required"),
            }
        }
    }

    fn path_item(&mut self, path: &str, item: &Value, ptr: &str, consumes: Option<&Value>) {
        let Some(item) = item.as_object() else { return };
        self.fields(item, ptr, PATH_ITEM);
        let shared = self.params_of(item.get("parameters"), &join(ptr, "parameters"));
        for m in METHODS {
            let Some(Value::Object(op)) = item.get(*m) else { continue };
            let optr = join(ptr, m);
            self.fields(op, &optr, OPERATION);
            if let Some(id) = op.get("operationId").and_then(Value::as_str)
                && let Some(prev) = self.operation_ids.insert(id.to_owned(), optr.clone())
            {
                self.diags.error(join(&optr, "operationId"), format!("operationId `{id}` is also used at {prev}"));
            }
            self.schemes(op.get("schemes"), &join(&optr, "schemes"));
            self.security(op.get("security"), &join(&optr, "security"));
            match op.get("responses") {
                Some(Value::Object(rs)) if rs.keys().any(|k| !k.starts_with("x-")) => {
                    for (code, r) in rs.iter().filter(|(k, _)| !k.starts_with("x-")) {
                        let rptr = join(&join(&optr, "responses"), code);
                        if code != "default"
                            && !(code.len() == crate::validate::STATUS_DIGITS
                                && code.bytes().all(|b| b.is_ascii_digit()))
                        {
                            self.diags.error(&rptr, format!("`{code}` is not a status code or `default`"));
                        }
                        self.response(r, &rptr);
                    }
                }
                _ => self.diags.error(&optr, "`responses` needs at least one response"),
            }
            let own = self.params_of(op.get("parameters"), &join(&optr, "parameters"));
            // Operation parameters override path-level ones (name + in).
            let mut all: Vec<(Map<String, Value>, String)> = own.clone();
            for (p, pptr) in &shared {
                let key = |p: &Map<String, Value>| (p.get("name").cloned(), p.get("in").cloned());
                if !own.iter().any(|(o, _)| key(o) == key(p)) {
                    all.push((p.clone(), pptr.clone()));
                }
            }
            self.operation_params(path, &all, &own, &optr, op.get("consumes").or(consumes));
        }
    }

    /// Parameters of a list with `$ref`s resolved. Each is validated on its own.
    fn params_of(&mut self, v: Option<&Value>, ptr: &str) -> Vec<(Map<String, Value>, String)> {
        let mut out = vec![];
        for (i, p) in json::items(v) {
            let pptr = format!("{ptr}/{i}");
            let resolved = match p.get("$ref").and_then(Value::as_str) {
                Some(r) => r.strip_prefix('#').and_then(|frag| pointer(self.root, frag)).cloned(),
                None => {
                    self.parameter(p, &pptr);
                    Some(p.clone())
                }
            };
            if let Some(Value::Object(o)) = resolved {
                out.push((o, pptr));
            }
        }
        out
    }

    /// Checks the rules across one operation's parameters (own and inherited).
    fn operation_params(
        &mut self,
        path: &str,
        all: &[(Map<String, Value>, String)],
        own: &[(Map<String, Value>, String)],
        optr: &str,
        consumes: Option<&Value>,
    ) {
        let s = |p: &Map<String, Value>, k: &str| json::string(p, k);
        let mut seen = BTreeSet::new();
        for (p, pptr) in own {
            if !seen.insert((s(p, "name"), s(p, "in"))) {
                self.diags.error(pptr, "duplicate parameter (same `name` and `in`)");
            }
        }
        let bodies = all.iter().filter(|(p, _)| s(p, "in").as_deref() == Some("body")).count();
        if bodies > 1 {
            self.diags.error(optr, "an operation has at most one `body` parameter");
        }
        let form = all.iter().any(|(p, _)| s(p, "in").as_deref() == Some("formData"));
        if bodies > 0 && form {
            self.diags.error(optr, "`body` and `formData` parameters cannot be combined");
        }
        let vars: BTreeSet<&str> = path.split('{').skip(1).filter_map(|r| r.split_once('}')).map(|(v, _)| v).collect();
        let path_params: BTreeSet<String> = all
            .iter()
            .filter(|(p, _)| s(p, "in").as_deref() == Some("path"))
            .filter_map(|(p, _)| s(p, "name"))
            .collect();
        for (p, pptr) in all {
            if s(p, "in").as_deref() == Some("path")
                && let Some(n) = s(p, "name")
                && !vars.contains(n.as_str())
            {
                self.diags.error(pptr, format!("path parameter `{n}` does not appear in the path"));
            }
        }
        for v in vars.iter().filter(|v| !path_params.contains(**v)) {
            self.diags.error(optr, format!("path variable `{v}` has no `in: path` parameter"));
        }
        // A `file` parameter needs a form media type to travel in.
        let file = all.iter().any(|(p, _)| s(p, "type").as_deref() == Some("file"));
        let form_media = consumes.and_then(Value::as_array).is_some_and(|c| {
            json::strs(c).any(|m| {
                let m = m.split(';').next().unwrap_or("").trim();
                m == "multipart/form-data" || m == "application/x-www-form-urlencoded"
            })
        });
        if file && !form_media {
            self.diags.error(
                optr,
                "a `type: file` parameter needs `consumes` with `multipart/form-data` or `application/x-www-form-urlencoded`",
            );
        }
    }

    fn parameter(&mut self, p: &Value, ptr: &str) {
        let Some(p) = p.as_object() else { return };
        let s = |k: &str| p.get(k).and_then(Value::as_str);
        if s("name").is_none() {
            self.diags.error(ptr, "`name` is required");
        }
        let Some(at) = s("in").filter(|i| LOCATIONS.contains(i)) else {
            self.diags.error(join(ptr, "in"), "`in` must be `query`, `header`, `path`, `formData` or `body`");
            return;
        };
        if at == "path" && p.get("required") != Some(&Value::Bool(true)) {
            self.diags.error(join(ptr, "required"), "path parameters must have `required: true`");
        }
        if at == "body" {
            match p.get("schema") {
                Some(schema) => self.schema(schema, &join(ptr, "schema")),
                None => self.diags.error(ptr, "a `body` parameter needs `schema`"),
            }
            return;
        }
        match s("type") {
            Some(t) if PARAM_TYPES.contains(&t) => {
                if t == "file" && at != "formData" {
                    self.diags.error(join(ptr, "type"), "`type: file` is only for `formData` parameters");
                }
                if t == "array" {
                    self.items(p.get("items"), &join(ptr, "items"));
                }
            }
            _ => self
                .diags
                .error(join(ptr, "type"), "`type` must be `string`, `number`, `integer`, `boolean`, `array` or `file`"),
        }
        if let Some(cf) = s("collectionFormat") {
            if !COLLECTION_FORMATS.contains(&cf) {
                self.diags.error(join(ptr, "collectionFormat"), format!("unknown collectionFormat `{cf}`"));
            } else if cf == "multi" && !matches!(at, "query" | "formData") {
                self.diags.error(join(ptr, "collectionFormat"), "`multi` is only for `query` and `formData`");
            }
        }
        if p.contains_key("allowEmptyValue") && !matches!(at, "query" | "formData") {
            self.diags.error(join(ptr, "allowEmptyValue"), "`allowEmptyValue` is only for `query` and `formData`");
        }
    }

    /// Checks an Items Object (and Header Object): a simple type or `array` with `items`.
    fn items(&mut self, v: Option<&Value>, ptr: &str) {
        let Some(Value::Object(i)) = v else {
            self.diags.error(ptr, "`type: array` needs `items`");
            return;
        };
        match i.get("type").and_then(Value::as_str) {
            Some("array") => self.items(i.get("items"), &join(ptr, "items")),
            Some(t) if ITEM_TYPES.contains(&t) => {}
            _ => self
                .diags
                .error(join(ptr, "type"), "`type` must be `string`, `number`, `integer`, `boolean` or `array`"),
        }
    }

    fn response(&mut self, r: &Value, ptr: &str) {
        let Some(r) = r.as_object() else { return };
        if r.contains_key("$ref") {
            return;
        }
        self.fields(r, ptr, RESPONSE);
        if !r.get("description").is_some_and(Value::is_string) {
            self.diags.error(ptr, "`description` is required");
        }
        if let Some(schema) = r.get("schema") {
            self.schema(schema, &join(ptr, "schema"));
        }
        for (name, h) in r.get("headers").and_then(Value::as_object).into_iter().flatten() {
            self.items(Some(h), &join(&join(ptr, "headers"), name));
        }
    }

    /// Checks the 2.0 `discriminator`. It is a property of this schema and required.
    fn schema(&mut self, s: &Value, ptr: &str) {
        let Some(o) = s.as_object() else { return };
        if let Some(d) = o.get("discriminator").and_then(Value::as_str) {
            let declared = json::object(o, "properties").is_some_and(|p| p.contains_key(d));
            let required =
                o.get("required").and_then(Value::as_array).is_some_and(|r| r.iter().any(|x| x.as_str() == Some(d)));
            // A MUST. But the official 2.0 schema cannot check it and many documents break it (mostly not in
            // `required`).
            if !declared || !required {
                self.diags.warning(
                    join(ptr, "discriminator"),
                    format!("discriminator `{d}` must be a property of this schema and in `required`"),
                );
            }
        }
    }

    /// Checks that every local `$ref` resolves.
    fn refs(&mut self, v: &Value, ptr: &str, parent: &str) {
        match v {
            Value::Object(o) => {
                if let Some(r) = o.get("$ref").and_then(Value::as_str)
                    && let Some(frag) = r.strip_prefix('#')
                    && pointer(self.root, frag).is_none()
                {
                    self.diags.error(join(ptr, "$ref"), format!("`{r}` does not resolve"));
                }
                for (k, c) in o {
                    if !is_data(parent, k) {
                        self.refs(c, &join(ptr, k), k);
                    }
                }
            }
            Value::Array(a) => {
                for (i, c) in a.iter().enumerate() {
                    self.refs(c, &format!("{ptr}/{i}"), parent);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
}
