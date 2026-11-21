//! Intermediate representation: types and operations built from a bundled 3.2 document for any target language.

pub(crate) mod checked;
pub(crate) mod dynamic;
pub(crate) mod http;
pub(crate) mod media;
pub(crate) mod names;
pub(crate) mod refs;
pub(crate) mod security;
pub(crate) mod types;

use crate::diag::Diagnostics;
use crate::json::Doc;
use names::Reserved;

pub(crate) use http::Http;
pub(crate) use security::SecurityScheme;
pub(crate) use types::TypeSpace;

#[derive(Debug, Clone)]
/// Everything that the emitters need. It does not depend on the shape of the spec.
pub(crate) struct Api {
    /// What the description says about itself.
    pub info: Info,
    /// Servers with URLs as written (variables unexpanded).
    pub servers: Vec<Server>,
    /// Every named type.
    pub types: TypeSpace,
    /// HTTP operations.
    pub http: Http,
    /// Security schemes from `components.securitySchemes`.
    pub schemes: Vec<SecurityScheme>,
    /// Top-level `tags` plus tags that operations use without declaring them.
    pub tags: Vec<Tag>,
}

/// The Info Object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Info {
    /// `title`.
    pub title: String,
    /// `version`.
    pub version: String,
    /// `summary` and `description` and then terms of service, contact, license and external docs.
    pub description: Option<String>,
}

/// A tag: a group of operations (3.2: nested through `parent`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tag {
    /// `name`.
    pub name: String,
    /// `summary` (3.2) or else `description`.
    pub doc: Option<String>,
    /// `parent` (3.2): the enclosing tag's name.
    pub parent: Option<String>,
}

/// A server URL template and its variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Server {
    /// URL as written with `{name}` for variables.
    pub url: String,
    /// `name` (3.2) or else `description`.
    pub name: Option<String>,
    /// Variables as (name, default, allowed values). Empty allowed values mean any.
    pub variables: Vec<(String, String, Vec<String>)>,
    /// `description` and then the `description` of each variable.
    pub doc: Option<String>,
}

/// Builds the IR of a bundled 3.2 document.
#[cfg(test)]
pub(crate) fn build(doc: &Doc, diags: &mut Diagnostics) -> Api {
    build_with(doc, diags, false, false, Reserved::default(), &[], false)
}

/// Returns the IR of a bundled 3.2 document, built with the options of the generator.
///
/// `lenient`: response types accept an enum value or a property that the description does not list (`client = { strict
/// = false }`). `keep_unknown`: objects without `additionalProperties` keep unknown keys. `from_v20` (the description
/// was Swagger 2.0): every optional property decodes `null` as absent. `reserved`: names that the target language keeps
/// for itself. Nothing generated uses them.
pub(crate) fn build_with(
    doc: &Doc,
    diags: &mut Diagnostics,
    keep_unknown: bool,
    from_v20: bool,
    reserved: Reserved<'_>,
    crates: &[String],
    lenient: bool,
) -> Api {
    dialects(&doc.root, "", "", diags);
    // `$dynamicRef` generics become one specialized schema per binding site.
    let special = dynamic::monomorphize(doc);
    let doc = special.as_ref().unwrap_or(doc);
    let mut b = types::Builder::new(doc, diags, reserved);
    b.keep_unknown = keep_unknown;
    b.crates = crates.iter().map(|c| c.replace('-', "_")).collect();
    b.from_v20 = from_v20;
    b.components();
    let schemes = security::schemes(&mut b);
    let mut http = Http::build(&mut b, &schemes);
    let mut types = b.finish();
    http.use_request_types(&types.split_directions(lenient));
    // After the split: each direction's type wraps its own fields.
    checked::wrap(&mut types, &mut http);
    let servers = doc
        .root
        .get("servers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(http::server).collect())
        .unwrap_or_default();
    let tags = tags(&doc.root, http.operations.iter().flat_map(|o| o.tags.iter().map(String::as_str)));
    Api { info: info(&doc.root), tags, servers, types, http, schemes }
}

/// Returns the top-level `tags` of `root` and then each name in `used` that they do not declare.
fn tags<'a>(root: &serde_json::Value, used: impl IntoIterator<Item = &'a str>) -> Vec<Tag> {
    let mut tags: Vec<Tag> = root
        .get("tags")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|t| {
            let text = |k: &str| t.get(k).and_then(serde_json::Value::as_str).map(String::from);
            let both = match (text("summary"), text("description")) {
                (Some(s), Some(d)) => Some(format!("{s}\n\n{d}")),
                (s, d) => s.or(d),
            };
            let doc = types::with_external_docs(both, t);
            Some(Tag { name: text("name")?, doc, parent: text("parent") })
        })
        .collect();
    for name in used {
        if !tags.iter().any(|t| t.name == name) {
            tags.push(Tag { name: name.to_owned(), doc: None, parent: None });
        }
    }
    tags
}

