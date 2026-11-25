//! [`Coerce`]: a deserializer of JSON values whose scalars may be wire text.

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::binary::{BINARY, raw_part};
use crate::raw_value::RAW_VALUE;

/// Deserializes a JSON value whose scalars may be strings (paths, queries, headers, forms).
#[derive(Debug)]
pub struct Coerce(pub Value);

#[derive(Debug)]
/// Error of the [`Coerce`] deserializer.
pub struct CoerceError(pub(crate) String);

impl fmt::Display for CoerceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CoerceError {}
impl de::Error for CoerceError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

impl From<serde_json::Error> for CoerceError {
    fn from(e: serde_json::Error) -> Self {
        Self(e.to_string())
    }
}

/// Each number that text is parsed as: the method of the deserializer, the method of the visitor, the type, the
/// `format` of the schema and what a value must be. [`Coerce`] and the XML stream parse text through it.
macro_rules! numbers {
    ($each:ident) => {
        $each!(deserialize_i8, visit_i8, i8, "int8", |_| true);
        $each!(deserialize_i16, visit_i16, i16, "int16", |_| true);
        $each!(deserialize_i32, visit_i32, i32, "int32", |_| true);
        $each!(deserialize_i64, visit_i64, i64, "int64", |_| true);
        $each!(deserialize_u8, visit_u8, u8, "uint8", |_| true);
        $each!(deserialize_u16, visit_u16, u16, "uint16", |_| true);
        $each!(deserialize_u32, visit_u32, u32, "uint32", |_| true);
        $each!(deserialize_u64, visit_u64, u64, "uint64", |_| true);
        $each!(deserialize_f32, visit_f32, f32, "float", |n: &f32| n.is_finite());
        $each!(deserialize_f64, visit_f64, f64, "double", |n: &f64| n.is_finite());
    };
}
pub(crate) use numbers;

/// Returns `text` as a number of `T`. The `OpenAPI` numbers are the numbers of JSON. They follow its grammar (no `+5`,
/// `007`, ` 5`, `NaN` or `inf`). They are finite where `ok` asks and in the range of `T`.
pub(crate) fn number<T: std::str::FromStr>(text: &str, ok: impl FnOnce(&T) -> bool) -> Option<T> {
    crate::decimal::is_json_number(text).then(|| text.parse::<T>().ok()).flatten().filter(ok)
}

/// Returns why `text` is not a number of the `format` of a schema.
pub(crate) fn not_a_number(text: &str, format: &str) -> CoerceError {
    CoerceError(format!("`{text}` is not a number in the range of `{format}`"))
}

/// Returns `text` as a boolean: `true` or `1`, `false` or `0`, with spaces around.
pub(crate) fn boolean(text: &str) -> Option<bool> {
    match text.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// Returns why `text` is not a boolean.
pub(crate) fn not_a_boolean(text: &str) -> CoerceError {
    CoerceError(format!("`{text}` is not a boolean"))
}

/// Text parsed as a number of `$ty` by [`number`].
macro_rules! coerce_num {
    ($method:ident, $visit:ident, $ty:ty, $format:literal, $ok:expr) => {
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
            match self.0 {
                Value::String(s) => match number::<$ty>(&s, $ok) {
                    Some(n) => v.$visit(n),
                    None => Err(not_a_number(&s, $format)),
                },
                other => other.$method(v).map_err(CoerceError::from),
            }
        }
    };
}

