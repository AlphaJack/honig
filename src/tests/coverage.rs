//! Every field of the `OpenAPI` model either changes the generated code or
//! is listed in [`NOT_EMITTED`] with the reason.
//!
//! A client and server that drop a field alike still agree. Only this test sees a
//! field that the generator ignores. For each field of
//! each object kind that the fixtures use it removes that field from every object
//! of that kind and regenerates. The code must change. It also fails when the
//! model gains a field that no fixture uses and no entry explains.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value};

/// Fields that do not change the generated code by object kind and why.
/// `gap:` marks work to do. `by design:` marks fields with nothing to emit.
const NOT_EMITTED: &[(&str, &str, &str)] = &[
    // Nothing to emit.
    ("Header", "style", "by design: `simple` is the only header style"),
    ("Example", "summary", "by design: examples become test values, which carry no docs"),
    ("Example", "description", "by design: examples become test values, which carry no docs"),
    ("OpenApi", "jsonSchemaDialect", "by design: an unsupported dialect is a generation warning"),
    ("OpenApi", "openapi", "by design: the version picks the upgrade at load; every later pass reads 3.2"),
    ("OpenApi", "$self", "by design: the base URI resolves `$ref`s at load; the loaded document has local ones"),
    ("SecurityScheme", "bearerFormat", "by design: generated docs hold only the description's own prose"),
    ("Tag", "kind", "by design: generated docs hold only the description's own prose"),
    ("Schema", "example", "by design: an annotation next to a `$ref`, which checks nothing"),
    ("Schema", "$comment", "by design: a note for the authors of the schema"),
    ("Schema", "$schema", "by design: a schema has the dialect of its OpenAPI version"),
    ("Schema", "$vocabulary", "by design: generated code checks the vocabularies of JSON Schema 2020-12"),
    ("Schema", "$anchor", "resolved at load: a `$ref` to it reaches its schema as a pointer does"),
];

/// Fields that the model declares by object kind (the `src/spec` struct name).
/// They are read from the source of the model: a `pub` field named in camelCase or the
/// name that its `#[serde(rename = "...")]` gives.
fn model_fields() -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for file in ["src/spec/openapi.rs", "src/spec/schema.rs", "src/spec.rs"] {
        let text = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(file)).expect("model source");
        let (mut kind, mut camel, mut rename): (Option<String>, bool, Option<String>) = (None, false, None);
        let mut pending_camel = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with("#[serde(") && line.contains("rename_all = \"camelCase\"") {
                pending_camel = true;
            } else if let Some(rest) = line.strip_prefix("pub(crate) struct ") {
                // `SchemaObject` is the Schema Object; `Reference` and `Entries` are not objects of the spec.
                kind = rest
                    .split([' ', '{', '<'])
                    .next()
                    .filter(|k| !matches!(*k, "Reference" | "Entries"))
                    .map(|k| if k == "SchemaObject" { "Schema".to_owned() } else { k.to_owned() });
                camel = std::mem::take(&mut pending_camel);
            } else if line == "}" {
                kind = None;
            } else if let Some(r) = line.strip_prefix("#[serde(").and_then(|l| l.split("rename = \"").nth(1)) {
                rename = r.split('"').next().map(String::from);
            } else if let (Some(k), Some(field)) = (&kind, line.strip_prefix("pub ").and_then(|l| l.split(':').next()))
            {
                let name = rename
                    .take()
                    .unwrap_or_else(|| if camel { camel_case(field) } else { field.trim_end_matches('_').to_owned() });
                if field != "extra" {
                    out.insert((k.clone(), name));
                }
            } else if !line.starts_with("#[") && !line.starts_with("///") {
                rename = None;
            }
        }
    }
    out
}

