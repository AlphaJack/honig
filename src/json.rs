//! JSON documents: the lookups that every pass shares, pointers (RFC 6901) and the index of `$anchor`s and `$id`s
//! that resolves local refs. It is a leaf and uses no other module of the generator.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

/// A JSON object held as a value or as its map.
pub(crate) trait Fields {
    /// Returns the member named `key`.
    fn field(&self, key: &str) -> Option<&Value>;
}

impl Fields for Value {
    fn field(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }
}

impl Fields for Map<String, Value> {
    fn field(&self, key: &str) -> Option<&Value> {
        self.get(key)
    }
}

/// Returns the items of an array with their index. None when `v` is not an array.
pub(crate) fn items(v: Option<&Value>) -> impl Iterator<Item = (usize, &Value)> {
    v.and_then(Value::as_array).into_iter().flatten().enumerate()
}

/// Returns the members of the object at `key`. None when it is not an object.
pub(crate) fn members<'a>(v: &'a (impl Fields + ?Sized), key: &str) -> impl Iterator<Item = (&'a String, &'a Value)> {
    object(v, key).into_iter().flatten()
}

/// Returns the object at `key`.
pub(crate) fn object<'a>(v: &'a (impl Fields + ?Sized), key: &str) -> Option<&'a Map<String, Value>> {
    v.field(key).and_then(Value::as_object)
}

/// Returns the string at `key`.
pub(crate) fn string(v: &(impl Fields + ?Sized), key: &str) -> Option<String> {
    v.field(key).and_then(Value::as_str).map(String::from)
}