/// Returns the Info Object of `root`.
fn info(root: &serde_json::Value) -> Info {
    let info = root.get("info");
    let s = |k: &str| info.and_then(|i| i.get(k)).and_then(|v| v.as_str()).map(String::from);
    Info {
        title: s("title").unwrap_or_default(),
        version: s("version").unwrap_or_default(),
        description: types::with_external_docs(info_doc(info), root),
    }
}

/// Returns `info.description` and then terms of service, contact and license.
fn info_doc(info: Option<&serde_json::Value>) -> Option<String> {
    let text = |p: &str| {
        info.and_then(|i| i.pointer(p)).and_then(serde_json::Value::as_str).map(str::trim).filter(|t| !t.is_empty())
    };
    let mut lines = vec![];
    if let Some(t) = text("/termsOfService") {
        lines.push(format!("Terms of service: {t}"));
    }
    let contact: Vec<&str> = ["/contact/name", "/contact/email", "/contact/url"].into_iter().filter_map(text).collect();
    if !contact.is_empty() {
        lines.push(format!("Contact: {}", contact.join(", ")));
    }
    let license: Vec<&str> =
        ["/license/name", "/license/identifier", "/license/url"].into_iter().filter_map(text).collect();
    if !license.is_empty() {
        lines.push(format!("License: {}", license.join(", ")));
    }
    let mut parts: Vec<String> = ["/summary", "/description"].into_iter().filter_map(text).map(String::from).collect();
    if !lines.is_empty() {
        // One paragraph with one line each.
        parts.push(lines.join("  \n"));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

/// Returns JSON Schema dialects whose keywords the generator interprets correctly: the OAS
/// dialects and JSON Schema drafts 6 to 2020-12.
fn known_dialect(uri: &str) -> bool {
    let u = uri.trim_end_matches('#').trim_end_matches('/');
    let u = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).unwrap_or(u);
    u.starts_with("spec.openapis.org/oas/3.")
        || [
            "json-schema.org/draft/2020-12/schema",
            "json-schema.org/draft/2019-09/schema",
            "json-schema.org/draft-07/schema",
            "json-schema.org/draft-06/schema",
        ]
        .contains(&u)
}

/// Warns about a `jsonSchemaDialect` or `$schema` that the generator does not support.
/// Also warn about `$vocabulary`. Schemas are always interpreted with the OAS dialect.
fn dialects(v: &serde_json::Value, ptr: &str, parent: &str, diags: &mut Diagnostics) {
    match v {
        serde_json::Value::Object(o) => {
            for key in ["jsonSchemaDialect", "$schema"].iter().filter(|k| ptr.is_empty() || **k == "$schema") {
                if let Some(uri) = o.get(*key).and_then(serde_json::Value::as_str).filter(|u| !known_dialect(u)) {
                    diags.warning(
                        crate::json::join(ptr, key),
                        format!("dialect `{uri}` is not supported: schemas are interpreted with the OpenAPI dialect (JSON Schema 2020-12)"),
                    );
                }
            }
            if o.contains_key("$vocabulary") {
                diags.warning(
                    crate::json::join(ptr, "$vocabulary"),
                    "`$vocabulary` is not applied: schemas are interpreted with every OpenAPI dialect keyword",
                );
            }
            for (k, c) in o {
                if !crate::json::is_data(parent, k) {
                    dialects(c, &crate::json::join(ptr, k), k, diags);
                }
            }
        }
        serde_json::Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                dialects(c, &format!("{ptr}/{i}"), parent, diags);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}