fn camel_case(snake: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for c in snake.trim_end_matches('_').chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.extend(c.to_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Keys of a schema that hold one subschema, a map of them or a list.
const SUB_ONE: &[&str] = &[
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
const SUB_MAP: &[&str] = &["properties", "patternProperties", "$defs", "dependentSchemas"];
const SUB_LIST: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace", "query"];

/// Every object of a 3.2 document by kind as (kind, pointer).
struct Kinds(Vec<(&'static str, Vec<String>)>);

impl Kinds {
    fn of(doc: &Value) -> Self {
        let mut k = Self(vec![]);
        k.root(doc);
        k
    }

    fn add(&mut self, kind: &'static str, path: &[String]) {
        self.0.push((kind, path.to_vec()));
    }

    fn at(path: &[String], key: &str) -> Vec<String> {
        let mut p = path.to_vec();
        p.push(key.to_owned());
        p
    }

    fn members<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
        v.get(key).and_then(Value::as_object).into_iter().flatten().filter(|(k, _)| !k.starts_with("x-"))
    }

    fn list<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = (String, &'a Value)> {
        v.get(key).and_then(Value::as_array).into_iter().flatten().enumerate().map(|(i, x)| (i.to_string(), x))
    }

    /// A `$ref` object is a Reference Object and not the kind that it stands for.
    fn obj(&mut self, kind: &'static str, v: &Value, path: &[String]) -> bool {
        if !v.is_object() || v.get("$ref").is_some() && kind != "Schema" && kind != "PathItem" {
            return false;
        }
        self.add(kind, path);
        true
    }

    fn root(&mut self, doc: &Value) {
        let p: Vec<String> = vec![];
        self.add("OpenApi", &p);
        if let Some(info) = doc.get("info") {
            let ip = Self::at(&p, "info");
            self.obj("Info", info, &ip);
            for (key, kind) in [("contact", "Contact"), ("license", "License")] {
                if let Some(x) = info.get(key) {
                    self.obj(kind, x, &Self::at(&ip, key));
                }
            }
        }
        self.servers(doc, &p);
        self.external_docs(doc, &p);
        for (i, t) in Self::list(doc, "tags") {
            let tp = Self::at(&Self::at(&p, "tags"), &i);
            if self.obj("Tag", t, &tp) {
                self.external_docs(t, &tp);
            }
        }
        for key in ["paths", "webhooks"] {
            for (name, item) in Self::members(doc, key) {
                self.path_item(item, &Self::at(&Self::at(&p, key), name));
            }
        }
        if let Some(c) = doc.get("components") {
            self.components(c, &Self::at(&p, "components"));
        }
    }

    fn external_docs(&mut self, v: &Value, path: &[String]) {
        if let Some(d) = v.get("externalDocs") {
            self.obj("ExternalDocs", d, &Self::at(path, "externalDocs"));
        }
    }

    fn servers(&mut self, v: &Value, path: &[String]) {
        for (i, s) in Self::list(v, "servers") {
            let sp = Self::at(&Self::at(path, "servers"), &i);
            if self.obj("Server", s, &sp) {
                for (n, var) in Self::members(s, "variables") {
                    self.obj("ServerVariable", var, &Self::at(&Self::at(&sp, "variables"), n));
                }
            }
        }
    }

    fn components(&mut self, c: &Value, p: &[String]) {
        self.obj("Components", c, p);
        let section = |s: &str| Self::at(p, s);
        for (n, s) in Self::members(c, "schemas") {
            self.schema(s, &Self::at(&section("schemas"), n));
        }
        for (n, r) in Self::members(c, "responses") {
            self.response(r, &Self::at(&section("responses"), n));
        }
        for (n, x) in Self::members(c, "parameters") {
            self.parameter(x, &Self::at(&section("parameters"), n));
        }
        for (n, x) in Self::members(c, "examples") {
            self.obj("Example", x, &Self::at(&section("examples"), n));
        }
        for (n, x) in Self::members(c, "requestBodies") {
            self.request_body(x, &Self::at(&section("requestBodies"), n));
        }
        for (n, x) in Self::members(c, "headers") {
            self.header(x, &Self::at(&section("headers"), n));
        }
        for (n, x) in Self::members(c, "securitySchemes") {
            let sp = Self::at(&section("securitySchemes"), n);
            if self.obj("SecurityScheme", x, &sp)
                && let Some(flows) = x.get("flows")
            {
                let fp = Self::at(&sp, "flows");
                self.obj("OAuthFlows", flows, &fp);
                for (f, flow) in flows.as_object().into_iter().flatten().filter(|(k, _)| !k.starts_with("x-")) {
                    self.obj("OAuthFlow", flow, &Self::at(&fp, f));
                }
            }
        }
        for (n, x) in Self::members(c, "links") {
            self.obj("Link", x, &Self::at(&section("links"), n));
        }
        for (n, cb) in Self::members(c, "callbacks") {
            self.callback(cb, &Self::at(&section("callbacks"), n));
        }
        for (n, x) in Self::members(c, "pathItems") {
            self.path_item(x, &Self::at(&section("pathItems"), n));
        }
        for (n, x) in Self::members(c, "mediaTypes") {
            self.media_type(x, &Self::at(&section("mediaTypes"), n));
        }
    }

    fn callback(&mut self, cb: &Value, p: &[String]) {
        for (expr, item) in cb.as_object().into_iter().flatten().filter(|(k, _)| !k.starts_with("x-")) {
            self.path_item(item, &Self::at(p, expr));
        }
    }

    fn path_item(&mut self, item: &Value, p: &[String]) {
        if !self.obj("PathItem", item, p) {
            return;
        }
        self.servers(item, p);
        for (i, x) in Self::list(item, "parameters") {
            self.parameter(x, &Self::at(&Self::at(p, "parameters"), &i));
        }
        for m in METHODS {
            if let Some(op) = item.get(*m) {
                self.operation(op, &Self::at(p, m));
            }
        }
        for (m, op) in Self::members(item, "additionalOperations") {
            self.operation(op, &Self::at(&Self::at(p, "additionalOperations"), m));
        }
    }

    fn operation(&mut self, op: &Value, p: &[String]) {
        if !self.obj("Operation", op, p) {
            return;
        }
        self.external_docs(op, p);
        self.servers(op, p);
        for (i, x) in Self::list(op, "parameters") {
            self.parameter(x, &Self::at(&Self::at(p, "parameters"), &i));
        }
        if let Some(b) = op.get("requestBody") {
            self.request_body(b, &Self::at(p, "requestBody"));
        }
        for (code, r) in Self::members(op, "responses") {
            self.response(r, &Self::at(&Self::at(p, "responses"), code));
        }
        for (n, cb) in Self::members(op, "callbacks") {
            self.callback(cb, &Self::at(&Self::at(p, "callbacks"), n));
        }
    }

    fn parameter(&mut self, x: &Value, p: &[String]) {
        if self.obj("Parameter", x, p) {
            self.schema_at(x, "schema", p);
            self.content(x, p);
            self.examples(x, p);
        }
    }

    fn header(&mut self, x: &Value, p: &[String]) {
        if self.obj("Header", x, p) {
            self.schema_at(x, "schema", p);
            self.content(x, p);
            self.examples(x, p);
        }
    }

    fn request_body(&mut self, x: &Value, p: &[String]) {
        if self.obj("RequestBody", x, p) {
            self.content(x, p);
        }
    }

    fn response(&mut self, r: &Value, p: &[String]) {
        if !self.obj("Response", r, p) {
            return;
        }
        self.content(r, p);
        for (n, h) in Self::members(r, "headers") {
            self.header(h, &Self::at(&Self::at(p, "headers"), n));
        }
        for (n, l) in Self::members(r, "links") {
            self.obj("Link", l, &Self::at(&Self::at(p, "links"), n));
        }
    }

    fn content(&mut self, x: &Value, p: &[String]) {
        for (m, mt) in Self::members(x, "content") {
            self.media_type(mt, &Self::at(&Self::at(p, "content"), m));
        }
    }

    fn media_type(&mut self, mt: &Value, p: &[String]) {
        if !self.obj("MediaType", mt, p) {
            return;
        }
        self.schema_at(mt, "schema", p);
        self.schema_at(mt, "itemSchema", p);
        self.examples(mt, p);
        for (n, e) in Self::members(mt, "encoding") {
            self.encoding(e, &Self::at(&Self::at(p, "encoding"), n));
        }
        for (i, e) in Self::list(mt, "prefixEncoding") {
            self.encoding(e, &Self::at(&Self::at(p, "prefixEncoding"), &i));
        }
        if let Some(e) = mt.get("itemEncoding") {
            self.encoding(e, &Self::at(p, "itemEncoding"));
        }
    }

    fn encoding(&mut self, e: &Value, p: &[String]) {
        if !self.obj("Encoding", e, p) {
            return;
        }
        for (n, h) in Self::members(e, "headers") {
            self.header(h, &Self::at(&Self::at(p, "headers"), n));
        }
        for (n, x) in Self::members(e, "encoding") {
            self.encoding(x, &Self::at(&Self::at(p, "encoding"), n));
        }
        for (i, x) in Self::list(e, "prefixEncoding") {
            self.encoding(x, &Self::at(&Self::at(p, "prefixEncoding"), &i));
        }
        if let Some(x) = e.get("itemEncoding") {
            self.encoding(x, &Self::at(p, "itemEncoding"));
        }
    }

    fn examples(&mut self, x: &Value, p: &[String]) {
        for (n, e) in Self::members(x, "examples") {
            self.obj("Example", e, &Self::at(&Self::at(p, "examples"), n));
        }
    }

    fn schema_at(&mut self, x: &Value, key: &str, p: &[String]) {
        if let Some(s) = x.get(key) {
            self.schema(s, &Self::at(p, key));
        }
    }

    fn schema(&mut self, s: &Value, p: &[String]) {
        if !self.obj("Schema", s, p) {
            return;
        }
        for key in SUB_ONE {
            self.schema_at(s, key, p);
        }
        for key in SUB_MAP {
            for (n, x) in s.get(*key).and_then(Value::as_object).into_iter().flatten() {
                self.schema(x, &Self::at(&Self::at(p, key), n));
            }
        }
        for key in SUB_LIST {
            for (i, x) in Self::list(s, key) {
                self.schema(x, &Self::at(&Self::at(p, key), &i));
            }
        }
        for (key, kind) in [("discriminator", "Discriminator"), ("xml", "Xml"), ("externalDocs", "ExternalDocs")] {
            if let Some(x) = s.get(key) {
                self.obj(kind, x, &Self::at(p, key));
            }
        }
    }
}

fn object_mut<'a>(doc: &'a mut Value, path: &[String]) -> Option<&'a mut Map<String, Value>> {
    let mut cur = doc;
    for t in path {
        cur = match cur {
            Value::Object(m) => m.get_mut(t)?,
            Value::Array(a) => a.get_mut(t.parse::<usize>().ok()?)?,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
        };
    }
    cur.as_object_mut()
}

/// The generated module of a 3.2 document as one string. `externals` are the files and URLs that its examples name:
/// read once per fixture, since some are URLs and removing a field never names a new one.
fn generate(doc: &Value, externals: &crate::json::Externals, opts: &crate::emit::Options) -> String {
    let mut diags = crate::diag::Diagnostics::default();
    let mut d = crate::json::Doc::new(doc.clone());
    d.externals = externals.clone();
    let api = crate::ir::build(&d, &mut diags);
    crate::emit::rust::print(crate::emit::rust::trees(&api, opts), opts.rust.binary)
        .map(|files| files.into_iter().map(|(_, t)| t).collect())
        .unwrap_or_default()
}

/// Every fixture loaded and upgraded to 3.2 with the libraries that it declares.
fn fixtures() -> Vec<(String, Value, crate::emit::Options, std::path::PathBuf)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
    let mut out = vec![];
    for dir in ["v20", "v31", "v32", "v30"] {
        let mut entries: Vec<_> =
            std::fs::read_dir(root.join(dir)).expect("fixtures").flatten().map(|e| e.path()).collect();
        entries.sort();
        for path in entries.into_iter().filter(|p| p.extension().is_some_and(|e| e == "yaml")) {
            let loaded = crate::load::load_path(&path);
            let Some(doc) = loaded.value else { continue };
            let deps = std::fs::read_to_string(path.with_extension("deps")).unwrap_or_default();
            let opts = crate::emit::Options {
                rust: crate::emit::rust::Options {
                    libs: crate::settings::detected_format_libs(
                        &crate::settings::Manifest::parse(&format!("[dependencies]\n{deps}")).unwrap(),
                    ),
                    ..Default::default()
                },
                // The examples test is where examples reach the generated code.
                tests: true,
                ..crate::emit::Options::default()
            };
            let name = format!("{dir}/{}", path.file_stem().unwrap_or_default().to_string_lossy());
            out.push((name, doc, opts, root.join(dir)));
        }
    }
    out
}

/// Each (kind, field) that `doc` sets, other than `x-` extensions, with the paths of the objects that set it.
fn fields<'k>(doc: &Value, kinds: &'k Kinds) -> BTreeMap<(String, String), Vec<&'k Vec<String>>> {
    let mut fields: BTreeMap<(String, String), Vec<&Vec<String>>> = BTreeMap::new();
    for (kind, path) in &kinds.0 {
        let Some(Value::Object(o)) = path.iter().try_fold(doc, |v, t| match v {
            Value::Array(a) => a.get(t.parse::<usize>().ok()?),
            Value::Object(m) => m.get(t),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
        }) else {
            continue;
        };
        for k in o.keys().filter(|k| !k.starts_with("x-")) {
            fields.entry(((*kind).to_owned(), k.clone())).or_default().push(path);
        }
    }
    fields
}

