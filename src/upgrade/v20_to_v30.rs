//! Swagger 2.0 to OpenAPI 3.0.
//!
//! | 2.0 | Becomes |
//! | --- | --- |
//! | `host`, `basePath`, `schemes` | `servers` |
//! | `definitions`, `parameters`, `responses`, `securityDefinitions` | `components`, with every `$ref` rewritten |
//! | `body` and `formData` parameters | `requestBody`, one entry per `consumes` media type |
//! | a response `schema` | `content`, one entry per `produces` media type |
//! | inline type keywords of other parameters and headers | their `schema` |
//! | `collectionFormat` | `style` and `explode` |
//! | `collectionFormat: tsv` | `x-collection-format: tsv`: 3.x has no tab style, but honig still encodes it |
//! | `type: file` | `type: string` with `format: binary` |
//! | `x-nullable` | `nullable` |
//!
//! What is lost:
//!
//! - An unknown `collectionFormat` is read as `csv`.
//! - An unknown `OAuth2` `flow` is read as `implicit`.
//! - A security scheme of an unknown `type` is kept as written. Generated code does not use it.

use serde_json::{Value, json};

use crate::diag::Diagnostics;
use crate::json;
use crate::json::join;
use crate::walk::Obj;

const DEFAULT_MEDIA: &str = "application/json";
const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch"];
/// Keywords that move from a 2.0 parameter / header / items into a schema.
const SCHEMA_KEYS: &[&str] = &[
    "type",
    "format",
    "items",
    "default",
    "maximum",
    "exclusiveMaximum",
    "minimum",
    "exclusiveMinimum",
    "maxLength",
    "minLength",
    "pattern",
    "maxItems",
    "minItems",
    "uniqueItems",
    "enum",
    "multipleOf",
];

pub(super) fn upgrade(doc: Value, diags: &mut Diagnostics) -> Value {
    let Value::Object(src) = doc else { return doc };
    let mut cx = Cx { src: &src, diags };
    let mut out = Obj::new();
    out.insert("openapi".into(), json!("3.0.3"));
    if let Some(v) = src.get("info") {
        out.insert("info".into(), v.clone());
    }
    if let Some(servers) = cx.servers() {
        out.insert("servers".into(), servers);
    }
    let global_consumes = media_list(src.get("consumes"));
    let global_produces = media_list(src.get("produces"));

    if let Some(Value::Object(paths)) = src.get("paths") {
        let mut new_paths = Obj::new();
        for (path, item) in paths {
            if path.starts_with("x-") {
                new_paths.insert(path.clone(), item.clone());
                continue;
            }
            let ptr = join("/paths", path);
            new_paths.insert(path.clone(), cx.path_item(item, &ptr, &global_consumes, &global_produces));
        }
        out.insert("paths".into(), Value::Object(new_paths));
    }

    let mut comps = Obj::new();
    if let Some(Value::Object(defs)) = src.get("definitions") {
        let schemas: Obj = defs.iter().map(|(k, s)| (k.clone(), schema(s))).collect();
        comps.insert("schemas".into(), Value::Object(schemas));
    }
    if let Some(Value::Object(resps)) = src.get("responses") {
        let r: Obj = resps.iter().map(|(k, r)| (k.clone(), Cx::response(r, &global_produces))).collect();
        comps.insert("responses".into(), Value::Object(r));
    }
    if let Some(Value::Object(params)) = src.get("parameters") {
        let mut ps = Obj::new();
        let mut bodies = Obj::new();
        for (k, p) in params {
            match p.get("in").and_then(Value::as_str) {
                Some("body") => {
                    bodies.insert(k.clone(), body_request(p, &global_consumes));
                }
                // formData parameters cannot stand alone in 3.0; they are
                // inlined into each operation's form body instead.
                Some("formData") => {}
                _ => {
                    ps.insert(k.clone(), cx.parameter(p, &join("/parameters", k)));
                }
            }
        }
        if !ps.is_empty() {
            comps.insert("parameters".into(), Value::Object(ps));
        }
        if !bodies.is_empty() {
            comps.insert("requestBodies".into(), Value::Object(bodies));
        }
    }
    if let Some(Value::Object(defs)) = src.get("securityDefinitions") {
        let s: Obj =
            defs.iter().map(|(k, d)| (k.clone(), cx.security_scheme(d, &join("/securityDefinitions", k)))).collect();
        comps.insert("securitySchemes".into(), Value::Object(s));
    }
    if !comps.is_empty() {
        out.insert("components".into(), Value::Object(comps));
    }
    for key in ["security", "tags", "externalDocs"] {
        if let Some(v) = src.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    copy_extensions(&src, &mut out);

    let mut out = Value::Object(out);
    rewrite_refs(&mut out, &src, "");
    out
}

struct Cx<'a> {
    src: &'a Obj,
    diags: &'a mut Diagnostics,
}