impl<'de> Deserializer<'de> for Coerce {
    type Error = CoerceError;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            // Untyped and union targets: wire text in the number grammar of JSON is a number. `42` in a path is the
            // integer 42. `+5` and `007` stay text. An integer past `u64` stays text too because an `f64` would round
            // it.
            Value::String(s) if crate::decimal::is_json_number(&s) => {
                if let Ok(i) = s.parse::<i64>() {
                    v.visit_i64(i)
                } else if let Ok(u) = s.parse::<u64>() {
                    v.visit_u64(u)
                } else if let Some(f) = s.parse::<f64>().ok().filter(|f| f.is_finite() && s.contains('.')) {
                    v.visit_f64(f)
                } else {
                    v.visit_string(s)
                }
            }
            Value::String(s) => {
                if s == "true" || s == "false" {
                    v.visit_bool(s == "true")
                } else {
                    v.visit_string(s)
                }
            }
            Value::Array(a) => v.visit_seq(CoerceSeq(a.into_iter())),
            Value::Object(o) => v.visit_map(CoerceMap(o.into_iter(), None)),
            other @ (Value::Null | Value::Bool(_) | Value::Number(_)) => {
                other.deserialize_any(v).map_err(CoerceError::from)
            }
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Value::String(s) => match boolean(&s) {
                Some(b) => v.visit_bool(b),
                None => Err(not_a_boolean(&s)),
            },
            other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
                other.deserialize_bool(v).map_err(CoerceError::from)
            }
        }
    }

    numbers!(coerce_num);

    fn deserialize_char<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_string(v)
    }

    fn deserialize_str<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_string(v)
    }

    fn deserialize_string<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            // Raw part bytes sent to a text field are its text: bytes that are not UTF-8 are no text.
            Value::String(s) => match raw_part(&s) {
                Some(raw) => match String::from_utf8(raw.bytes.to_vec()) {
                    Ok(text) => v.visit_string(text),
                    Err(_) => Err(CoerceError("a text field is not UTF-8".to_owned())),
                },
                None => v.visit_string(s),
            },
            Value::Number(n) => v.visit_string(n.to_string()),
            Value::Bool(b) => v.visit_string(b.to_string()),
            other @ (Value::Null | Value::Array(_) | Value::Object(_)) => {
                other.deserialize_string(v).map_err(CoerceError::from)
            }
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Value::Null => v.visit_none(),
            other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) | Value::Object(_)) => {
                v.visit_some(Self(other))
            }
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Value::Null => v.visit_unit(),
            Value::String(s) if s.is_empty() || s == "null" => v.visit_unit(),
            other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) | Value::Object(_)) => {
                other.deserialize_unit(v).map_err(CoerceError::from)
            }
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_unit(v)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        // `Binary` decodes the marked string itself (no text conversion).
        match self.0 {
            Value::String(s) if name == BINARY => v.visit_string(s),
            // A `format: decimal` number asks for its text. That is the digits as sent and not an `f64` parsed from
            // them.
            Value::String(s) if name == RAW_VALUE => v.visit_string(s),
            other @ (Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::String(_)
            | Value::Array(_)
            | Value::Object(_)) => v.visit_newtype_struct(Self(other)),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Value::Array(a) => v.visit_seq(CoerceSeq(a.into_iter())),
            Value::Null => v.visit_seq(CoerceSeq(Vec::new().into_iter())),
            Value::String(s) if s.trim_start().starts_with('[') => match serde_json::from_str::<Value>(&s) {
                Ok(parsed @ Value::Array(_)) => Self(parsed).deserialize_seq(v),
                _ => v.visit_seq(CoerceSeq(vec![Value::String(s)].into_iter())),
            },
            // A single repeated-key value arrives as a scalar.
            other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_)) => {
                v.visit_seq(CoerceSeq(vec![other].into_iter()))
            }
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _n: &'static str,
        _len: usize,
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Value::Object(o) => v.visit_map(CoerceMap(o.into_iter(), None)),
            // A multipart part declared XML arrives as raw bytes. When an object is wanted it is parsed as XML.
            Value::String(s) => match raw_part(&s) {
                #[cfg(feature = "xml")]
                Some(raw) if raw.content_type.as_deref().is_some_and(crate::media::is_xml_media) => {
                    match crate::xml_tree::xml_document_value(&raw.bytes) {
                        Ok(parsed @ Value::Object(_)) => Self(parsed).deserialize_map(v),
                        // An empty root (`<address/>`): an object with no properties set.
                        Ok(Value::String(text)) if text.trim().is_empty() => {
                            Self(Value::Object(serde_json::Map::new())).deserialize_map(v)
                        }
                        Ok(_) => {
                            Err(CoerceError("expected an object, found an XML element without children".to_owned()))
                        }
                        Err(e) => Err(CoerceError(format!("invalid XML part: {e}"))),
                    }
                }
                _ => match serde_json::from_str::<Value>(&s) {
                    Ok(parsed @ Value::Object(_)) => Self(parsed).deserialize_map(v),
                    _ => Err(CoerceError(format!("expected an object, found `{s}`"))),
                },
            },
            other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_)) => {
                other.deserialize_map(v).map_err(CoerceError::from)
            }
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _n: &'static str,
        _f: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.deserialize_map(v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.0.deserialize_enum(name, variants, v).map_err(CoerceError::from)
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_string(v)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        v.visit_unit()
    }
}

struct CoerceSeq(std::vec::IntoIter<Value>);

impl<'de> SeqAccess<'de> for CoerceSeq {
    type Error = CoerceError;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, CoerceError> {
        match self.0.next() {
            Some(v) => seed.deserialize(Coerce(v)).map(Some),
            None => Ok(None),
        }
    }
}

