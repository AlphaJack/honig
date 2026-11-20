//! `$ref`s inside schemas resolve against the base URI that `$id` sets (JSON Schema 2020-12 section 8.2, which
//! `OpenAPI` 3.1 and 3.2 adopt for Schema Objects). `{"$id": "http://x/tree", "$defs": {...}, "$ref": "#/$defs/node"}`
//! means `http://x/tree#/$defs/node`, not the document's `#/$defs/node`. `list` beside it means `http://x/list`.
//!
//! Before bundling, each schema `$ref` is resolved against its base. One that refers to a resource of the document (a
//! schema with that `$id`, or an `$anchor` in one) becomes a local pointer to it. Any other becomes absolute, and the
//! bundler fetches or reads it. A `$ref` with no `$id` above it is left as written.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::walk::{self, Obj, Visit};

/// The base of a document without `$self`: refs that resolve against it are left as written.
const DOCUMENT: &str = "honig-document:/";

/// What one schema declares, by its pointer.
#[derive(Default)]
struct Declared {
    id: Option<String>,
    anchors: Vec<String>,
    reference: Option<String>,
    dynamic_reference: Option<String>,
    dynamic_anchor: Option<String>,
}

/// The schema resources of a document and their dynamic anchors, for resolving `$dynamicRef` in a dynamic scope.
#[derive(Debug, Default, Clone)]
pub(crate) struct Dynamic {
    /// Pointers of the schemas with an `$id`.
    pub resources: Vec<String>,
    /// (resource pointer, `$dynamicAnchor` name) to the pointer of the schema declaring it. The document is resource
    /// `""`.
    pub anchors: BTreeMap<(String, String), String>,
}

impl Dynamic {
    /// Returns the resource that holds the schema at `ptr`: the nearest schema with an `$id` at or above it, or the
    /// document.
    pub(crate) fn resource_of(&self, ptr: &str) -> String {
        self.resources
            .iter()
            .filter(|r| {
                ptr == r.as_str() || (ptr.starts_with(r.as_str()) && ptr.as_bytes().get(r.len()) == Some(&b'/'))
            })
            .max_by_key(|r| r.len())
            .cloned()
            .unwrap_or_default()
    }
}

/// Returns the resources and dynamic anchors of `root` (an `OpenAPI` document).
pub(crate) fn dynamic(root: &Value) -> Dynamic {
    let mut found = Collect::default();
    walk::walk(&mut root.clone(), &mut found);
    let mut out = Dynamic {
        resources: found.schemas.iter().filter(|(_, d)| d.id.is_some()).map(|(p, _)| p.clone()).collect(),
        anchors: BTreeMap::new(),
    };
    for (ptr, d) in &found.schemas {
        if let Some(a) = &d.dynamic_anchor {
            let resource = out.resource_of(ptr);
            out.anchors.entry((resource, a.clone())).or_insert_with(|| ptr.clone());
        }
    }
    out
}

#[derive(Default)]
struct Collect {
    schemas: BTreeMap<String, Declared>,
}

impl Visit for Collect {
    fn schema(&mut self, s: &mut Obj, ptr: &str) {
        let text = |k: &str| s.get(k).and_then(Value::as_str).map(String::from);
        let anchors = ["$anchor", "$dynamicAnchor"].iter().filter_map(|k| text(k)).collect();
        self.schemas.insert(
            ptr.to_owned(),
            Declared {
                id: text("$id"),
                anchors,
                reference: text("$ref"),
                dynamic_reference: text("$dynamicRef"),
                dynamic_anchor: text("$dynamicAnchor"),
            },
        );
    }
}

/// Rewrites the schema `$ref`s of `root` that `$id`s give another base. See the module.
pub(crate) fn rebase(root: &mut Value) {
    let doc_base = root.get("$self").and_then(Value::as_str).map(String::from);
    rebase_from(root, doc_base.as_deref());
}

/// Runs [`rebase`] for a document retrieved from `uri` (a fetched or read schema file): its root is a resource at
/// `uri`, so `#foo` in it finds its `$anchor`.
pub(crate) fn rebase_fetched(root: &mut Value, uri: &str) {
    rebase_from(root, Some(uri));
}

