//! The 3.2 rules beyond what parsing enforces. They run on the typed model that every 3.x document is upgraded to.
//!
//! Driven by the official OAI test suite (`tests/fixtures/oai-v32/fail`).
//! Every rule records a diagnostic and continues.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json::Doc;
use crate::json::join;
use crate::version::Version;

use super::FIXED_METHODS;
use crate::spec::{
    Components, Encoding, Entries, Example, Extra, Header, Map, MediaType, OpenApi, Operation, Parameter,
    ParameterLocation, ParameterStyle, PathItem, RefOr, RequestBody, Response, Schema, SecurityRequirement,
    SecurityScheme, SecuritySchemeType, Server, from_value_as,
};

/// Checks the rules parsing cannot enforce and records every violation.
pub(crate) fn validate(doc: &OpenApi, resolved: &Doc, diags: &mut Diagnostics) {
    validate_as(doc, resolved, Version::V32, diags);
}

/// Runs [`validate`] for a document written as `version` and upgraded to 3.2:
/// the `openapi` field is checked by [`super::newer_fields`] on the original instead.
pub(crate) fn validate_as(doc: &OpenApi, resolved: &Doc, version: Version, diags: &mut Diagnostics) {
    let mut v = V { diags, resolved, op_ids: BTreeMap::new(), links: vec![], version };
    v.root(doc);
    super::overlap::check(resolved, diags);
    for (ptr, message) in super::schema_rules::errors(&resolved.root) {
        diags.error(&ptr, message);
    }
}

struct V<'a> {
    diags: &'a mut Diagnostics,
    resolved: &'a Doc,
    op_ids: BTreeMap<String, String>,
    /// Link `operationId`s with their pointers. They are checked once every operation is known.
    links: Vec<(String, String)>,
    /// Version the document was written in.
    version: Version,
}

fn is_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c))
}

fn is_component_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
}