struct CoerceMap(serde_json::map::IntoIter, Option<Value>);

impl<'de> MapAccess<'de> for CoerceMap {
    type Error = CoerceError;
    fn next_key_seed<K: de::DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, CoerceError> {
        match self.0.next() {
            Some((k, v)) => {
                self.1 = Some(v);
                seed.deserialize(Coerce(Value::String(k))).map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, CoerceError> {
        seed.deserialize(Coerce(self.1.take().unwrap_or(Value::Null)))
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    fn read<T: Deserialize<'static>>(text: &str) -> Result<T, CoerceError> {
        T::deserialize(Coerce(Value::String(text.to_owned())))
    }

    /// The `OpenAPI` numbers are the numbers of JSON (RFC 8259 section 6). They follow its grammar, are finite and are
    /// in the range of the format.
    #[test]
    fn text_is_a_number_only_by_the_json_grammar_finite_and_in_range() {
        assert_eq!(
            (read::<i32>("-12").ok(), read::<f64>("1.5e3").ok(), read::<f64>("-0").ok()),
            (Some(-12), Some(1500.0), Some(-0.0))
        );
        for bad in ["+5", "007", " 5", "5 ", "NaN", "inf", "-infinity", "1.", ".5", "0x10", ""] {
            assert!(read::<f64>(bad).is_err() && read::<i64>(bad).is_err(), "{bad:?}");
        }
        assert!(read::<f64>("1e400").is_err() && read::<f32>("1e39").is_err(), "past the format's range: infinite");
        assert!(read::<u8>("256").is_err() && read::<i32>("1.5").is_err());
        assert_eq!(read::<f64>("NaN").unwrap_err().0, "`NaN` is not a number in the range of `double`");
    }

    #[test]
    fn untyped_text_is_a_number_only_by_the_json_grammar() {
        assert_eq!(read::<Value>("42").ok(), Some(Value::from(42)));
        for text in ["+5", "007", "NaN", "inf"] {
            assert_eq!(read::<Value>(text).ok(), Some(Value::from(text)), "{text} stays text");
        }
    }

    #[test]
    fn untyped_text_is_a_boolean_and_a_number_only_when_exact() {
        assert_eq!(read::<Value>("true").ok(), Some(Value::Bool(true)));
        assert_eq!(read::<Value>("false").ok(), Some(Value::Bool(false)));
        assert_eq!(read::<Value>("1.5").ok(), Some(Value::from(1.5)));
        // An `f64` would round these: they stay text.
        for text in ["1e5", "18446744073709551616"] {
            assert_eq!(read::<Value>(text).ok(), Some(Value::from(text)), "{text} stays text");
        }
    }

    #[test]
    fn text_is_a_boolean_unit_newtype_or_list() {
        #[derive(Debug, PartialEq, Deserialize)]
        struct Id(i32);
        assert_eq!((read::<bool>(" 0 ").ok(), read::<bool>("1").ok()), (Some(false), Some(true)));
        assert!(read::<bool>("yes").is_err());
        assert!(read::<()>("").is_ok() && read::<()>("null").is_ok() && read::<()>("x").is_err());
        assert_eq!(read::<Id>("5").ok(), Some(Id(5)));
        assert_eq!(read::<Vec<i32>>("[1, 2]").ok(), Some(vec![1, 2]));
        assert_eq!(read::<Vec<String>>("[x").ok(), Some(vec!["[x".to_owned()]));
        assert_eq!(read::<Vec<String>>("a").ok(), Some(vec!["a".to_owned()]));
        assert_eq!(read::<bool>("yes").unwrap_err().to_string(), "`yes` is not a boolean");
    }

    /// A multipart part declared XML is parsed as XML when an object is wanted. A part of another media type is not.
    #[cfg(all(feature = "multipart", feature = "xml"))]
    #[test]
    fn an_xml_part_is_read_as_an_object() {
        use std::collections::BTreeMap;
        type Fields = BTreeMap<String, String>;
        let part = |text: &'static str, media: &str| {
            crate::binary::raw_parts(|| {
                let mark = crate::binary::raw_mark(&bytes::Bytes::from(text), None, Some(media));
                Fields::deserialize(Coerce(Value::String(mark))).ok()
            })
        };
        assert_eq!(part("<a><b>1</b></a>", "application/xml"), Some(Fields::from([("b".into(), "1".into())])));
        assert_eq!(part("<a/>", "application/xml"), Some(Fields::new()));
        assert_eq!(part("<a>x</a>", "application/xml"), None);
        assert_eq!(part("<a><b>1</b></a>", "text/plain"), None, "not declared XML");
    }
}
