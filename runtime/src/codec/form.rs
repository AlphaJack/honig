//! `application/x-www-form-urlencoded` bodies.

use std::borrow::Cow;

use serde::Serialize;
use serde::de::value::StrDeserializer;
use serde::de::{DeserializeOwned, DeserializeSeed, Deserializer, MapAccess, Visitor};
use serde_json::{Map, Value};

use crate::coerce::CoerceError;
use crate::json::push_multi;
use crate::json::to_value;
#[cfg(feature = "xml")]
use crate::media::is_xml_media;
use crate::params::{encode_query_value, key_order, ordered};
use crate::scalar::scalar;
use crate::text::{decode_err, lossy};
use crate::{
    ClientError, coerce::Coerce, params::Enc, params::Shape, params::Style, params::decode_query, params::parse_query,
};

/// Returns the text of a form field in its `encoding.contentType`. That is XML elements or JSON or else
/// [`form_scalar`].
fn form_field_text(v: &Value, media: Option<&str>) -> String {
    match media.map(|m| m.split(';').next().unwrap_or(m).trim().to_ascii_lowercase()) {
        #[cfg(feature = "xml")]
        Some(m) if is_xml_media(&m) => crate::xml_tree::xml_fields(v),
        Some(m) if m.ends_with("/json") || m.ends_with("+json") => v.to_string(),
        _ => form_scalar(v),
    }
}

/// Returns the reverse of [`form_field_text`] on a decoded field (a string or one per repeated key).
fn typed_field(field: &mut Value, media: &str) -> Result<(), String> {
    let m = media.split(';').next().unwrap_or(media).trim().to_ascii_lowercase();
    let read = |s: &str| -> Result<Value, String> {
        match m.as_str() {
            #[cfg(feature = "xml")]
            xml if is_xml_media(xml) => crate::xml_tree::xml_fields_value(s),
            json if json.ends_with("/json") || json.ends_with("+json") => {
                serde_json::from_str(s).map_err(|e| e.to_string())
            }
            _ => Ok(Value::String(s.to_owned())),
        }
    };
    match field {
        Value::String(s) => *field = read(s)?,
        Value::Array(items) => {
            for item in items.iter_mut() {
                if let Value::String(s) = item {
                    *item = read(s)?;
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => {}
    }
    Ok(())
}

fn form_scalar(v: &Value) -> String {
    match v {
        Value::Object(_) | Value::Array(_) => v.to_string(),
        other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)) => scalar(other),
    }
}

/// Returns `application/x-www-form-urlencoded`: arrays repeat the key. Nested objects are JSON.
pub fn form_encode<T: Serialize>(v: &T) -> Result<Vec<u8>, ClientError> {
    form_encode_styled(v, &[])
}

/// Returns [`form_encode`] where fields with a `style` encode like query parameters.
/// Fields follow the schema's property order.
/// Errors: the value does not serialize.
pub fn form_encode_styled<T: Serialize>(v: &T, styles: &[(&str, Enc)]) -> Result<Vec<u8>, ClientError> {
    form_encode_with(v, styles, &[])
}

/// Returns [`form_encode_styled`] that writes fields whose `encoding.contentType` is XML or JSON in that syntax.
///
/// 3.2 "Parsing and Serializing": `extra` as `application/xml` and an `id` string as `application/json`.
/// Errors: the value does not serialize.
pub fn form_encode_with<T: Serialize>(
    v: &T,
    styles: &[(&str, Enc)],
    types: &[(&str, &str)],
) -> Result<Vec<u8>, ClientError> {
    let value = to_value(v)?;
    let mut parts = vec![];
    if let Value::Object(o) = &value {
        for (k, x) in ordered(o, &key_order(v)) {
            if let Some((_, enc)) = styles.iter().find(|(n, _)| n == k) {
                encode_query_value(&mut parts, k, x, *enc, &[]);
                continue;
            }
            let media = types.iter().find(|(n, _)| n == k).map(|(_, m)| *m);
            let pair = |v: &Value| {
                let mut s = form_urlencoded::Serializer::new(String::new());
                s.append_pair(k, &form_field_text(v, media));
                s.finish()
            };
            match x {
                Value::Null => {}
                Value::Array(a) => parts.extend(a.iter().map(pair)),
                other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_)) => {
                    parts.push(pair(other));
                }
            }
        }
    }
    Ok(parts.join("&").into_bytes())
}