fn rebase_from(root: &mut Value, retrieved: Option<&str>) {
    let doc_base = retrieved.unwrap_or(DOCUMENT).to_owned();
    let mut found = Collect::default();
    // A schema file is no OpenAPI document: its root is the schema.
    if retrieved.is_none() || root.get("openapi").is_some() || root.get("swagger").is_some() {
        walk::walk(root, &mut found);
    } else {
        walk::schema(root, "", &mut found);
    }
    if retrieved.is_none() && found.schemas.values().all(|d| d.id.is_none()) {
        return;
    }
    // The base of each schema: its `$id` against the base of the nearest schema above it. Shorter pointers first.
    let mut bases: BTreeMap<&str, String> = BTreeMap::new();
    let mut ptrs: Vec<&String> = found.schemas.keys().collect();
    ptrs.sort_by_key(|p| p.len());
    for ptr in ptrs {
        let parent = bases
            .iter()
            .filter(|(p, _)| ptr.len() > p.len() && ptr.starts_with(*p) && ptr.as_bytes().get(p.len()) == Some(&b'/'))
            .max_by_key(|(p, _)| p.len())
            .map_or(doc_base.as_str(), |(_, b)| b.as_str())
            .to_owned();
        let base = match &found.schemas[ptr].id {
            Some(id) => strip_fragment(&resolve(&parent, id)).to_owned(),
            None => parent,
        };
        bases.insert(ptr.as_str(), base);
    }
    let mut resources: BTreeMap<String, String> = BTreeMap::new();
    if retrieved.is_some() {
        resources.insert(doc_base.clone(), String::new());
    }
    let mut anchors: BTreeMap<(String, String), String> = BTreeMap::new();
    for (ptr, d) in &found.schemas {
        let base = bases[ptr.as_str()].clone();
        if d.id.is_some() {
            resources.entry(base.clone()).or_insert_with(|| ptr.clone());
        }
        for a in &d.anchors {
            anchors.entry((base.clone(), a.clone())).or_insert_with(|| ptr.clone());
        }
    }
    let index = Index { bases: &bases, resources: &resources, anchors: &anchors };
    dynamic_refs(root, &found, &index);
    for (ptr, d) in &found.schemas {
        let Some(r) = &d.reference else { continue };
        let base = &bases[ptr.as_str()];
        if base == &doc_base && retrieved.is_none() {
            continue;
        }
        let absolute = resolve(base, r);
        let (uri, fragment) = absolute.split_once('#').unwrap_or((absolute.as_str(), ""));
        let local = resources.get(uri).and_then(|at| match fragment {
            "" => Some(at.clone()),
            f if f.starts_with('/') => Some(format!("{at}{}", percent_decode(f))),
            name => anchors.get(&(uri.to_owned(), name.to_owned())).cloned(),
        });
        let rewritten = match local {
            Some(at) => format!("#{at}"),
            // No `$id` above it: the bundler resolves it against the document as written.
            None if base == &doc_base => continue,
            None if uri == doc_base => format!("#{fragment}"),
            None => absolute.clone(),
        };
        if let Some(Value::Object(o)) = root.pointer_mut(ptr) {
            o.insert("$ref".to_owned(), Value::String(rewritten));
        }
    }
}

/// Returns the pointer of the schema in `root` (a schema document) that declares `$anchor` (or `$dynamicAnchor`)
/// `name`.
pub(crate) fn anchor(root: &Value, name: &str) -> Option<String> {
    let mut found = Collect::default();
    walk::schema(&mut root.clone(), "", &mut found);
    found.schemas.into_iter().find(|(_, d)| d.anchors.iter().any(|a| a == name)).map(|(p, _)| p)
}

/// Where each schema's base and the document's resources and anchors are.
struct Index<'a> {
    bases: &'a BTreeMap<&'a str, String>,
    resources: &'a BTreeMap<String, String>,
    anchors: &'a BTreeMap<(String, String), String>,
}

