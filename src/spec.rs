//! Typed and lossless model of an `OpenAPI` 3.2 document.
//!
//! Lossless means that `Value` to `OpenApi` and back to `Value` yields an equal JSON value. Every
//! object keeps keys that it does not model in `extra` (both extensions and unknown
//! fields). A present `null` is kept apart from an absent key where
//! the spec allows arbitrary values (`example`, `default`, `const`, ...).

mod openapi;
mod schema;

pub(crate) use openapi::*;
pub(crate) use schema::*;

use std::fmt;
use std::marker::PhantomData;

use indexmap::IndexMap;
use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::diag::Diagnostic;
use crate::json::escape_token;

/// Ordered JSON object for unmodelled keys.
pub(crate) type Extra = serde_json::Map<String, Value>;

/// Ordered map used for every typed `OpenAPI` map.
pub(crate) type Map<V> = IndexMap<String, V>;

/// Parses a 3.2 document from a JSON value. Errors carry a JSON pointer.
pub(crate) fn from_value(value: Value) -> Result<OpenApi, Diagnostic> {
    from_value_as(value)
}

/// Parses any model type from a JSON value with pointer-precise errors.
pub(crate) fn from_value_as<T: DeserializeOwned>(value: Value) -> Result<T, Diagnostic> {
    serde_path_to_error::deserialize(value).map_err(|err| {
        let outer = path_to_pointer(err.path());
        let (inner, message) = split_nested(&err.inner().to_string());
        Diagnostic::error(format!("{outer}{inner}"), message)
    })
}

/// Serializes a model value back to JSON. Model types only contain string-keyed maps. Those always serialize.
#[cfg(test)]
pub(crate) fn to_value<T: Serialize>(model: &T) -> Value {
    serde_json::to_value(model).expect("model serialization is infallible")
}

fn path_to_pointer(path: &serde_path_to_error::Path) -> String {
    use serde_path_to_error::Segment;
    let mut out = String::new();
    for seg in path {
        match seg {
            Segment::Seq { index } => out.push_str(&format!("/{index}")),
            Segment::Map { key } => out.push_str(&format!("/{}", escape_token(key))),
            Segment::Enum { .. } | Segment::Unknown => {}
        }
    }
    out
}

// Errors raised inside a buffered `RefOr` lose the outer path tracker. They
// carry their own inner pointer between two SOH markers instead.
const MARK: char = '\u{1}';

fn nested_error<E: de::Error>(inner_pointer: &str, message: &str) -> E {
    E::custom(format!("{MARK}{inner_pointer}{MARK}{message}"))
}

fn split_nested(message: &str) -> (String, String) {
    let mut pointer = String::new();
    let mut rest = message;
    while let Some(after) = rest.strip_prefix(MARK) {
        match after.split_once(MARK) {
            Some((p, tail)) => {
                pointer.push_str(p);
                rest = tail;
            }
            None => break,
        }
    }
    (pointer, rest.to_owned())
}

/// A `deserialize_with` helper: a present key becomes `Some` even when it is `null`.
pub(crate) fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// External Documentation Object (shared by the document and its schemas).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct ExternalDocs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// A description of the target documentation (`CommonMark`).
    pub description: Option<String>,
    /// The URL of the target documentation.
    pub url: String,
    #[serde(flatten)]
    /// Extensions and unknown fields.
    pub extra: Extra,
}

/// Reference Object. `summary` and `description` override the target's.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[expect(clippy::struct_field_names, reason = "fields are named as the OpenAPI fields they store")]
pub(crate) struct Reference {
    #[serde(rename = "$ref")]
    /// `$ref`: URI of the target.
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Overrides the target's summary.
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Overrides the target's description.
    pub description: Option<String>,
    /// Other siblings are ignored by the spec but kept for losslessness.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Either a Reference Object or an inline `T`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefOr<T> {
    /// A Reference Object.
    Ref(Reference),
    /// An inline object.
    Item(T),
}

impl<T> RefOr<T> {
    /// The inline object if this is not a reference.
    #[cfg(test)]
    pub(crate) fn as_item(&self) -> Option<&T> {
        match self {
            Self::Item(t) => Some(t),
            Self::Ref(_) => None,
        }
    }

    /// Returns the reference if this is one.
    pub(crate) fn as_ref(&self) -> Option<&Reference> {
        match self {
            Self::Ref(r) => Some(r),
            Self::Item(_) => None,
        }
    }
}

impl<T: Serialize> Serialize for RefOr<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Ref(r) => r.serialize(s),
            Self::Item(t) => t.serialize(s),
        }
    }
}

impl<'de, T: DeserializeOwned> Deserialize<'de> for RefOr<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(d)?;
        let is_ref = value.as_object().is_some_and(|o| o.get("$ref").is_some_and(Value::is_string));
        let parsed = if is_ref {
            serde_path_to_error::deserialize::<_, Reference>(value).map(RefOr::Ref)
        } else {
            serde_path_to_error::deserialize::<_, T>(value).map(RefOr::Item)
        };
        parsed.map_err(|err| {
            let inner = path_to_pointer(err.path());
            let (nested, message) = split_nested(&err.inner().to_string());
            nested_error(&format!("{inner}{nested}"), &message)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A map whose `x-*` keys are extensions: Paths, Responses and Callbacks.
pub(crate) struct Entries<T> {
    /// Non-extension entries in document order.
    pub entries: Map<T>,
    /// `x-*` entries.
    pub extensions: Extra,
}

impl<T> Default for Entries<T> {
    fn default() -> Self {
        Self { entries: Map::new(), extensions: Extra::new() }
    }
}

impl<T> Entries<T> {
    /// Entries in document order (extensions excluded).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &T)> {
        self.entries.iter()
    }

    /// The entry for `key`.
    #[cfg(test)]
    pub(crate) fn get(&self, key: &str) -> Option<&T> {
        self.entries.get(key)
    }
}

impl<T: Serialize> Serialize for Entries<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(None)?;
        for (k, v) in &self.entries {
            map.serialize_entry(k, v)?;
        }
        for (k, v) in &self.extensions {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Entries<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Entries<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Entries::default();
                while let Some(key) = map.next_key::<String>()? {
                    if key.starts_with("x-") {
                        let v: Value = map.next_value()?;
                        out.extensions.insert(key, v);
                    } else {
                        let v: T = map.next_value()?;
                        out.entries.insert(key, v);
                    }
                }
                Ok(out)
            }
        }
        d.deserialize_map(V(PhantomData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ref_or_picks_reference() {
        let r: RefOr<Response> = from_value_as(json!({"$ref": "#/components/responses/A"})).unwrap();
        assert_eq!(r.as_ref().unwrap().reference, "#/components/responses/A");
    }

    #[test]
    fn nested_errors_have_full_pointer() {
        let doc = json!({
            "openapi": "3.2.0",
            "info": {"title": "t", "version": "1"},
            "paths": {"/orders": {"get": {"responses": {"200": {"description": 5}}}}}
        });
        let err = from_value(doc).unwrap_err();
        assert_eq!(err.pointer, "/paths/~1orders/get/responses/200/description");
    }

    #[test]
    fn entries_split_extensions() {
        let e: Entries<Value> = from_value_as(json!({"/a": 1, "x-b": 2})).unwrap();
        assert_eq!(e.entries.len(), 1);
        assert_eq!(e.extensions.len(), 1);
        assert_eq!(to_value(&e), json!({"/a": 1, "x-b": 2}));
    }
}