/// Decodes a form body. Styled fields (name, style, shape, explode) read like query parameters.
/// Errors: the fields do not fit `T`.
pub fn form_decode_styled<T: DeserializeOwned>(
    status: Option<u16>,
    body: &[u8],
    styles: &[(&'static str, Style, Shape, bool)],
) -> Result<T, ClientError> {
    form_decode_with(status, body, styles, &[])
}

/// Returns [`form_decode_styled`] that reads fields whose `encoding.contentType` is XML or JSON in it.
/// Errors: a field is not in its type or the fields do not fit `T`.
pub fn form_decode_with<T: DeserializeOwned>(
    status: Option<u16>,
    body: &[u8],
    styles: &[(&'static str, Style, Shape, bool)],
    types: &[(&str, &str)],
) -> Result<T, ClientError> {
    let fields = form_fields(status, body, styles, types)?;
    T::deserialize(Coerce(Value::Object(fields))).map_err(|e| {
        // Tracking the pointer allocates per key. So only a body that fails is read again with tracking to say where.
        let (pointer, kind) = form_fields(status, body, styles, types)
            .ok()
            .and_then(|fields| crate::path::deserialize::<_, T>(Coerce(Value::Object(fields))).err())
            .map(|(pointer, kind, _)| (pointer, kind))
            .unwrap_or_default();
        let (pointer, message) = crate::path::reported(pointer, kind, e.to_string());
        ClientError::Decode { status, message, body: bytes::Bytes::copy_from_slice(body), pointer }
    })
}

/// Returns the fields of a form body as an object: styled fields read like query parameters, typed ones in their
/// media type. Errors: a styled field or a typed one does not decode.
fn form_fields(
    status: Option<u16>,
    body: &[u8],
    styles: &[(&'static str, Style, Shape, bool)],
    types: &[(&str, &str)],
) -> Result<Map<String, Value>, ClientError> {
    let text = lossy(body);
    let pairs = parse_query(&text);
    let mut o = Map::new();
    for (name, style, shape, explode) in styles {
        if let Some(v) =
            decode_query(&pairs, name, "", *style, *shape, *explode).map_err(|e| field_err(status, body, name, &e))?
        {
            o.insert((*name).to_owned(), v);
        }
    }
    let styled =
        |k: &str| styles.iter().any(|(n, ..)| k == *n || k.strip_prefix(*n).is_some_and(|r| r.starts_with('[')));
    for (k, v) in pairs {
        if !styled(&k) {
            push_multi(&mut o, k.into_owned(), Value::String(v.into_owned()));
        }
    }
    for (prop, media) in types {
        if let Some(field) = o.get_mut(*prop) {
            typed_field(field, media).map_err(|e| decode_err(status, body, e))?;
        }
    }
    Ok(o)
}

/// Returns the error of the form field `name` that does not decode: it is at `/name` in the body.
fn field_err(status: Option<u16>, body: &[u8], name: &str, e: &crate::Reason) -> ClientError {
    let message = e.violations().first().map_or_else(|| e.to_string(), |v| v.message.clone());
    ClientError::Decode { status, message, body: bytes::Bytes::copy_from_slice(body), pointer: format!("/{name}") }
}

/// Decodes an `application/x-www-form-urlencoded` body.
pub fn form_decode<T: DeserializeOwned>(status: Option<u16>, body: &[u8]) -> Result<T, ClientError> {
    let text = lossy(body);
    T::deserialize(Form(sorted_pairs(&text))).map_err(|e| {
        // Read again with tracking only once it failed, to say where: tracking allocates per key.
        let (pointer, kind) = crate::path::deserialize::<_, T>(Form(sorted_pairs(&text)))
            .err()
            .map(|(pointer, kind, _)| (pointer, kind))
            .unwrap_or_default();
        let (pointer, message) = crate::path::reported(pointer, kind, e.to_string());
        ClientError::Decode { status, message, body: bytes::Bytes::copy_from_slice(body), pointer }
    })
}

/// Returns the fields of a form sorted by name, so a repeated field's values are next to each other. The sort is
/// stable. So a repeated field keeps its values in order.
fn sorted_pairs(text: &str) -> Vec<(Cow<'_, str>, Cow<'_, str>)> {
    let mut pairs = parse_query(text);
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

/// The fields of a form sorted by name and read as their object. A field once is its text. A repeated one is the
/// list of its texts. Each goes through [`Coerce`] as a `Value` would but without the object or its keys.
struct Form<'a>(Vec<(Cow<'a, str>, Cow<'a, str>)>);

impl Form<'_> {
    /// Returns the object of the fields for a type that reads the form as something else than an object.
    fn object(self) -> Coerce {
        let mut o = Map::new();
        for (k, v) in self.0 {
            push_multi(&mut o, k.into_owned(), Value::String(v.into_owned()));
        }
        Coerce(Value::Object(o))
    }
}

/// A method for which a form is no object. The object of its fields says why it does not fit as it did before.
macro_rules! through_object {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
            self.object().$method(v)
        }
    )*};
}

impl<'de> Deserializer<'de> for Form<'_> {
    type Error = CoerceError;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_map(v)
    }

    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        v.visit_map(Fields { pairs: self.0.into_iter().peekable(), value: None })
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.deserialize_map(v)
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        v.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        v.visit_newtype_struct(self)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        v.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        self.object().deserialize_unit_struct(name, v)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, CoerceError> {
        self.object().deserialize_tuple(len, v)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.object().deserialize_tuple_struct(name, len, v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.object().deserialize_enum(name, variants, v)
    }

    through_object!(
        deserialize_bool,
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64,
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64,
        deserialize_f32,
        deserialize_f64,
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_unit,
        deserialize_seq,
        deserialize_identifier
    );
}

/// The fields of a form one name at a time. The run of pairs with that name is its value.
struct Fields<'a> {
    pairs: std::iter::Peekable<std::vec::IntoIter<(Cow<'a, str>, Cow<'a, str>)>>,
    value: Option<Value>,
}

impl<'de> MapAccess<'de> for Fields<'_> {
    type Error = CoerceError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, CoerceError> {
        let Some((name, first)) = self.pairs.next() else { return Ok(None) };
        let mut more = vec![];
        while let Some((_, v)) = self.pairs.next_if(|(k, _)| *k == name) {
            more.push(Value::String(v.into_owned()));
        }
        let first = Value::String(first.into_owned());
        self.value = Some(if more.is_empty() {
            first
        } else {
            more.insert(0, first);
            Value::Array(more)
        });
        seed.deserialize(StrDeserializer::<CoerceError>::new(&name)).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, CoerceError> {
        seed.deserialize(Coerce(self.value.take().unwrap_or(Value::Null)))
    }
}

/// Encodes a whole query string from one value (3.2 `in: querystring`).
pub fn encode_querystring(out: &mut Vec<String>, v: &Value, form: bool) {
    if form {
        // The same bytes as an `application/x-www-form-urlencoded` body. An object
        // property is JSON (the default `contentType` of the Encoding Object). It is
        // never exploded into bare `key=value` pairs that lose the name of the property.
        if v.is_object()
            && let Ok(body) = form_encode(v)
            && !body.is_empty()
        {
            out.push(String::from_utf8_lossy(&body).into_owned());
        }
    } else if !v.is_null() {
        out.push(crate::params::pct(&v.to_string(), false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `form_decode` read before it went field by field: the object of the fields.
    fn through_object<T: DeserializeOwned>(body: &[u8]) -> Result<T, String> {
        let text = lossy(body);
        let mut o = Map::new();
        for (k, v) in parse_query(&text) {
            push_multi(&mut o, k.into_owned(), Value::String(v.into_owned()));
        }
        T::deserialize(Coerce(Value::Object(o))).map_err(|e| e.to_string())
    }

    #[test]
    fn fields_read_as_the_object_of_them_did() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Order {
            id: i64,
            tags: Vec<String>,
            note: Option<String>,
            done: bool,
        }
        let body = b"tags=b&id=7&done=true&tags=a&note=fragile%2C+keep+upright";
        let order: Order = form_decode(Some(200), body).unwrap();
        assert_eq!(order.tags, ["b", "a"], "a repeated field keeps its order");
        assert_eq!(Ok(order), through_object::<Order>(body));
        let fields: std::collections::BTreeMap<String, String> = form_decode(Some(200), b"b=2&a=1&c=%20").unwrap();
        assert_eq!(Ok(fields), through_object(b"b=2&a=1&c=%20"));
        let one: Order = form_decode(Some(200), b"id=1&tags=x&done=false").unwrap();
        assert_eq!(one.tags, ["x"], "one value where a list is expected is a list of one");
        assert!(form_decode::<Order>(Some(200), b"id=x&tags=a&done=true").is_err());
        assert!(through_object::<Order>(b"id=x&tags=a&done=true").is_err());
    }

    /// 3.2 "Parsing and Serializing": `extra` is `application/xml` in the form. `code` is a string that looks like a
    /// number. The request body of the spec decodes to its data form. The data form encodes back to it.
    #[cfg(feature = "xml")]
    #[test]
    fn a_form_field_travels_in_its_encoding_content_type() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct Extra {
            info: String,
        }
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct FormData {
            code: String,
            count: i64,
            extra: Extra,
        }
        let types = [("extra", "application/xml")];
        let spec = b"code=1234&count=42&extra=%3Cinfo%3Eabc%3C/info%3E";
        let data: FormData = form_decode_with(Some(200), spec, &[], &types).unwrap();
        let want = FormData { code: "1234".into(), count: 42, extra: Extra { info: "abc".into() } };
        assert_eq!(data, want);
        let body = form_encode_with(&data, &[], &types).unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "code=1234&count=42&extra=%3Cinfo%3Eabc%3C%2Finfo%3E");
        assert_eq!(form_decode_with::<FormData>(Some(200), &body, &[], &types).unwrap(), want);
    }

    #[test]
    fn a_json_form_field_is_a_quoted_json_string() {
        let id = [("id", "application/json")];
        let json = form_encode_with(&serde_json::json!({"id": "f81d4fae"}), &[], &id).unwrap();
        assert_eq!(String::from_utf8_lossy(&json), "id=%22f81d4fae%22");
    }

    /// The learn.openapis.org `querystring` example: a form query string with a
    /// free-form object property keeps the property's name.
    #[test]
    fn a_form_querystring_keeps_object_properties_by_name() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Advanced {
            filters: Option<Map<String, Value>>,
            sorting: Option<Vec<String>>,
        }
        let v = serde_json::json!({"filters": {"k": "v"}, "sorting": ["a", "b"]});
        let mut out = vec![];
        encode_querystring(&mut out, &v, true);
        let raw = out.join("&");
        assert_eq!(raw, "filters=%7B%22k%22%3A%22v%22%7D&sorting=a&sorting=b");
        let back: Advanced =
            serde::Deserialize::deserialize(Coerce(crate::params::decode_querystring(&raw, true).unwrap())).unwrap();
        assert_eq!(
            back,
            Advanced { filters: v["filters"].as_object().cloned(), sorting: Some(vec!["a".into(), "b".into()]) }
        );
    }

    /// A field whose `encoding.contentType` is JSON is read as JSON. A text field stays text.
    #[test]
    fn a_json_field_is_read_as_json() {
        let styles = [("meta", Style::Form, Shape::Prim, true), ("note", Style::Form, Shape::Prim, true)];
        let types = [("meta", "application/json"), ("note", "text/plain")];
        let got: Value = form_decode_with(None, b"meta=%7B%22a%22%3A1%7D&note=%7B%7D", &styles, &types).unwrap();
        assert_eq!(got, serde_json::json!({"meta": {"a": 1}, "note": "{}"}));
    }
}