impl V<'_> {
    fn unknown(&mut self, extra: &Extra, ptr: &str) {
        for k in extra.keys() {
            if !k.starts_with("x-") {
                self.diags.refuse_server(join(ptr, k), format!("unknown field `{k}`"));
            }
        }
    }

    fn root(&mut self, d: &OpenApi) {
        let ok_version =
            d.openapi.strip_prefix("3.2.").and_then(|rest| rest.split('-').next()).is_some_and(crate::text::is_digits);
        if !ok_version && self.version == Version::V32 {
            self.diags.error("/openapi", format!("`{}` is not a valid 3.2.x version", d.openapi));
        }
        if d.paths.is_none() && d.components.is_none() && d.webhooks.is_none() {
            self.diags.error("", "a document needs at least one of `paths`, `components` or `webhooks`");
        }
        self.unknown(&d.extra, "");
        self.unknown(&d.info.extra, "/info");
        if let Some(l) = &d.info.license {
            self.unknown(&l.extra, "/info/license");
            if l.identifier.is_some() && l.url.is_some() {
                self.diags.error("/info/license", "`identifier` and `url` are mutually exclusive");
            }
        }
        if let Some(c) = &d.info.contact {
            self.unknown(&c.extra, "/info/contact");
        }
        self.servers(d.servers.as_deref(), "/servers");
        let mut tag_names = BTreeSet::new();
        for (i, t) in indexed(d.tags.as_deref()) {
            let ptr = format!("/tags/{i}");
            self.unknown(&t.extra, &ptr);
            if !tag_names.insert(t.name.as_str()) {
                self.diags.error(join(&ptr, "name"), format!("duplicate tag `{}`", t.name));
            }
        }
        // Tag `parent` is 3.2; older documents get one `newer_fields` error instead.
        for (i, t) in indexed(d.tags.as_deref()).filter(|_| self.version == Version::V32) {
            if let Some(p) = &t.parent
                && !tag_names.contains(p.as_str())
            {
                self.diags.error(format!("/tags/{i}/parent"), format!("parent tag `{p}` is not defined"));
            }
        }
        let schemes: BTreeSet<String> =
            d.components.iter().flat_map(|c| c.security_schemes.iter().flatten()).map(|(k, _)| k.clone()).collect();
        self.security(d.security.as_deref(), &schemes, "/security");
        if let Some(paths) = &d.paths {
            for (path, item) in paths.iter() {
                let ptr = join("/paths", path);
                if !path.starts_with('/') {
                    self.diags.error(&ptr, "path must start with `/`");
                }
                self.path_item(item, &ptr, Some(path), &schemes);
            }
        }
        for (k, item) in d.webhooks.iter().flatten() {
            self.path_item(item, &join("/webhooks", k), None, &schemes);
        }
        if let Some(c) = &d.components {
            self.components(c, &schemes);
        }
        for (id, ptr) in std::mem::take(&mut self.links) {
            if !self.op_ids.contains_key(&id) {
                // Warning: the description may span documents this file does not include.
                self.diags.warning(join(&ptr, "operationId"), format!("no operation has operationId `{id}`"));
            }
        }
        if self.version == Version::V32 {
            self.tag_cycles(d);
        }
        if let Some(paths) = &d.paths {
            self.path_shapes(paths);
        }
    }

    /// Checks that `parent` chains do not loop.
    fn tag_cycles(&mut self, d: &OpenApi) {
        let parents: BTreeMap<&str, &str> =
            d.tags.iter().flatten().filter_map(|t| t.parent.as_deref().map(|p| (t.name.as_str(), p))).collect();
        for (i, t) in indexed(d.tags.as_deref()) {
            let mut cur = t.name.as_str();
            for _ in 0..=parents.len() {
                match parents.get(cur) {
                    Some(&p) if p == t.name => {
                        self.diags.error(format!("/tags/{i}/parent"), format!("tag `{}` is its own ancestor", t.name));
                        break;
                    }
                    Some(&p) => cur = p,
                    None => break,
                }
            }
        }
    }

    /// Checks paths: two that differ only in variable names are the same path, and a variable
    /// must not appear twice in one template.
    fn path_shapes(&mut self, paths: &Entries<PathItem>) {
        let mut shapes: BTreeMap<String, &str> = BTreeMap::new();
        for (path, _) in paths.iter() {
            let mut shape = String::new();
            let mut vars = BTreeSet::new();
            let mut rest = path.as_str();
            while let Some((before, after)) = rest.split_once('{') {
                shape.push_str(before);
                shape.push_str("{}");
                let Some((var, tail)) = after.split_once('}') else { break };
                if !vars.insert(var) {
                    self.diags.error(join("/paths", path), format!("path variable `{var}` appears twice"));
                }
                rest = tail;
            }
            shape.push_str(rest);
            if let Some(other) = shapes.insert(shape, path) {
                self.diags.error(join("/paths", path), format!("`{path}` and `{other}` are the same templated path"));
            }
        }
    }

    fn servers(&mut self, servers: Option<&[Server]>, ptr: &str) {
        for (i, s) in indexed(servers) {
            let ptr = format!("{ptr}/{i}");
            self.unknown(&s.extra, &ptr);
            // The variables' braces are not part of the URL syntax.
            let mut url = s.url.clone();
            let mut seen = BTreeSet::new();
            while let Some((before, after)) = url.clone().split_once('{') {
                let Some((var, tail)) = after.split_once('}') else { break };
                if !seen.insert(var.to_owned()) {
                    self.diags.error(join(&ptr, "url"), format!("server variable `{var}` appears twice"));
                }
                url = format!("{before}x{tail}");
            }
            if url.contains('?') || url.contains('#') {
                self.diags.error(join(&ptr, "url"), "a server URL must not have a query or a fragment");
            }
            for (name, var) in s.variables.iter().flatten() {
                let vptr = join(&join(&ptr, "variables"), name);
                self.unknown(&var.extra, &vptr);
                if let Some(values) = &var.enum_values {
                    if values.is_empty() {
                        self.diags.error(join(&vptr, "enum"), "`enum` must not be empty");
                    } else if !values.contains(&var.default) {
                        self.diags.error(join(&vptr, "default"), "`default` must be one of the `enum` values");
                    }
                }
            }
        }
    }

    fn security(&mut self, reqs: Option<&[SecurityRequirement]>, schemes: &BTreeSet<String>, ptr: &str) {
        for (i, req) in indexed(reqs) {
            for name in req.keys() {
                // URI references (3.2) that resolve were renamed to component
                // names while bundling. One left points at a document that is
                // not available. It is valid but unchecked. So it is only a warning
                // (generating a server refuses it).
                if schemes.contains(name) {
                } else if name.contains('#') {
                    self.diags.warning(
                        format!("{ptr}/{i}"),
                        format!("security scheme `{name}` does not resolve: a generated server would refuse to build"),
                    );
                } else {
                    self.diags.error(format!("{ptr}/{i}"), format!("security scheme `{name}` is not defined"));
                }
            }
        }
    }

    fn components(&mut self, c: &Components, schemes: &BTreeSet<String>) {
        let base = "/components";
        self.unknown(&c.extra, base);
        macro_rules! names {
            ($field:ident, $key:literal) => {
                for (name, _) in c.$field.iter().flatten() {
                    if !is_component_name(name) {
                        self.diags.error(join(&join(base, $key), name), "component names may only use `A-Za-z0-9.-_`");
                    }
                }
            };
        }
        names!(schemas, "schemas");
        names!(responses, "responses");
        names!(parameters, "parameters");
        names!(examples, "examples");
        names!(request_bodies, "requestBodies");
        names!(headers, "headers");
        names!(security_schemes, "securitySchemes");
        names!(links, "links");
        names!(callbacks, "callbacks");
        names!(path_items, "pathItems");
        names!(media_types, "mediaTypes");
        for (k, s) in c.schemas.iter().flatten() {
            self.schema(s, &join("/components/schemas", k));
        }
        for (k, r) in c.responses.iter().flatten() {
            self.ref_or(r, &join("/components/responses", k), Self::response);
        }
        for (k, p) in c.parameters.iter().flatten() {
            self.ref_or(p, &join("/components/parameters", k), Self::parameter);
        }
        for (k, e) in c.examples.iter().flatten() {
            self.ref_or(e, &join("/components/examples", k), Self::example);
        }
        for (k, b) in c.request_bodies.iter().flatten() {
            self.ref_or(b, &join("/components/requestBodies", k), Self::request_body);
        }
        for (k, h) in c.headers.iter().flatten() {
            self.ref_or(h, &join("/components/headers", k), Self::header);
        }
        for (k, m) in c.media_types.iter().flatten() {
            self.ref_or(m, &join("/components/mediaTypes", k), Self::media_type);
        }
        for (k, l) in c.links.iter().flatten() {
            self.ref_or(l, &join("/components/links", k), |v, l, p| {
                v.unknown(&l.extra, p);
                if let Some(id) = &l.operation_id {
                    v.links.push((id.clone(), p.to_owned()));
                }
            });
        }
        for (k, s) in c.security_schemes.iter().flatten() {
            self.ref_or(s, &join("/components/securitySchemes", k), Self::security_scheme);
        }
        for (k, cb) in c.callbacks.iter().flatten() {
            let ptr = join("/components/callbacks", k);
            if let RefOr::Item(cb) = cb {
                for (expr, item) in cb.iter() {
                    self.path_item(item, &join(&ptr, expr), None, schemes);
                }
            } else {
                self.check_ref(cb.as_ref().map(|r| r.reference.as_str()), &ptr);
            }
        }
        for (k, item) in c.path_items.iter().flatten() {
            self.path_item(item, &join("/components/pathItems", k), None, schemes);
        }
    }

    fn check_ref(&mut self, r: Option<&str>, ptr: &str) {
        if let Some(r) = r
            && r.starts_with('#')
            && self.resolved.get(r).is_none()
        {
            self.diags.error(join(ptr, "$ref"), format!("`{r}` does not resolve"));
        }
    }

    fn ref_or<T>(&mut self, r: &RefOr<T>, ptr: &str, f: impl FnOnce(&mut Self, &T, &str)) {
        match r {
            RefOr::Ref(reference) => {
                self.check_ref(Some(&reference.reference), ptr);
                // Reference Objects allow only $ref, summary and description.
                // The spec says that other siblings "SHALL be ignored".
                for k in reference.extra.keys().filter(|k| !k.starts_with("x-")) {
                    self.diags.warning(join(ptr, k), format!("`{k}` next to `$ref` is ignored"));
                }
            }
            RefOr::Item(t) => f(self, t, ptr),
        }
    }

    fn path_item(&mut self, item: &PathItem, ptr: &str, template: Option<&str>, schemes: &BTreeSet<String>) {
        self.unknown(&item.extra, ptr);
        self.check_ref(item.reference.as_deref(), ptr);
        self.servers(item.servers.as_deref(), &join(ptr, "servers"));
        for (k, _) in item.additional_operations.iter().flatten() {
            if FIXED_METHODS.contains(&k.to_ascii_lowercase().as_str()) {
                self.diags.error(
                    join(&join(ptr, "additionalOperations"), k),
                    format!("`{k}` must use the fixed `{}` field", k.to_ascii_lowercase()),
                );
            }
        }
        let shared = item.parameters.as_deref().unwrap_or_default();
        self.params(shared, &join(ptr, "parameters"));
        self.querystring_rules(shared, &[], &join(ptr, "parameters"));
        for (method, op) in item.operations() {
            let key = if FIXED_METHODS.contains(&method.to_ascii_lowercase().as_str()) {
                method.to_ascii_lowercase()
            } else {
                format!("additionalOperations/{}", crate::json::escape_token(method))
            };
            let optr = format!("{ptr}/{key}");
            self.operation(op, &optr, shared, template, schemes);
        }
    }

    fn operation(
        &mut self,
        op: &Operation<PathItem>,
        ptr: &str,
        shared: &[RefOr<Parameter>],
        template: Option<&str>,
        schemes: &BTreeSet<String>,
    ) {
        self.unknown(&op.extra, ptr);
        if let Some(id) = &op.operation_id
            && let Some(prev) = self.op_ids.insert(id.clone(), ptr.to_owned())
        {
            self.diags.error(join(ptr, "operationId"), format!("operationId `{id}` is also used at {prev}"));
        }
        let own = op.parameters.as_deref().unwrap_or_default();
        self.params(own, &join(ptr, "parameters"));
        self.querystring_rules(shared, own, &join(ptr, "parameters"));
        if let Some(t) = template {
            self.path_params(t, shared, own, ptr);
        }
        if let Some(b) = &op.request_body {
            self.ref_or(b, &join(ptr, "requestBody"), Self::request_body);
        }
        for (code, r) in op.responses.iter().flat_map(Entries::iter) {
            let rptr = join(&join(ptr, "responses"), code);
            let valid = code == "default" || {
                let b = code.as_bytes();
                b.len() == crate::validate::STATUS_DIGITS
                    && b[0].is_ascii_digit()
                    && (b[1..].iter().all(u8::is_ascii_digit) || &b[1..] == b"XX")
            };
            if !valid {
                self.diags.error(&rptr, format!("`{code}` is not a status code, range (`2XX`) or `default`"));
            }
            self.ref_or(r, &rptr, Self::response);
        }
        self.security(op.security.as_deref(), schemes, &join(ptr, "security"));
        self.servers(op.servers.as_deref(), &join(ptr, "servers"));
        for (k, cb) in op.callbacks.iter().flatten() {
            let cptr = join(&join(ptr, "callbacks"), k);
            match cb {
                RefOr::Item(cb) => {
                    for (expr, item) in cb.iter() {
                        self.path_item(item, &join(&cptr, expr), None, schemes);
                    }
                }
                RefOr::Ref(r) => self.check_ref(Some(&r.reference), &cptr),
            }
        }
    }

    fn resolve_param<'p>(&'p self, p: &'p RefOr<Parameter>) -> Option<Parameter> {
        match p {
            RefOr::Item(p) => Some(p.clone()),
            RefOr::Ref(r) => self.resolved.get(&r.reference).and_then(|v| from_value_as(v.clone()).ok()),
        }
    }

    fn params(&mut self, ps: &[RefOr<Parameter>], ptr: &str) {
        let mut seen = BTreeSet::new();
        for (i, p) in ps.iter().enumerate() {
            let pptr = format!("{ptr}/{i}");
            self.ref_or(p, &pptr, Self::parameter);
            if let Some(p) = self.resolve_param(p) {
                // Names are case sensitive (3.2 4.12.1) except a header's (RFC 9110).
                let name =
                    if p.location == ParameterLocation::Header { p.name.to_ascii_lowercase() } else { p.name.clone() };
                let key = (name, p.location);
                if !seen.insert(key) {
                    self.diags.error(&pptr, format!("duplicate parameter `{}` in {:?}", p.name, p.location));
                }
            }
        }
    }

    fn querystring_rules(&mut self, shared: &[RefOr<Parameter>], own: &[RefOr<Parameter>], ptr: &str) {
        // An operation's own parameter overrides a shared one with the same name+in.
        let own_ps: Vec<Parameter> = own.iter().filter_map(|p| self.resolve_param(p)).collect();
        let mut all: Vec<Parameter> = shared
            .iter()
            .filter_map(|p| self.resolve_param(p))
            .filter(|s| !own_ps.iter().any(|o| o.name == s.name && o.location == s.location))
            .collect();
        all.extend(own_ps);
        let qs = all.iter().filter(|p| p.location == ParameterLocation::Querystring).count();
        let q = all.iter().filter(|p| p.location == ParameterLocation::Query).count();
        if qs > 1 {
            self.diags.error(ptr, "at most one `querystring` parameter is allowed");
        }
        if qs > 0 && q > 0 {
            self.diags.error(ptr, "`querystring` and `query` parameters cannot be combined");
        }
    }

    fn path_params(&mut self, template: &str, shared: &[RefOr<Parameter>], own: &[RefOr<Parameter>], ptr: &str) {
        let names: Vec<&str> = template.split('{').skip(1).filter_map(|s| s.split_once('}').map(|(n, _)| n)).collect();
        let declared: Vec<Parameter> = shared
            .iter()
            .chain(own)
            .filter_map(|p| self.resolve_param(p))
            .filter(|p| p.location == ParameterLocation::Path)
            .collect();
        for n in &names {
            if !declared.iter().any(|p| p.name == *n) {
                self.diags.error(ptr, format!("path parameter `{n}` is not declared"));
            }
        }
        for p in &declared {
            if !names.contains(&p.name.as_str()) {
                self.diags.warning(ptr, format!("path parameter `{}` does not appear in the path", p.name));
            }
        }
    }

    /// Returns whether the `type` of a schema allows an array or an object. Local `$ref`s are followed.
    fn is_composite(&self, s: &Schema) -> bool {
        let Ok(mut v) = serde_json::to_value(s) else { return false };
        // Every link of the chain once: a `$ref` met again is a cycle, which has no `type` to find.
        let mut seen = BTreeSet::new();
        while let Some(r) = v.get("$ref").and_then(Value::as_str).map(str::to_owned) {
            match self.resolved.get(&r).filter(|_| seen.insert(r.clone())) {
                Some(target) => v = target.clone(),
                None => break,
            }
        }
        let composite = |t: &Value| matches!(t.as_str(), Some("array" | "object"));
        match v.get("type") {
            Some(Value::Array(ts)) => ts.iter().any(composite),
            Some(t) => composite(t),
            None => false,
        }
    }

    fn parameter(&mut self, p: &Parameter, ptr: &str) {
        self.unknown(&p.extra, ptr);
        // 3.2 section 4.12.5 Style Values: "Combinations not represented in this
        // table are not permitted."
        if let Some(style) = p.style {
            use ParameterLocation as L;
            use ParameterStyle as S;
            let allowed: &[ParameterLocation] = match style {
                S::Matrix | S::Label => &[L::Path],
                S::Simple => &[L::Path, L::Header],
                S::Form => &[L::Query, L::Cookie],
                S::SpaceDelimited | S::PipeDelimited | S::DeepObject => &[L::Query],
                S::Cookie => &[L::Cookie],
            };
            if !allowed.contains(&p.location) {
                self.diags.error(
                    join(ptr, "style"),
                    format!(
                        "style {} is not allowed for `in: {}` parameters",
                        serde_json::to_string(&style).unwrap_or_default(),
                        serde_json::to_string(&p.location).unwrap_or_default().trim_matches('"'),
                    ),
                );
            }
        }
        match p.location {
            ParameterLocation::Path => {
                // A spec MUST. But the official schema (and many real specs) tolerate it.
                if p.required != Some(true) {
                    self.diags.warning(join(ptr, "required"), "path parameters must have `required: true`");
                }
                if p.name.contains(['{', '}']) {
                    self.diags.error(join(ptr, "name"), "path parameter names cannot contain `{` or `}`");
                }
            }
            ParameterLocation::Header => {
                if !is_token(&p.name) {
                    self.diags.error(join(ptr, "name"), format!("`{}` is not a valid header name", p.name));
                }
                if p.allow_reserved.is_some() {
                    self.diags
                        .error(join(ptr, "allowReserved"), "`allowReserved` is not allowed for header parameters");
                }
            }
            ParameterLocation::Cookie => {
                if p.style == Some(ParameterStyle::Cookie) && p.allow_reserved.is_some() {
                    self.diags.error(join(ptr, "allowReserved"), "`allowReserved` is not allowed with `style: cookie`");
                }
                // 3.2.1 section 4.12.5.3: "The Cookie header prohibits unencoded commas,
                // therefore `explode: false` is invalid with `in: cookie`". An
                // array or object would join its items with commas. 3.2.0 and
                // earlier allowed it. So a document written for them only gets a warning.
                if p.explode == Some(false) && p.schema.as_ref().is_some_and(|s| self.is_composite(s)) {
                    let msg = "`explode: false` joins items with commas, which the Cookie header forbids";
                    if self.version == Version::V32 {
                        self.diags.error(join(ptr, "explode"), msg);
                    } else {
                        self.diags.warning(join(ptr, "explode"), msg);
                    }
                }
            }
            ParameterLocation::Querystring => {
                if p.schema.is_some() || p.content.is_none() {
                    self.diags.error(ptr, "`querystring` parameters must use `content`, not `schema`");
                }
            }
            ParameterLocation::Query => {}
        }
        match (&p.schema, &p.content) {
            (Some(_), Some(_)) => self.diags.error(ptr, "`schema` and `content` are mutually exclusive"),
            (None, None) => self.diags.error(ptr, "a parameter needs `schema` or `content`"),
            (None, Some(c)) => {
                if c.len() != 1 {
                    self.diags.error(join(ptr, "content"), "`content` must have exactly one entry");
                }
                if p.style.is_some() || p.explode.is_some() || p.allow_reserved.is_some() {
                    self.diags.error(ptr, "`style`, `explode` and `allowReserved` cannot be used with `content`");
                }
            }
            _ => {}
        }
        if let Some(s) = &p.schema {
            self.schema(s, &join(ptr, "schema"));
        }
        self.examples(p.example.as_ref(), p.examples.as_ref(), ptr);
        self.content(p.content.as_ref(), ptr);
    }

    fn header(&mut self, h: &Header<MediaType>, ptr: &str) {
        self.unknown(&h.extra, ptr);
        if h.style.is_some_and(|s| s != ParameterStyle::Simple) {
            self.diags.error(join(ptr, "style"), "header `style` must be `simple`");
        }
        if h.schema.is_some() && h.content.is_some() {
            self.diags.error(ptr, "`schema` and `content` are mutually exclusive");
        }
        if let Some(s) = &h.schema {
            self.schema(s, &join(ptr, "schema"));
        }
        self.examples(h.example.as_ref(), h.examples.as_ref(), ptr);
        self.content(h.content.as_ref(), ptr);
    }

    fn headers(&mut self, hs: Option<&Map<RefOr<Header<MediaType>>>>, ptr: &str) {
        for (name, h) in hs.into_iter().flatten() {
            let hptr = join(&join(ptr, "headers"), name);
            if !is_token(name) {
                self.diags.error(&hptr, format!("`{name}` is not a valid header name"));
            }
            self.ref_or(h, &hptr, Self::header);
        }
    }

    fn examples(&mut self, example: Option<&Value>, examples: Option<&Map<RefOr<Example>>>, ptr: &str) {
        if example.is_some() && examples.is_some() {
            self.diags.error(ptr, "`example` and `examples` are mutually exclusive");
        }
        for (k, e) in examples.into_iter().flatten() {
            self.ref_or(e, &join(&join(ptr, "examples"), k), Self::example);
        }
    }

    fn example(&mut self, e: &Example, ptr: &str) {
        self.unknown(&e.extra, ptr);
        let pairs = [
            (e.value.is_some(), e.external_value.is_some(), "value", "externalValue"),
            (e.value.is_some(), e.data_value.is_some(), "value", "dataValue"),
            (e.value.is_some(), e.serialized_value.is_some(), "value", "serializedValue"),
            (e.serialized_value.is_some(), e.external_value.is_some(), "serializedValue", "externalValue"),
        ];
        for (a, b, an, bn) in pairs {
            if a && b {
                self.diags.error(ptr, format!("`{an}` and `{bn}` are mutually exclusive"));
            }
        }
    }

    fn request_body(&mut self, b: &RequestBody, ptr: &str) {
        self.unknown(&b.extra, ptr);
        if b.content.is_none() {
            self.diags.error(ptr, "a request body needs `content`");
        }
        self.content(b.content.as_ref(), ptr);
    }

    fn response(&mut self, r: &Response, ptr: &str) {
        self.unknown(&r.extra, ptr);
        self.headers(r.headers.as_ref(), ptr);
        self.content(r.content.as_ref(), ptr);
        for (k, l) in r.links.iter().flatten() {
            self.ref_or(l, &join(&join(ptr, "links"), k), |v, l, p| {
                v.unknown(&l.extra, p);
                if let Some(id) = &l.operation_id {
                    v.links.push((id.clone(), p.to_owned()));
                }
                if l.operation_id.is_some() && l.operation_ref.is_some() {
                    v.diags.error(p, "`operationId` and `operationRef` are mutually exclusive");
                }
            });
        }
    }

    fn content(&mut self, c: Option<&Map<RefOr<MediaType>>>, ptr: &str) {
        for (k, m) in c.into_iter().flatten() {
            self.ref_or(m, &join(&join(ptr, "content"), k), Self::media_type);
        }
    }

    fn media_type(&mut self, m: &MediaType, ptr: &str) {
        self.unknown(&m.extra, ptr);
        if m.encoding.is_some() && (m.prefix_encoding.is_some() || m.item_encoding.is_some()) {
            self.diags.error(ptr, "`encoding` cannot be combined with `prefixEncoding` or `itemEncoding`");
        }
        for key in [&m.schema, &m.item_schema].into_iter().flatten() {
            self.schema(key, &join(ptr, "schema"));
        }
        self.examples(m.example.as_ref(), m.examples.as_ref(), ptr);
        for (k, e) in m.encoding.iter().flatten() {
            self.encoding(e, &join(&join(ptr, "encoding"), k));
        }
        for (i, e) in indexed(m.prefix_encoding.as_deref()) {
            self.encoding(e, &format!("{ptr}/prefixEncoding/{i}"));
        }
        if let Some(e) = &m.item_encoding {
            self.encoding(e, &join(ptr, "itemEncoding"));
        }
    }

    fn encoding(&mut self, e: &Encoding<Header<MediaType>>, ptr: &str) {
        self.unknown(&e.extra, ptr);
        if e.encoding.is_some() && (e.prefix_encoding.is_some() || e.item_encoding.is_some()) {
            self.diags.error(ptr, "`encoding` cannot be combined with `prefixEncoding` or `itemEncoding`");
        }
        self.headers(e.headers.as_ref(), ptr);
        for (k, inner) in e.encoding.iter().flatten() {
            self.encoding(inner, &join(&join(ptr, "encoding"), k));
        }
        for (i, inner) in indexed(e.prefix_encoding.as_deref()) {
            self.encoding(inner, &format!("{ptr}/prefixEncoding/{i}"));
        }
        if let Some(inner) = &e.item_encoding {
            self.encoding(inner, &join(ptr, "itemEncoding"));
        }
    }

    fn security_scheme(&mut self, s: &SecurityScheme, ptr: &str) {
        self.unknown(&s.extra, ptr);
        let need = |v: &mut Self, ok: bool, field: &str| {
            if !ok {
                v.diags.refuse_server(ptr, format!("`{field}` is required for this security scheme type"));
            }
        };
        match s.kind {
            SecuritySchemeType::ApiKey => {
                need(self, s.name.is_some(), "name");
                need(self, s.location.is_some(), "in");
                if let Some(place) =
                    s.location.as_ref().filter(|p| !matches!(p.as_str(), "header" | "query" | "cookie"))
                {
                    self.diags.refuse_server(
                        ptr,
                        format!("`in` is `{place}`: an API key goes in `header`, `query` or `cookie`"),
                    );
                }
            }
            SecuritySchemeType::Http => need(self, s.scheme.is_some(), "scheme"),
            SecuritySchemeType::OAuth2 => need(self, s.flows.is_some(), "flows"),
            SecuritySchemeType::OpenIdConnect => need(self, s.open_id_connect_url.is_some(), "openIdConnectUrl"),
            SecuritySchemeType::MutualTls => {}
        }
        if let Some(f) = &s.flows {
            self.unknown(&f.extra, &join(ptr, "flows"));
        }
    }

    /// Schemas are open (unknown keywords allowed); only OAS dialect objects are checked.
    fn schema(&mut self, s: &Schema, ptr: &str) {
        let Schema::Object(o) = s else { return };
        self.check_ref(o.reference.as_deref(), ptr);
        // JSON Schema 2020-12 section 6.3.3: `pattern` SHOULD be an ECMA-262 regular
        // expression. Generated code compiles it with `regress` (ECMA-262 with the
        // `u` flag or else without it). One that compiles neither way cannot be checked.
        let patterns =
            o.pattern.iter().map(|p| (join(ptr, "pattern"), p)).chain(
                o.pattern_properties.iter().flatten().map(|(p, _)| (join(&join(ptr, "patternProperties"), p), p)),
            );
        // A type that claims a pattern it cannot check would accept every value: `generate` refuses the document.
        for (at, p) in patterns {
            if let Some(e) = pattern_error(p) {
                self.diags.error(at, format!("pattern `{p}` is not an ECMA-262 regular expression ({e})"));
            }
        }
        if let Some(x) = &o.xml {
            let xptr = join(ptr, "xml");
            self.unknown(&x.extra, &xptr);
            if x.node_type.is_some() && (x.attribute.is_some() || x.wrapped.is_some()) {
                self.diags.error(&xptr, "`nodeType` cannot be combined with `attribute` or `wrapped`");
            }
            if let Some(n) = &x.node_type
                && !["element", "attribute", "text", "cdata", "none"].contains(&n.as_str())
            {
                self.diags.error(join(&xptr, "nodeType"), format!("unknown nodeType `{n}`"));
            }
        }
        if let Some(d) = &o.discriminator {
            self.unknown(&d.extra, &join(ptr, "discriminator"));
            for (k, target) in d.mapping.iter().flatten() {
                if target.starts_with('#') && self.resolved.get(target).is_none() {
                    self.diags.error(
                        join(&join(&join(ptr, "discriminator"), "mapping"), k),
                        format!("`{target}` does not resolve"),
                    );
                }
            }
        }
        let subs =
            [&o.not, &o.if_, &o.then, &o.else_, &o.items, &o.contains, &o.additional_properties, &o.property_names]
                .into_iter()
                .flatten();
        for sub in subs {
            self.schema(sub, ptr);
        }
        for list in [&o.all_of, &o.any_of, &o.one_of, &o.prefix_items].into_iter().flatten() {
            for sub in list {
                self.schema(sub, ptr);
            }
        }
        for map in [&o.properties, &o.pattern_properties, &o.defs].into_iter().flatten() {
            for (k, sub) in map {
                self.schema(sub, &join(ptr, k));
            }
        }
    }
}