impl Cx<'_> {
    fn servers(&self) -> Option<Value> {
        let host = self.src.get("host").and_then(Value::as_str);
        let base = self.src.get("basePath").and_then(Value::as_str).unwrap_or("");
        if host.is_none() && base.is_empty() {
            return None;
        }
        let base = if base.is_empty() || base.starts_with('/') { base.to_owned() } else { format!("/{base}") };
        let Some(host) = host else {
            return Some(json!([{"url": base}]));
        };
        let schemes: Vec<&str> = self
            .src
            .get("schemes")
            .and_then(Value::as_array)
            .map(|s| json::strs(s).collect())
            .filter(|s: &Vec<&str>| !s.is_empty())
            .unwrap_or_else(|| vec!["https"]);
        Some(servers(schemes, host, &base))
    }

    fn path_item(&mut self, item: &Value, ptr: &str, consumes: &[String], produces: &[String]) -> Value {
        let Value::Object(item) = item else { return item.clone() };
        let mut out = Obj::new();
        if let Some(r) = item.get("$ref") {
            out.insert("$ref".into(), r.clone());
        }
        // Path-level body/formData parameters must move into each operation.
        let (path_params, path_body_params) = self.split_params(item.get("parameters"));
        if let Some(ps) = &path_params {
            let converted = self.parameters(ps, ptr);
            out.insert("parameters".into(), Value::Array(converted));
        }
        for m in METHODS {
            if let Some(op) = item.get(*m) {
                let op = self.operation(op, &join(ptr, m), &path_body_params, consumes, produces);
                out.insert((*m).into(), op);
            }
        }
        copy_extensions(item, &mut out);
        Value::Object(out)
    }

    /// Splits a parameter list into (non-body params, body + formData params).
    /// References to global body/formData parameters are resolved here.
    fn split_params(&self, params: Option<&Value>) -> (Option<Vec<Value>>, Vec<Value>) {
        let Some(Value::Array(ps)) = params else { return (None, vec![]) };
        let mut plain = vec![];
        let mut body = vec![];
        for p in ps {
            let resolved = self.resolve_param(p);
            match resolved.and_then(|r| r.get("in")).and_then(Value::as_str) {
                Some("body" | "formData") => body.push(p.clone()),
                _ => plain.push(p.clone()),
            }
        }
        (Some(plain), body)
    }

    fn resolve_param<'b>(&'b self, p: &'b Value) -> Option<&'b Value> {
        match p.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let name = r.strip_prefix("#/parameters/")?;
                self.src.get("parameters")?.get(json::unescape_token(name))
            }
            None => Some(p),
        }
    }

    fn operation(
        &mut self,
        op: &Value,
        ptr: &str,
        path_body: &[Value],
        consumes: &[String],
        produces: &[String],
    ) -> Value {
        let Value::Object(op) = op else { return op.clone() };
        let consumes = media_list(op.get("consumes")).or_default(consumes);
        let produces = media_list(op.get("produces")).or_default(produces);
        let mut out = Obj::new();
        for key in ["tags", "summary", "description", "externalDocs", "operationId"] {
            if let Some(v) = op.get(key) {
                out.insert(key.into(), v.clone());
            }
        }

        let (params, mut body_params) = self.split_params(op.get("parameters"));
        // Operation-level parameters override path-level ones with the same name+in.
        for pb in path_body {
            let key = self.resolve_param(pb).map(|p| (p.get("name").cloned(), p.get("in").cloned()));
            let overridden = body_params
                .iter()
                .any(|b| self.resolve_param(b).map(|p| (p.get("name").cloned(), p.get("in").cloned())) == key);
            if !overridden {
                body_params.push(pb.clone());
            }
        }
        if let Some(ps) = params {
            let converted = self.parameters(&ps, ptr);
            if !converted.is_empty() {
                out.insert("parameters".into(), Value::Array(converted));
            }
        }
        if let Some(rb) = self.request_body(&body_params, &consumes) {
            out.insert("requestBody".into(), rb);
        }
        if let Some(Value::Object(resps)) = op.get("responses") {
            let mut r = Obj::new();
            for (code, resp) in resps {
                if code.starts_with("x-") {
                    r.insert(code.clone(), resp.clone());
                } else {
                    r.insert(code.clone(), Self::response(resp, &produces));
                }
            }
            out.insert("responses".into(), Value::Object(r));
        }
        for key in ["deprecated", "security"] {
            if let Some(v) = op.get(key) {
                out.insert(key.into(), v.clone());
            }
        }
        if let Some(Value::Array(schemes)) = op.get("schemes")
            && let Some(host) = self.src.get("host").and_then(Value::as_str)
        {
            let base = self.src.get("basePath").and_then(Value::as_str).unwrap_or("");
            out.insert("servers".into(), servers(json::strs(schemes), host, base));
        }
        copy_extensions(op, &mut out);
        Value::Object(out)
    }

    fn request_body(&self, params: &[Value], consumes: &[String]) -> Option<Value> {
        let mut form = vec![];
        for p in params {
            let resolved = self.resolve_param(p)?.clone();
            match resolved.get("in").and_then(Value::as_str) {
                Some("body") => {
                    // A $ref to a global body parameter maps to a requestBody ref.
                    if let Some(r) = p.get("$ref").and_then(Value::as_str) {
                        let name = r.trim_start_matches("#/parameters/");
                        return Some(json!({"$ref": format!("#/components/requestBodies/{name}")}));
                    }
                    let mut rb = body_request(&resolved, consumes);
                    if let Some(name) = resolved.get("name") {
                        rb["x-codegen-request-body-name"] = name.clone();
                    }
                    return Some(rb);
                }
                Some("formData") => form.push(resolved),
                _ => {}
            }
        }
        if form.is_empty() {
            return None;
        }
        let has_file = form.iter().any(|p| p.get("type").and_then(Value::as_str) == Some("file"));
        let mut props = Obj::new();
        let mut required = vec![];
        for p in &form {
            let name = p.get("name").and_then(Value::as_str).unwrap_or_default().to_owned();
            let mut s = Self::param_schema(p);
            if let Some(d) = p.get("description")
                && let Some(o) = s.as_object_mut()
            {
                o.insert("description".into(), d.clone());
            }
            props.insert(name.clone(), s);
            if p.get("required") == Some(&Value::Bool(true)) {
                required.push(json!(name));
            }
        }
        let mut schema = json!({"type": "object", "properties": props});
        if !required.is_empty() {
            schema["required"] = Value::Array(required);
        }
        let mut types: Vec<&str> = consumes.iter().map(String::as_str).filter(|c| is_form(c)).collect();
        if types.is_empty() || (has_file && !types.contains(&"multipart/form-data")) {
            types = vec![if has_file { "multipart/form-data" } else { "application/x-www-form-urlencoded" }];
        }
        let content: Obj = types.iter().map(|t| (t.to_string(), json!({"schema": schema.clone()}))).collect();
        let required_body = form.iter().any(|p| p.get("required") == Some(&Value::Bool(true)));
        let mut rb = json!({"content": content});
        if required_body {
            rb["required"] = json!(true);
        }
        Some(rb)
    }

    /// Each parameter of `ps` at `ptr/parameters/<i>` in 3.0 form.
    fn parameters(&mut self, ps: &[Value], ptr: &str) -> Vec<Value> {
        ps.iter().enumerate().map(|(i, p)| self.parameter(p, &format!("{ptr}/parameters/{i}"))).collect()
    }

    fn parameter(&mut self, p: &Value, ptr: &str) -> Value {
        let Value::Object(p) = p else { return p.clone() };
        if let Some(r) = p.get("$ref") {
            return json!({"$ref": r});
        }
        let mut out = Obj::new();
        for key in ["name", "in", "description", "required", "allowEmptyValue"] {
            if let Some(v) = p.get(key) {
                out.insert(key.into(), v.clone());
            }
        }
        let location = p.get("in").and_then(Value::as_str).unwrap_or("query");
        if let Some((style, explode)) = self.collection_style(p, location, ptr) {
            out.insert("style".into(), json!(style));
            out.insert("explode".into(), json!(explode));
        }
        // No 3.x style is tab separated. Keep the format as an extension.
        // The generator reads it back. Other tools see form and csv.
        if p.get("type").and_then(Value::as_str) == Some("array")
            && p.get("collectionFormat").and_then(Value::as_str) == Some("tsv")
        {
            out.insert("x-collection-format".into(), json!("tsv"));
        }
        out.insert("schema".into(), Self::param_schema(&Value::Object(p.clone())));
        copy_extensions(p, &mut out);
        Value::Object(out)
    }

    /// Maps `collectionFormat` to a 3.0 style. `None` means the 3.0 default is right.
    fn collection_style(&mut self, p: &Obj, location: &str, ptr: &str) -> Option<(&'static str, bool)> {
        if p.get("type").and_then(Value::as_str) != Some("array") {
            return None;
        }
        let format = p.get("collectionFormat").and_then(Value::as_str).unwrap_or("csv");
        let query_like = matches!(location, "query" | "formData");
        match format {
            "csv" if query_like => Some(("form", false)),
            "csv" => None, // simple style, explode false: the 3.0 default for path/header
            "ssv" => Some(("spaceDelimited", false)),
            "pipes" => Some(("pipeDelimited", false)),
            "multi" => Some(("form", true)),
            "tsv" => query_like.then_some(("form", false)),
            other => {
                self.diags.warning(
                    join(ptr, "collectionFormat"),
                    format!("collectionFormat `{other}` has no OpenAPI 3 equivalent; using csv"),
                );
                query_like.then_some(("form", false))
            }
        }
    }

    /// Builds a schema from a 2.0 parameter / header / items object.
    fn param_schema(p: &Value) -> Value {
        if let Some(s) = p.get("schema") {
            return schema(s);
        }
        let mut s = Obj::new();
        for key in SCHEMA_KEYS {
            if let Some(v) = p.get(*key) {
                let v = if *key == "items" { Self::param_schema(v) } else { v.clone() };
                s.insert((*key).into(), v);
            }
        }
        if s.get("type").and_then(Value::as_str) == Some("file") {
            s.insert("type".into(), json!("string"));
            s.insert("format".into(), json!("binary"));
        }
        if matches!(p.get("x-nullable"), Some(Value::Bool(true))) {
            s.insert("nullable".into(), json!(true));
        }
        Value::Object(s)
    }

    fn response(r: &Value, produces: &[String]) -> Value {
        let Value::Object(r) = r else { return r.clone() };
        if let Some(rf) = r.get("$ref") {
            return json!({"$ref": rf});
        }
        let mut out = Obj::new();
        out.insert("description".into(), r.get("description").cloned().unwrap_or_else(|| json!("")));
        if let Some(Value::Object(headers)) = r.get("headers") {
            let h: Obj = headers
                .iter()
                .map(|(k, h)| {
                    let mut o = Obj::new();
                    if let Some(d) = h.get("description") {
                        o.insert("description".into(), d.clone());
                    }
                    o.insert("schema".into(), Self::param_schema(h));
                    (k.clone(), Value::Object(o))
                })
                .collect();
            out.insert("headers".into(), Value::Object(h));
        }
        let examples = r.get("examples").and_then(Value::as_object);
        if let Some(s) = r.get("schema") {
            let s = schema(s);
            let mut content = Obj::new();
            for mt in produces {
                let mut media = json!({"schema": s.clone()});
                if let Some(ex) = examples.and_then(|e| e.get(mt)) {
                    media["example"] = ex.clone();
                }
                content.insert(mt.clone(), media);
            }
            out.insert("content".into(), Value::Object(content));
        } else if let Some(ex) = examples {
            // Examples without a schema still describe the media types.
            let content: Obj = ex.iter().map(|(mt, v)| (mt.clone(), json!({"example": v}))).collect();
            out.insert("content".into(), Value::Object(content));
        }
        copy_extensions(r, &mut out);
        Value::Object(out)
    }

    fn security_scheme(&mut self, d: &Value, ptr: &str) -> Value {
        let Value::Object(d) = d else { return d.clone() };
        let mut out = Obj::new();
        match d.get("type").and_then(Value::as_str) {
            Some("basic") => {
                out.insert("type".into(), json!("http"));
                out.insert("scheme".into(), json!("basic"));
            }
            Some("apiKey") => {
                out.insert("type".into(), json!("apiKey"));
                for key in ["name", "in"] {
                    if let Some(v) = d.get(key) {
                        out.insert(key.into(), v.clone());
                    }
                }
            }
            Some("oauth2") => {
                out.insert("type".into(), json!("oauth2"));
                let flow_name = match d.get("flow").and_then(Value::as_str) {
                    Some("implicit") => "implicit",
                    Some("password") => "password",
                    Some("application") => "clientCredentials",
                    Some("accessCode") => "authorizationCode",
                    other => {
                        self.diags.warning(join(ptr, "flow"), format!("unknown OAuth2 flow {other:?}"));
                        "implicit"
                    }
                };
                let mut flow = Obj::new();
                for key in ["authorizationUrl", "tokenUrl"] {
                    if let Some(v) = d.get(key) {
                        flow.insert(key.into(), v.clone());
                    }
                }
                flow.insert("scopes".into(), d.get("scopes").cloned().unwrap_or_else(|| json!({})));
                out.insert("flows".into(), json!({ flow_name: flow }));
            }
            other => {
                self.diags.warning(join(ptr, "type"), format!("unknown security scheme type {other:?}"));
                return Value::Object(d.clone());
            }
        }
        if let Some(desc) = d.get("description") {
            out.insert("description".into(), desc.clone());
        }
        copy_extensions(d, &mut out);
        Value::Object(out)
    }
}