/// Returns the boolean at `key` or `false` when absent.
pub(crate) fn flag(v: &(impl Fields + ?Sized), key: &str) -> bool {
    v.field(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Returns the `schema` of a parameter, header or media type. Any value when absent.
pub(crate) fn schema(v: &(impl Fields + ?Sized)) -> Value {
    v.field("schema").cloned().unwrap_or(Value::Bool(true))
}

/// Returns the strings among `values`.
pub(crate) fn strs(values: &[Value]) -> impl Iterator<Item = &str> {
    values.iter().filter_map(Value::as_str)
}

/// Returns the owned strings among `values`.
pub(crate) fn strings(values: &[Value]) -> Vec<String> {
    strs(values).map(String::from).collect()
}

/// Returns the owned string that `v` holds.
pub(crate) fn owned_str(v: &Value) -> Option<String> {
    v.as_str().map(String::from)
}

/// Returns whether `v` is a string, number or bool.
pub(crate) fn is_scalar(v: &Value) -> bool {
    v.is_string() || v.is_number() || v.is_boolean()
}

/// Escapes one reference token per RFC 6901.
pub(crate) fn escape_token(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// Unescapes one reference token per RFC 6901.
pub(crate) fn unescape_token(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// Appends a token to a pointer.
pub(crate) fn join(pointer: &str, token: &str) -> String {
    format!("{pointer}/{}", escape_token(token))
}

/// Does `key` (in an object found under `parent`) hold literal data?
///
/// Examples, enum values and schema defaults are data. `default` under
/// `responses` is the default response. That is structure. Under a map of
/// names (`properties`, `definitions`, ...) every key is a name: a property
/// called `default` holds a schema.
pub(crate) fn is_data(parent: &str, key: &str) -> bool {
    if matches!(parent, "properties" | "patternProperties" | "definitions" | "$defs" | "dependentSchemas" | "schemas") {
        return false;
    }
    match key {
        "example" | "examples" | "enum" | "const" => true,
        "default" => parent != "responses",
        _ => false,
    }
}

/// Resolves a JSON pointer (without the leading `#`) inside a value.
pub(crate) fn pointer<'a>(root: &'a Value, ptr: &str) -> Option<&'a Value> {
    if ptr.is_empty() {
        return Some(root);
    }
    let ptr = ptr.strip_prefix('/')?;
    let mut cur = root;
    for raw in ptr.split('/') {
        let token = unescape_token(&percent_decode(raw));
        cur = match cur {
            Value::Object(m) => m.get(&token)?,
            Value::Array(a) => a.get(array_index(&token)?)?,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
        };
    }
    Some(cur)
}

/// Returns an RFC 6901 array index: `0` or digits without a leading zero. `parse` alone would take `+1` and `01`.
fn array_index(token: &str) -> Option<usize> {
    let digits = !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit());
    (digits && (token == "0" || !token.starts_with('0'))).then(|| token.parse().ok()).flatten()
}

/// Percent-decodes a URI fragment (`#/paths/~1a%7Bb%7D`).
/// Decode `%XX` escapes (URI fragments) as the runtime decodes paths and queries.
pub(crate) fn percent_decode(s: &str) -> String {
    runtime::__private::pct_decode(s)
}

/// A bundled document with an index of anchors and `$id`s.
#[derive(Debug)]
pub(crate) struct Doc {
    /// The whole document.
    pub root: Value,
    /// `$anchor` / `$dynamicAnchor` name to pointer of the schema declaring it.
    anchors: BTreeMap<String, String>,
    /// Absolute `$id` to pointer.
    ids: BTreeMap<String, String>,
    self_uri: Option<String>,
    /// The text of each `externalValue` read at load time or why it could not be read.
    pub externals: Externals,
}

/// Each `externalValue` URI with its text or why it could not be read.
pub(crate) type Externals = BTreeMap<String, Result<String, String>>;

impl Doc {
    /// Indexes a bundled document's anchors and `$id`s.
    pub(crate) fn new(root: Value) -> Self {
        let mut anchors = BTreeMap::new();
        let mut ids = BTreeMap::new();
        index(&root, "", "", &mut anchors, &mut ids);
        let self_uri = root.get("$self").and_then(Value::as_str).map(String::from);
        Self { root, anchors, ids, self_uri, externals: Externals::new() }
    }

    /// Turns a `$ref` string into a local JSON pointer if it points into this document.
    pub(crate) fn local_pointer(&self, reference: &str) -> Option<String> {
        let (base, frag) = match reference.split_once('#') {
            Some((b, f)) => (b, f),
            None => (reference, ""),
        };
        let prefix = if base.is_empty() || self.self_uri.as_deref().is_some_and(|s| s.trim_end_matches('#') == base) {
            String::new()
        } else {
            let p = self.ids.get(base)?;
            p.clone()
        };
        if frag.is_empty() || frag.starts_with('/') {
            Some(format!("{prefix}{frag}"))
        } else {
            self.anchors.get(&percent_decode(frag)).cloned()
        }
    }

    /// Returns the value that a `$ref` points at if it is in this document.
    pub(crate) fn get(&self, reference: &str) -> Option<&Value> {
        pointer(&self.root, &self.local_pointer(reference)?)
    }
}

/// Records `$anchor` / `$dynamicAnchor` and `$id` of every object outside data
/// keywords (examples, enums, defaults) with its pointer.
fn index(
    v: &Value,
    ptr: &str,
    parent: &str,
    anchors: &mut BTreeMap<String, String>,
    ids: &mut BTreeMap<String, String>,
) {
    match v {
        Value::Object(o) => {
            for key in ["$anchor", "$dynamicAnchor"] {
                if let Some(a) = o.get(key).and_then(Value::as_str) {
                    anchors.entry(a.to_owned()).or_insert_with(|| ptr.to_owned());
                }
            }
            if let Some(id) = o.get("$id").and_then(Value::as_str) {
                ids.entry(id.trim_end_matches('#').to_owned()).or_insert_with(|| ptr.to_owned());
            }
            for (k, c) in o {
                if !is_data(parent, k) {
                    index(c, &join(ptr, k), k, anchors, ids);
                }
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter().enumerate() {
                index(c, &format!("{ptr}/{i}"), parent, anchors, ids);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Returns a keyword's non-negative integer. `2.0` counts as `2` (2020-12: a number with a zero fraction is an
/// integer).
pub(crate) fn count(v: &Value) -> Option<u64> {
    /// 2^53: every whole float below it converts exactly.
    const MAX_EXACT: f64 = 9_007_199_254_740_992.0;
    v.as_u64().or_else(|| {
        let f = v.as_f64()?;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "whole, non-negative and below 2^53"
        )]
        (f >= 0.0 && f.fract() == 0.0 && f < MAX_EXACT).then_some(f as u64)
    })
}

/// Does schema `s` allow exactly one value: a `const` or a one-value `enum`? A discriminated branch that fixes its
/// tag this way cannot match another branch's tag.
pub(crate) fn fixes_one_value(s: &Value) -> bool {
    s.get("const").is_some() || s.get("enum").and_then(Value::as_array).is_some_and(|e| e.len() == 1)
}

/// Returns a JSON number as the digits of its decimal text and the power of ten they scale by: `2.5` is `(25, -1)`
/// and `1e3` is `(1, 3)`. `None` for a negative number or more digits than a `u128` holds.
pub(crate) fn decimal_parts(n: &serde_json::Number) -> Option<(u128, i32)> {
    let text = n.to_string();
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((&text, "0"));
    let exponent: i32 = exponent.parse().ok()?;
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.starts_with('-') {
        return None;
    }
    let digits: u128 = format!("{whole}{fraction}").parse().ok()?;
    Some((digits, exponent.checked_sub(i32::try_from(fraction.len()).ok()?)?))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pointer_escaping_roundtrips() {
        assert_eq!(join("", "/orders/{id}"), "/~1orders~1{id}");
        assert_eq!(join("/a", "x~y"), "/a/x~0y");
        assert_eq!(unescape_token("~1orders~0x"), "/orders~x");
    }

    #[test]
    fn pointers_and_anchors() {
        let doc = Doc::new(json!({
            "openapi": "3.2.0",
            "$self": "https://x.com/api.yaml",
            "paths": {"/a{b}": {"get": {"responses": {}}}},
            "components": {"schemas": {
                "A": {"$anchor": "aa", "type": "string"},
                "B": {"$id": "https://x.com/b", "$defs": {"C": {"type": "integer"}}}
            }}
        }));
        assert_eq!(doc.get("#/components/schemas/A").unwrap()["type"], "string");
        assert_eq!(doc.get("#aa").unwrap()["type"], "string");
        assert_eq!(doc.get("https://x.com/api.yaml#/components/schemas/A").unwrap()["type"], "string");
        assert_eq!(doc.get("https://x.com/b#/$defs/C").unwrap()["type"], "integer");
        assert!(doc.get("#/paths/~1a%7Bb%7D/get").is_some());
        assert!(doc.get("#/nope").is_none());
    }

    /// RFC 6901 section 4: an array index is `0` or digits without a leading zero.
    #[test]
    fn an_array_index_is_only_its_plain_digits() {
        let doc = json!({"a": [10, 11]});
        assert_eq!(pointer(&doc, "/a/0"), Some(&json!(10)));
        assert_eq!(pointer(&doc, "/a/1"), Some(&json!(11)));
        for index in ["01", "+1", "00", "-0", " 1", "1 "] {
            assert_eq!(pointer(&doc, &format!("/a/{index}")), None, "{index}");
        }
    }

    #[test]
    fn anchors_in_examples_are_not_indexed() {
        let doc =
            Doc::new(json!({"components": {"schemas": {"A": {"type": "object", "examples": [{"$anchor": "x"}]}}}}));
        assert!(doc.get("#x").is_none());
    }
}