/// Resolves every `$dynamicRef` of `root` statically against its base.
fn dynamic_refs(root: &mut Value, found: &Collect, index: &Index<'_>) {
    for (ptr, d) in &found.schemas {
        let Some(r) = &d.dynamic_reference else { continue };
        let absolute = resolve(&index.bases[ptr.as_str()], r);
        let (uri, fragment) = absolute.split_once('#').unwrap_or((absolute.as_str(), ""));
        let target = index.resources.get(uri).and_then(|at| match fragment {
            "" => Some(at.clone()),
            f if f.starts_with('/') => Some(format!("{at}{}", percent_decode(f))),
            name => index.anchors.get(&(uri.to_owned(), name.to_owned())).cloned(),
        });
        let Some(target) = target else { continue };
        let bookended = !fragment.is_empty()
            && !fragment.starts_with('/')
            && found.schemas.get(&target).and_then(|t| t.dynamic_anchor.as_deref()) == Some(fragment);
        // Bookended: it stays dynamic, written as a pointer to its static target; the target's `$dynamicAnchor` names
        // the anchor. Without the bookend it is a `$ref` (2020-12 core section 8.2.3.2).
        if let Some(Value::Object(o)) = root.pointer_mut(ptr) {
            let to = Value::String(format!("#{target}"));
            if bookended {
                o.insert("$dynamicRef".to_owned(), to);
            } else {
                o.remove("$dynamicRef");
                if o.contains_key("$ref") {
                    let member = serde_json::json!({ "$ref": to });
                    match o.get_mut("allOf") {
                        Some(Value::Array(members)) => members.push(member),
                        _ => {
                            o.insert("allOf".to_owned(), Value::Array(vec![member]));
                        }
                    }
                } else {
                    o.insert("$ref".to_owned(), to);
                }
            }
        }
    }
}

/// Returns `uri` without its fragment.
fn strip_fragment(uri: &str) -> &str {
    uri.split_once('#').map_or(uri, |(u, _)| u)
}

/// Returns the scheme of `r`, when it has one (`http`, `urn`): letters, digits, `+`, `-` or `.` before a `:`.
pub(crate) fn scheme(r: &str) -> Option<&str> {
    let (s, _) = r.split_once(':')?;
    let mut chars = s.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic() && chars.all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))).then_some(s)
}

/// Returns `r` resolved against `base` (RFC 3986 section 5.2, without query handling).
pub(crate) fn resolve(base: &str, r: &str) -> String {
    if scheme(r).is_some() {
        return r.to_owned();
    }
    let base = strip_fragment(base);
    if r.is_empty() {
        return base.to_owned();
    }
    if r.starts_with('#') {
        return format!("{base}{r}");
    }
    let Some(s) = scheme(base) else { return r.to_owned() };
    let rest = base.split_once(':').map_or("", |(_, rest)| rest);
    // A base without a hierarchy (`urn:uuid:...`) takes only fragments.
    let Some(after) = rest.strip_prefix("//") else {
        return if rest.starts_with('/') { format!("{s}:{}", remove_dots(&merge("", rest, r))) } else { r.to_owned() };
    };
    if let Some(net) = r.strip_prefix("//") {
        return format!("{s}://{net}");
    }
    let (authority, path) = after.find('/').map_or((after, ""), |i| after.split_at(i));
    let (r_path, r_fragment) = r.split_once('#').map_or((r, None), |(p, f)| (p, Some(f)));
    let joined =
        if r_path.starts_with('/') { remove_dots(r_path) } else { remove_dots(&merge(authority, path, r_path)) };
    let fragment = r_fragment.map_or_else(String::new, |f| format!("#{f}"));
    format!("{s}://{authority}{joined}{fragment}")
}

/// Returns RFC 3986 section 5.2.3: the base path up to its last `/`, then `r`.
fn merge(authority: &str, path: &str, r: &str) -> String {
    if !authority.is_empty() && path.is_empty() {
        return format!("/{r}");
    }
    match path.rfind('/') {
        Some(i) => format!("{}{r}", path.split_at(i + 1).0),
        None => r.to_owned(),
    }
}

/// Returns RFC 3986 section 5.2.4: `.` and `..` segments removed.
fn remove_dots(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    let segments: Vec<&str> = path.split('/').collect();
    for (i, seg) in segments.iter().enumerate() {
        match *seg {
            "." => {}
            ".." => {
                if out.len() > 1 {
                    out.pop();
                }
            }
            s => out.push(s),
        }
        // A trailing `.` or `..` leaves a directory.
        if i == segments.len() - 1 && (*seg == "." || *seg == "..") {
            out.push("");
        }
    }
    out.join("/")
}

/// The length of a `%XX` escape.
const ESCAPE: usize = 3;