/// Returns a 2.0 body parameter as a 3.0 Request Body.
fn body_request(p: &Value, consumes: &[String]) -> Value {
    let s = p.get("schema").map_or_else(|| json!({}), schema);
    let mut types: Vec<&String> = consumes.iter().filter(|c| !is_form(c)).collect();
    let fallback = DEFAULT_MEDIA.to_owned();
    if types.is_empty() {
        types.push(&fallback);
    }
    let content: Obj = types.into_iter().map(|t| (t.clone(), json!({"schema": s.clone()}))).collect();
    let mut rb = json!({"content": content});
    if let Some(d) = p.get("description") {
        rb["description"] = d.clone();
    }
    if p.get("required") == Some(&Value::Bool(true)) {
        rb["required"] = json!(true);
    }
    if let (Value::Object(o), Some(out)) = (p, rb.as_object_mut()) {
        copy_extensions(o, out);
    }
    rb
}

fn is_form(media: &str) -> bool {
    media.starts_with("multipart/form-data") || media.starts_with("application/x-www-form-urlencoded")
}

/// Returns a 2.0 Schema Object as a 3.0 Schema Object (recursively).
fn schema(s: &Value) -> Value {
    match s {
        Value::Object(o) => {
            let mut out = Obj::new();
            for (k, v) in o {
                let v = match k.as_str() {
                    "properties" | "definitions" => match v.as_object() {
                        Some(m) => Value::Object(m.iter().map(|(k, s)| (k.clone(), schema(s))).collect()),
                        None => v.clone(),
                    },
                    "items" | "additionalProperties" | "not" => schema(v),
                    "allOf" | "anyOf" | "oneOf" => match v.as_array() {
                        Some(a) => Value::Array(a.iter().map(schema).collect()),
                        None => v.clone(),
                    },
                    "discriminator" => match v.as_str() {
                        Some(name) => json!({"propertyName": name}),
                        None => v.clone(),
                    },
                    "x-nullable" => {
                        if v == &Value::Bool(true) {
                            out.insert("nullable".into(), json!(true));
                        }
                        continue;
                    }
                    _ => v.clone(),
                };
                out.insert(k.clone(), v);
            }
            if out.get("type").and_then(Value::as_str) == Some("file") {
                out.insert("type".into(), json!("string"));
                out.insert("format".into(), json!("binary"));
            }
            Value::Object(out)
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => s.clone(),
    }
}

fn media_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| json::strings(a)).unwrap_or_default()
}