/// Returns the items of an optional list with their index.
pub(crate) fn indexed<T>(items: Option<&[T]>) -> impl Iterator<Item = (usize, &T)> {
    items.into_iter().flatten().enumerate()
}

/// Returns why `pattern` does not compile as generated code compiles it: with `regress`, as ECMA-262 with the `u` flag
/// or else without it. `None` when it compiles.
pub(crate) fn pattern_error(pattern: &str) -> Option<String> {
    regress::Regex::with_flags(pattern, "u").or_else(|_| regress::Regex::new(pattern)).err().map(|e| e.to_string())
}

/// Does a schema of the document contain a `pattern` or a `patternProperties` key that does not compile? Generated code
/// could not check it: a value of that schema would pass whatever it is. Data (examples, enums, defaults) does not
/// count.
pub(crate) fn has_uncompilable_pattern(v: &Value, parent: &str) -> bool {
    match v {
        Value::Object(o) => o.iter().any(|(k, c)| {
            (k == "pattern" && parent != "properties" && c.as_str().is_some_and(|p| pattern_error(p).is_some()))
                || (k == "patternProperties"
                    && c.as_object().is_some_and(|ps| ps.keys().any(|p| pattern_error(p).is_some())))
                || (!crate::json::is_data(parent, k) && has_uncompilable_pattern(c, k))
        }),
        Value::Array(a) => a.iter().any(|c| has_uncompilable_pattern(c, parent)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}