/// The radix of the digits of an escape.
const HEX: u32 = 16;

/// Returns `%XX` escapes of a URI fragment decoded: a JSON pointer in a fragment is percent-encoded.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + ESCAPE)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, HEX).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += ESCAPE;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn references_resolve_like_rfc_3986() {
        let base = "http://a/b/c/d";
        for (r, want) in [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("../g", "http://a/b/g"),
            ("/g", "http://a/g"),
            ("#s", "http://a/b/c/d#s"),
            ("g#s", "http://a/b/c/g#s"),
            ("//h/x", "http://h/x"),
            ("urn:x", "urn:x"),
        ] {
            assert_eq!(resolve(base, r), want, "{r}");
        }
        assert_eq!(resolve("urn:uuid:1234", "#/$defs/a"), "urn:uuid:1234#/$defs/a");
    }

    /// RFC 3986 section 5.4, without the examples that have a query: `resolve` does not handle queries.
    #[test]
    fn the_examples_of_rfc_3986_resolve() {
        let base = "http://a/b/c/d;p";
        for (r, want) in [
            ("g:h", "g:h"),
            ("g/", "http://a/b/c/g/"),
            ("//g", "http://g"),
            (";x", "http://a/b/c/;x"),
            ("g;x", "http://a/b/c/g;x"),
            ("", "http://a/b/c/d;p"),
            (".", "http://a/b/c/"),
            ("./", "http://a/b/c/"),
            ("..", "http://a/b/"),
            ("../", "http://a/b/"),
            ("../..", "http://a/"),
            ("../../", "http://a/"),
            ("../../g", "http://a/g"),
            // Section 5.4.2: abnormal examples.
            ("../../../g", "http://a/g"),
            ("../../../../g", "http://a/g"),
            ("/./g", "http://a/g"),
            ("/../g", "http://a/g"),
            ("g.", "http://a/b/c/g."),
            (".g", "http://a/b/c/.g"),
            ("g..", "http://a/b/c/g.."),
            ("..g", "http://a/b/c/..g"),
            ("./../g", "http://a/b/g"),
            ("./g/.", "http://a/b/c/g/"),
            ("g/./h", "http://a/b/c/g/h"),
            ("g/../h", "http://a/b/c/h"),
            ("g;x=1/./y", "http://a/b/c/g;x=1/y"),
            ("g;x=1/../y", "http://a/b/c/y"),
        ] {
            assert_eq!(resolve(base, r), want, "{r}");
        }
    }

    #[test]
    fn escapes_in_a_fragment_are_decoded() {
        assert_eq!(percent_decode("%2Fa%7e%25"), "/a~%");
        assert_eq!(percent_decode("a%2"), "a%2", "too short");
        assert_eq!(percent_decode("%zz%"), "%zz%", "not hex");
    }

    #[test]
    fn an_anchor_names_the_schema_that_declares_it() {
        let doc = json!({"$defs": {"a": {"type": "string"}, "b": {"$anchor": "here", "type": "integer"}}});
        assert_eq!(anchor(&doc, "here").as_deref(), Some("/$defs/b"));
        assert_eq!(anchor(&doc, "elsewhere"), None);
    }

    #[test]
    fn a_ref_inside_an_id_resolves_against_that_id() {
        let mut doc = json!({"openapi": "3.2.0", "components": {"schemas": {
            "Tree": {"$id": "http://example.com/tree", "$ref": "#/$defs/node",
                     "$defs": {"node": {"properties": {"next": {"$ref": "list"}}}}},
            "List": {"$id": "http://example.com/list", "type": "array", "items": {"$ref": "#anchored"},
                     "$defs": {"a": {"$anchor": "anchored", "type": "string"}}},
            "Plain": {"$ref": "#/components/schemas/Tree"}
        }}});
        rebase(&mut doc);
        let s = &doc["components"]["schemas"];
        assert_eq!(s["Tree"]["$ref"], "#/components/schemas/Tree/$defs/node");
        assert_eq!(s["Tree"]["$defs"]["node"]["properties"]["next"]["$ref"], "#/components/schemas/List");
        assert_eq!(s["List"]["items"]["$ref"], "#/components/schemas/List/$defs/a");
        assert_eq!(s["Plain"]["$ref"], "#/components/schemas/Tree", "no `$id` above it: as written");
    }
}