#[test]
fn every_model_field_reaches_the_generated_code_or_is_explained() {
    let model = model_fields();
    let not_emitted: BTreeMap<(String, String), &str> =
        NOT_EMITTED.iter().map(|(k, f, why)| (((*k).to_owned(), (*f).to_owned()), *why)).collect();
    // (kind, field) to the fixtures that use it and whether removing it changed the code. Each fixture is regenerated
    // once per field: the fixtures share the cores. A field that one fixture showed to change the code is not tried
    // again, so `changes` does not depend on the order.
    let fixtures = fixtures();
    let next = AtomicUsize::new(0);
    let changes: Mutex<BTreeSet<(String, String)>> = Mutex::new(BTreeSet::new());
    let workers = std::thread::available_parallelism().map_or(1, usize::from);
    let mut used: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let found: Vec<Vec<((String, String), String)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut found = vec![];
                    while let Some((name, doc, opts, dir)) = fixtures.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let kinds = Kinds::of(doc);
                        let externals = crate::resolve::read_externals(doc, Some(dir));
                        let base = generate(doc, &externals, opts);
                        for (key, paths) in fields(doc, &kinds) {
                            found.push((key.clone(), name.clone()));
                            if changes.lock().unwrap().contains(&key) {
                                continue;
                            }
                            let mut stripped = doc.clone();
                            for p in paths {
                                if let Some(o) = object_mut(&mut stripped, p) {
                                    o.remove(&key.1);
                                }
                            }
                            if generate(&stripped, &externals, opts) != base {
                                changes.lock().unwrap().insert(key);
                            }
                        }
                    }
                    found
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    // In the order of the fixtures, as one thread would list them.
    let mut found: Vec<_> = found.into_iter().flatten().collect();
    let order: BTreeMap<&str, usize> = fixtures.iter().enumerate().map(|(i, f)| (f.0.as_str(), i)).collect();
    found.sort_by_key(|(_, name)| order.get(name.as_str()).copied());
    for (key, name) in found {
        used.entry(key).or_default().push(name);
    }
    let changes = changes.into_inner().unwrap();
    let mut problems = vec![];
    for (key, files) in &used {
        let (k, f) = key;
        match (model.contains(key), changes.contains(key), not_emitted.get(key)) {
            (false, ..) => problems.push(format!("{k}.{f}: used by {files:?} but not in the model")),
            (true, true, Some(_)) => problems.push(format!("{k}.{f}: listed in NOT_EMITTED but changes the code")),
            (true, false, None) => problems
                .push(format!("{k}.{f}: removing it changes nothing ({files:?}); emit it or list it in NOT_EMITTED")),
            (true, true, None) | (true, false, Some(_)) => {}
        }
    }
    for key in &model {
        if !used.contains_key(key) && !not_emitted.contains_key(key) {
            problems.push(format!("{}.{}: no fixture uses it; add one or list it in NOT_EMITTED", key.0, key.1));
        }
    }
    for key in not_emitted.keys() {
        if !model.contains(key) {
            problems.push(format!("{}.{}: in NOT_EMITTED but not in the model", key.0, key.1));
        }
    }
    assert!(problems.is_empty(), "{} fields to look at:\n{}", problems.len(), problems.join("\n"));
}
