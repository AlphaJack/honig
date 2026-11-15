//! Schema Object: every JSON Schema 2020-12 keyword plus the `OpenAPI` 3.2
//! dialect keywords (`discriminator`, `xml`, `externalDocs`, `example`).
//! Unknown keywords are kept in `extra`.

use std::fmt;

use serde::de::value::MapAccessDeserializer;
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Number, Value};

use super::{ExternalDocs, Extra, Map, present};

/// A schema is either a boolean (`true` accepts all, `false` accepts none) or
/// an object.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Schema {
    Bool(bool),
    Object(Box<SchemaObject<Self>>),
}

impl Default for Schema {
    fn default() -> Self {
        Self::Bool(true)
    }
}

impl Schema {
    #[cfg(test)]
    pub(crate) fn as_object(&self) -> Option<&SchemaObject<Self>> {
        match self {
            Self::Object(o) => Some(o),
            Self::Bool(_) => None,
        }
    }
}

impl From<SchemaObject<Self>> for Schema {
    fn from(o: SchemaObject<Self>) -> Self {
        Self::Object(Box::new(o))
    }
}

impl Serialize for Schema {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Bool(b) => s.serialize_bool(*b),
            Self::Object(o) => o.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Schema {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Schema;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a schema (boolean or object)")
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<Schema, E> {
                Ok(Schema::Bool(b))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Schema, A::Error> {
                // No buffering: errors inside keep their exact path.
                SchemaObject::<Schema>::deserialize(MapAccessDeserializer::new(map))
                    .map(|o| Schema::Object(Box::new(o)))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SimpleType {
    Null,
    Boolean,
    Object,
    Array,
    Number,
    Integer,
    String,
}

/// `type` is either one name or a list of names. The original shape is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum TypeSet {
    Single(SimpleType),
    Multiple(Vec<SimpleType>),
}

/// The keywords of a schema object. `S` is the type of its subschemas: [`Schema`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SchemaObject<S> {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(rename = "$id", default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "$ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(rename = "$anchor", default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(rename = "$dynamicRef", default, skip_serializing_if = "Option::is_none")]
    pub dynamic_ref: Option<String>,
    #[serde(rename = "$dynamicAnchor", default, skip_serializing_if = "Option::is_none")]
    pub dynamic_anchor: Option<String>,
    #[serde(rename = "$vocabulary", default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<Map<bool>>,
    #[serde(rename = "$comment", default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(rename = "$defs", default, skip_serializing_if = "Option::is_none")]
    pub defs: Option<Map<S>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_of: Option<Vec<S>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<S>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub one_of: Option<Vec<S>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not: Option<S>,
    #[serde(rename = "if", default, skip_serializing_if = "Option::is_none")]
    pub if_: Option<S>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub then: Option<S>,
    #[serde(rename = "else", default, skip_serializing_if = "Option::is_none")]
    pub else_: Option<S>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependent_schemas: Option<Map<S>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_items: Option<Vec<S>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub items: Option<S>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contains: Option<S>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<Map<S>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern_properties: Option<Map<S>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<S>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub property_names: Option<S>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unevaluated_items: Option<S>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unevaluated_properties: Option<S>,

    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_: Option<TypeSet>,
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub enum_: Option<Vec<Value>>,
    #[serde(rename = "const", default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub const_: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiple_of: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_items: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_contains: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_contains: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_properties: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_properties: Option<Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependent_required: Option<Map<Vec<String>>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_schema: Option<S>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<Value>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discriminator: Option<Discriminator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xml: Option<Xml>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocs>,
    /// Deprecated in favour of `examples` but still allowed.
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub example: Option<Value>,

    /// Unknown keywords and `x-*` extensions.
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Discriminator {
    pub property_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping: Option<Map<String>>,
    /// 3.2: schema used when the discriminating value is absent or unmapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_mapping: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Xml {
    /// 3.2: replaces `attribute` and `wrapped`. Kept as a string so unknown
    /// values survive. The validator checks it against the 3.2 value set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// Deprecated in 3.2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute: Option<bool>,
    /// Deprecated in 3.2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrapped: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}