trait OrDefault {
    fn or_default(self, fallback: &[String]) -> Vec<String>;
}

impl OrDefault for Vec<String> {
    fn or_default(self, fallback: &[String]) -> Vec<String> {
        if !self.is_empty() {
            self
        } else if !fallback.is_empty() {
            fallback.to_vec()
        } else {
            vec![DEFAULT_MEDIA.to_owned()]
        }
    }
}

fn copy_extensions(from: &Obj, to: &mut Obj) {
    for (k, v) in from {
        if k.starts_with("x-") && k != "x-nullable" {
            to.insert(k.clone(), v.clone());
        }
    }
}

/// Rewrites every `$ref` to the 3.0 component locations. Works for local refs
/// and for the fragment part of external refs (`other.yaml#/definitions/A`).
fn rewrite_refs(v: &mut Value, src: &Obj, parent: &str) {
    match v {
        Value::Object(o) => {
            if let Some(Value::String(r)) = o.get_mut("$ref") {
                *r = rewrite_ref(r, src);
            }
            for (k, child) in o.iter_mut() {
                // Example payloads are data and not references.
                if !json::is_data(parent, k) {
                    rewrite_refs(child, src, k);
                }
            }
        }
        Value::Array(a) => {
            for c in a {
                rewrite_refs(c, src, parent);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn rewrite_ref(r: &str, src: &Obj) -> String {
    let Some((file, frag)) = r.split_once('#') else { return r.to_owned() };
    let new_frag = if let Some(rest) = frag.strip_prefix("/definitions/") {
        format!("/components/schemas/{rest}")
    } else if let Some(rest) = frag.strip_prefix("/responses/") {
        format!("/components/responses/{rest}")
    } else if let Some(rest) = frag.strip_prefix("/parameters/") {
        // Only local refs can be classified; external ones are assumed non-body.
        let is_body = file.is_empty()
            && src
                .get("parameters")
                .and_then(|p| p.get(json::unescape_token(rest.split('/').next().unwrap_or(rest))))
                .and_then(|p| p.get("in"))
                .and_then(Value::as_str)
                == Some("body");
        if is_body { format!("/components/requestBodies/{rest}") } else { format!("/components/parameters/{rest}") }
    } else {
        frag.to_owned()
    };
    format!("{file}#{new_frag}")
}

/// Returns a server per scheme at `host` and `base`.
fn servers<'s>(schemes: impl IntoIterator<Item = &'s str>, host: &str, base: &str) -> Value {
    Value::Array(schemes.into_iter().map(|s| json!({"url": format!("{s}://{host}{base}")})).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(doc: Value) -> Value {
        upgrade(doc, &mut Diagnostics::default())
    }

    /// A parameter keeps its `x-` extensions. `x-nullable` moves into its schema as `nullable`.
    #[test]
    fn a_parameter_keeps_its_extensions_but_x_nullable() {
        let out = up(json!({
            "swagger": "2.0", "info": {"title": "t", "version": "1"},
            "paths": {"/a": {"get": {"parameters": [
                {"name": "q", "in": "query", "type": "string", "x-nullable": true, "x-note": "n"}
            ], "responses": {"200": {"description": "ok"}}}}}
        }));
        let p = &out["paths"]["/a"]["get"]["parameters"][0];
        assert_eq!((p.get("x-note"), p.get("x-nullable")), (Some(&json!("n")), None), "{p:#}");
        assert_eq!(p["schema"]["nullable"], json!(true), "{p:#}");
    }

    /// A description may name properties `default`, `enum` and `example`. They hold schemas whose references move too.
    #[test]
    fn a_property_named_like_a_keyword_has_its_reference_rewritten() {
        let out = up(json!({
            "swagger": "2.0", "info": {"title": "t", "version": "1"}, "paths": {},
            "definitions": {
                "JSON": {"description": "any JSON value"},
                "Props": {"type": "object", "properties": {
                    "default": {"$ref": "#/definitions/JSON"},
                    "enum": {"type": "array", "items": {"$ref": "#/definitions/JSON"}},
                    "example": {"$ref": "#/definitions/JSON"}
                }}
            }
        }));
        let props = &out["components"]["schemas"]["Props"]["properties"];
        assert_eq!(props["default"]["$ref"], "#/components/schemas/JSON");
        assert_eq!(props["enum"]["items"]["$ref"], "#/components/schemas/JSON");
        assert_eq!(props["example"]["$ref"], "#/components/schemas/JSON");
    }

    /// A form with a file travels as `multipart/form-data`. Another form type beside it in `consumes` stays.
    #[test]
    fn a_form_with_a_file_is_multipart() {
        let (url, multi) = ("application/x-www-form-urlencoded", "multipart/form-data");
        for (consumes, want) in [(vec![url], vec![multi]), (vec![multi, url], vec![multi, url])] {
            let out = up(json!({
                "swagger": "2.0", "info": {"title": "t", "version": "1"},
                "paths": {"/a": {"post": {"consumes": consumes, "parameters": [{"name": "f", "in": "formData", "type": "file"}],
                    "responses": {"200": {"description": "ok"}}}}}
            }));
            let content = out["paths"]["/a"]["post"]["requestBody"]["content"].as_object().unwrap();
            assert_eq!(content.keys().collect::<Vec<_>>(), want, "{consumes:?}");
        }
    }

    #[test]
    fn host_and_base_path_become_servers() {
        let out = up(json!({"swagger": "2.0", "host": "api.x.com", "basePath": "/v1", "schemes": ["https", "http"]}));
        assert_eq!(out["servers"], json!([{"url": "https://api.x.com/v1"}, {"url": "http://api.x.com/v1"}]));
    }

    #[test]
    fn body_param_becomes_request_body() {
        let out = up(json!({
            "swagger": "2.0", "consumes": ["application/json", "application/xml"],
            "paths": {"/products": {"post": {
                "parameters": [{"in": "body", "name": "product", "required": true, "schema": {"$ref": "#/definitions/Product"}}],
                "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Product"}}}
            }}},
            "definitions": {"Product": {"type": "object", "discriminator": "kind", "x-nullable": true}}
        }));
        let op = &out["paths"]["/products"]["post"];
        assert_eq!(op["requestBody"]["required"], true);
        assert_eq!(op["requestBody"]["content"]["application/xml"]["schema"]["$ref"], "#/components/schemas/Product");
        assert_eq!(
            op["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
            "#/components/schemas/Product"
        );
        assert!(op.get("parameters").is_none());
        let product = &out["components"]["schemas"]["Product"];
        assert_eq!(product["discriminator"], json!({"propertyName": "kind"}));
        assert_eq!(product["nullable"], true);
    }

    #[test]
    fn form_data_becomes_multipart_with_file() {
        let out = up(json!({"swagger": "2.0", "paths": {"/up": {"post": {
            "parameters": [
                {"in": "formData", "name": "file", "type": "file", "required": true},
                {"in": "formData", "name": "note", "type": "string"}
            ],
            "responses": {"204": {"description": "ok"}}
        }}}}));
        let schema = &out["paths"]["/up"]["post"]["requestBody"]["content"]["multipart/form-data"]["schema"];
        assert_eq!(schema["properties"]["file"], json!({"type": "string", "format": "binary"}));
        assert_eq!(schema["required"], json!(["file"]));
    }

    #[test]
    fn collection_formats_map_to_styles() {
        let out = up(json!({"swagger": "2.0", "paths": {"/a": {"get": {
            "parameters": [
                {"in": "query", "name": "csv", "type": "array", "items": {"type": "string"}},
                {"in": "query", "name": "multi", "type": "array", "collectionFormat": "multi", "items": {"type": "integer"}},
                {"in": "query", "name": "pipes", "type": "array", "collectionFormat": "pipes", "items": {"type": "string"}},
                {"in": "header", "name": "h", "type": "array", "items": {"type": "string"}}
            ],
            "responses": {"200": {"description": "ok"}}
        }}}}));
        let ps = &out["paths"]["/a"]["get"]["parameters"];
        assert_eq!((ps[0]["style"].clone(), ps[0]["explode"].clone()), (json!("form"), json!(false)));
        assert_eq!((ps[1]["style"].clone(), ps[1]["explode"].clone()), (json!("form"), json!(true)));
        assert_eq!(ps[2]["style"], "pipeDelimited");
        assert!(ps[3].get("style").is_none());
        assert_eq!(ps[1]["schema"], json!({"type": "array", "items": {"type": "integer"}}));
    }

    #[test]
    fn global_params_and_refs_are_relocated() {
        let out = up(json!({
            "swagger": "2.0",
            "parameters": {
                "Limit": {"in": "query", "name": "limit", "type": "integer"},
                "Body": {"in": "body", "name": "b", "schema": {"type": "object"}}
            },
            "paths": {"/a": {"post": {
                "parameters": [{"$ref": "#/parameters/Limit"}, {"$ref": "#/parameters/Body"}],
                "responses": {"200": {"$ref": "#/responses/Ok"}}
            }}},
            "responses": {"Ok": {"description": "ok"}}
        }));
        let op = &out["paths"]["/a"]["post"];
        assert_eq!(op["parameters"], json!([{"$ref": "#/components/parameters/Limit"}]));
        assert_eq!(op["requestBody"], json!({"$ref": "#/components/requestBodies/Body"}));
        assert_eq!(op["responses"]["200"], json!({"$ref": "#/components/responses/Ok"}));
        assert_eq!(out["components"]["parameters"]["Limit"]["schema"], json!({"type": "integer"}));
    }

    #[test]
    fn oauth_flows_are_renamed() {
        let out = up(json!({"swagger": "2.0", "securityDefinitions": {
            "o": {"type": "oauth2", "flow": "accessCode", "authorizationUrl": "a", "tokenUrl": "t", "scopes": {"r": "read"}},
            "b": {"type": "basic"}
        }}));
        let s = &out["components"]["securitySchemes"];
        assert_eq!(s["o"]["flows"]["authorizationCode"]["tokenUrl"], "t");
        assert_eq!(s["b"], json!({"type": "http", "scheme": "basic"}));
    }
}
