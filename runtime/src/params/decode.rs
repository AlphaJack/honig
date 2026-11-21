//! Reading a parameter value in its style: path, query, header and cookie.

use std::borrow::Cow;

use serde::Serialize;
use serde::de::{DeserializeOwned, Deserializer, Visitor};
use serde_json::{Map, Value};

#[cfg(feature = "cookie")]
use super::cookie_pairs;
use super::{Query, Shape, Style, parse_query, pct_decode};
use crate::json::push_multi;
use crate::{ClientError, Reason, coerce::Coerce, coerce::CoerceError, json::to_value};

fn split_value(raw: &str, shape: Shape, explode: bool, sep: char) -> Value {
    match shape {
        Shape::Prim => Value::String(raw.to_owned()),
        // An empty value is the empty (RFC 6570 "undefined") list. Inside a
        // list an empty item is the empty string. `a,,b` has three items.
        Shape::Array if raw.is_empty() => Value::Array(vec![]),
        Shape::Array => Value::Array(raw.split(sep).map(|s| Value::String(s.to_owned())).collect()),
        Shape::Object => {
            let mut o = Map::new();
            if explode {
                for kv in raw.split(sep) {
                    if let Some((k, v)) = kv.split_once('=') {
                        o.insert(k.to_owned(), Value::String(v.to_owned()));
                    }
                }
            } else {
                let parts: Vec<&str> = raw.split(sep).collect();
                for pair in parts.chunks(2) {
                    if let [k, v] = pair {
                        o.insert(k.to_string(), Value::String(v.to_string()));
                    }
                }
            }
            Value::Object(o)
        }
    }
}

fn decode_parts(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(pct_decode(&s)),
        Value::Array(a) => Value::Array(a.into_iter().map(decode_parts).collect()),
        Value::Object(o) => Value::Object(o.into_iter().map(|(k, v)| (pct_decode(&k), decode_parts(v))).collect()),
        other @ (Value::Null | Value::Bool(_) | Value::Number(_)) => other,
    }
}

/// Parses a raw (still percent-encoded) path segment.
pub fn decode_path(raw: &str, name: &str, style: Style, shape: Shape, explode: bool) -> Option<Value> {
    let v = match style {
        Style::Label => {
            let body = raw.strip_prefix('.').unwrap_or(raw);
            let sep = if explode { '.' } else { ',' };
            split_value(body, shape, explode, sep)
        }
        Style::Matrix => {
            let body = raw.strip_prefix(';').unwrap_or(raw);
            match (shape, explode) {
                (Shape::Array, true) => Value::Array(
                    body.split(';')
                        .filter_map(|p| p.split_once('='))
                        .map(|(_, v)| Value::String(v.to_owned()))
                        .collect(),
                ),
                (Shape::Object, true) => split_value(body, shape, true, ';'),
                _ => {
                    let value = body.split_once('=').map(|(_, v)| v).unwrap_or_default();
                    let _ = name;
                    split_value(value, shape, false, ',')
                }
            }
        }
        Style::Simple
        | Style::Form
        | Style::SpaceDelimited
        | Style::TabDelimited
        | Style::PipeDelimited
        | Style::DeepObject
        | Style::Cookie => split_value(raw, shape, explode, ','),
    };
    Some(decode_parts(v))
}

/// Returns a header declared with JSON `content` (`X-Rate: {"limit":10}`).
pub fn json_header(raw: Option<&str>) -> Option<Value> {
    raw.and_then(|r| serde_json::from_str(r.trim()).ok())
}

/// Returns a header declared with JSON `content`: the value as compact JSON text.
pub fn encode_json_header(v: &Value) -> Option<String> {
    (!v.is_null()).then(|| v.to_string())
}

/// Parses a header value. `None` when the header is absent.
pub fn decode_header(raw: Option<&str>, shape: Shape, explode: bool) -> Option<Value> {
    let r = raw?;
    Some(split_value(r.trim(), shape, explode, ','))
}

/// Returns an exploded form object without the keys of the operation's other query
/// parameters; `None` when nothing of its own is left.
#[must_use]
pub fn without_keys(v: Option<Value>, keys: &[&str]) -> Option<Value> {
    match v {
        Some(Value::Object(mut o)) => {
            for k in keys {
                o.remove(*k);
            }
            (!o.is_empty()).then_some(Value::Object(o))
        }
        other => other,
    }
}

/// Parses a query parameter from decoded query pairs.
///
/// # Errors
/// A parameter that carries one value (a scalar or a delimited array or object) is sent more than once.
pub fn decode_query(
    pairs: &Query<'_>,
    name: &'static str,
    at: &str,
    style: Style,
    shape: Shape,
    explode: bool,
) -> Result<Option<Value>, Reason> {
    // The values of the parameter are found again where needed. Nothing is collected.
    let values = || pairs.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_ref());
    let one_value =
        shape == Shape::Prim || matches!(style, Style::SpaceDelimited | Style::PipeDelimited | Style::TabDelimited);
    if style != Style::DeepObject && one_value && values().nth(1).is_some() {
        return Err(repeated(at));
    }
    Ok(match (style, shape) {
        (Style::DeepObject, _) => {
            let mut root = Map::new();
            let mut any = false;
            for (k, v) in pairs {
                let Some(rest) = k.strip_prefix(name).filter(|r| r.starts_with('[')) else { continue };
                any = true;
                let keys: Vec<&str> = rest.split(['[', ']']).filter(|s| !s.is_empty()).collect();
                insert_deep(&mut root, &keys, v);
            }
            any.then_some(Value::Object(root))
        }
        (_, Shape::Prim) => values().next().map(|v| Value::String(v.to_owned())),
        (Style::SpaceDelimited, Shape::Array | Shape::Object) => {
            values().next().map(|v| split_value(v, shape, false, ' '))
        }
        (Style::PipeDelimited, Shape::Array | Shape::Object) => {
            values().next().map(|v| split_value(v, shape, false, '|'))
        }
        (Style::TabDelimited, Shape::Array | Shape::Object) => {
            values().next().map(|v| split_value(v, shape, false, '\t'))
        }
        (_, Shape::Array) => {
            let Some(first) = values().next() else { return Ok(None) };
            if explode {
                Some(Value::Array(values().map(|v| Value::String(v.to_owned())).collect()))
            } else if values().nth(1).is_none() {
                Some(split_value(first, shape, false, ','))
            } else {
                // `explode: false` sent exploded anyway (`tags=a&tags=b`) as some
                // clients do. Accept every value and not only the first.
                let mut items = vec![];
                for v in values() {
                    if let Value::Array(a) = split_value(v, shape, false, ',') {
                        items.extend(a);
                    }
                }
                Some(Value::Array(items))
            }
        }
        (_, Shape::Object) => {
            if explode {
                // Exploded objects spread their properties over the whole query.
                let o: Map<String, Value> = pairs
                    .iter()
                    .map(|(k, v)| (k.clone().into_owned(), Value::String(v.clone().into_owned())))
                    .collect();
                (!o.is_empty()).then_some(Value::Object(o))
            } else {
                values().next().map(|v| split_value(v, shape, false, ','))
            }
        }
    })
}

fn insert_deep(root: &mut Map<String, Value>, keys: &[&str], v: &str) {
    let Some((first, rest)) = keys.split_first() else {
        return;
    };
    if rest.is_empty() {
        match root.get_mut(*first) {
            Some(Value::Array(a)) => a.push(Value::String(v.to_owned())),
            Some(existing) => {
                let prev = existing.take();
                *existing = Value::Array(vec![prev, Value::String(v.to_owned())]);
            }
            None => {
                root.insert(first.to_string(), Value::String(v.to_owned()));
            }
        }
        return;
    }
    let child = root.entry(first.to_string()).or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(m) = child {
        insert_deep(m, rest, v);
    }
}

/// Returns `allowEmptyValue`: an empty query value is `true` for a flag. It is kept for a
/// string. Otherwise it is absent.
pub fn empty_value(v: Option<Value>, flag: bool, text: bool) -> Option<Value> {
    match v {
        Some(Value::String(s)) if s.is_empty() && flag => Some(Value::Bool(true)),
        Some(Value::String(s)) if s.is_empty() && !text => None,
        other => other,
    }
}

/// Parses the whole query string (3.2 `in: querystring`).
pub fn decode_querystring(raw_query: &str, form: bool) -> Option<Value> {
    if raw_query.is_empty() {
        return None;
    }
    if form {
        let mut o = Map::new();
        for (k, v) in parse_query(raw_query) {
            push_multi(&mut o, k.into_owned(), Value::String(v.into_owned()));
        }
        Some(Value::Object(o))
    } else {
        serde_json::from_str(&pct_decode(raw_query)).ok()
    }
}

/// Parses a cookie parameter from (name, value) pairs.
#[cfg(feature = "cookie")]
pub fn decode_cookie(header: Option<&str>, name: &str, style: Style, shape: Shape, explode: bool) -> Option<Value> {
    // Borrowed (name, value) pairs. Only the cookie that was asked for is copied.
    let pairs = || cookie_pairs(header.unwrap_or(""));
    let dec = |s: &str| {
        if style == Style::Cookie { s.to_owned() } else { pct_decode(s) }
    };
    match (shape, explode) {
        (Shape::Array, true) => {
            let vs: Vec<Value> = pairs().filter(|(k, _)| *k == name).map(|(_, v)| Value::String(dec(v))).collect();
            (!vs.is_empty()).then_some(Value::Array(vs))
        }
        (Shape::Object, true) => {
            Some(Value::Object(pairs().map(|(k, v)| (k.to_owned(), Value::String(dec(v)))).collect()))
        }
        // One string. It is decoded straight into the value and not split first.
        (Shape::Prim, _) => pairs().find(|(k, _)| *k == name).map(|(_, v)| Value::String(pct_decode(v))),
        _ => pairs().find(|(k, _)| *k == name).map(|(_, v)| decode_parts(split_value(v, shape, false, ','))),
    }
}

/// Creates [`from_param`] for a scalar parameter given as its decoded text. `at` is where the parameter is in the
/// request (`/query/limit`): a violation points to it.
///
/// Numbers and booleans parse straight from the text with no `Value` and no allocation. Every other target decodes it
/// as [`Coerce`] decodes a JSON string.
pub fn from_text<T: DeserializeOwned>(v: Option<Cow<'_, str>>, at: &str) -> Result<T, Reason> {
    match v {
        None => from_param(None, at),
        Some(s) => T::deserialize(Text(s)).map_err(|e| Reason::invalid(at, e.to_string())),
    }
}

/// Returns the scalar path parameter at `at` (`/path/id`) as `T`. It is the `index`th variable of the matched
/// template.
///
/// # Errors
/// The parameter is absent or does not decode as `T`.
pub fn path_param<T: DeserializeOwned>(caps: &[&str], index: usize, style: Style, at: &str) -> Result<T, Reason> {
    from_text(caps.get(index).map(|c| path_text(c, style)), at)
}

/// Returns a scalar path segment's text: `.x` (label) and `;name=x` (matrix) unwrapped,
/// then percent-decoded (borrowed when there is nothing to decode).
pub fn path_text(raw: &str, style: Style) -> Cow<'_, str> {
    let v = match style {
        Style::Label => raw.strip_prefix('.').unwrap_or(raw),
        Style::Matrix => raw.strip_prefix(';').unwrap_or(raw).split_once('=').map(|(_, v)| v).unwrap_or_default(),
        Style::Simple
        | Style::Form
        | Style::SpaceDelimited
        | Style::TabDelimited
        | Style::PipeDelimited
        | Style::DeepObject
        | Style::Cookie => raw,
    };
    pct_decode_cow(v)
}

/// Returns the text of a scalar query parameter. It is already decoded.
///
/// # Errors
/// The parameter is sent more than once (`a=1&a=2`). Servers disagree on which value counts. So none is picked.
pub fn query_text<'q>(pairs: &'q Query<'_>, name: &str, at: &str) -> Result<Option<Cow<'q, str>>, Reason> {
    let mut values = pairs.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_ref());
    let first = values.next();
    if values.next().is_some() {
        return Err(repeated(at));
    }
    Ok(first.map(Cow::Borrowed))
}

/// Returns a parameter at `at` that carries one value but is sent more than once.
fn repeated(at: &str) -> Reason {
    Reason::invalid(at, "is sent more than once, but it carries one value")
}

/// Returns the trimmed text of a scalar header.
pub fn trimmed_header(raw: Cow<'_, str>) -> Cow<'_, str> {
    match raw {
        Cow::Borrowed(s) => Cow::Borrowed(s.trim()),
        Cow::Owned(s) => Cow::Owned(s.trim().to_owned()),
    }
}

/// Returns the percent-decoded text of a scalar cookie.
#[cfg(feature = "cookie")]
pub fn cookie_text<'h>(header: Option<&'h str>, name: &str) -> Option<Cow<'h, str>> {
    cookie_pairs(header?).find(|(k, _)| *k == name).map(|(_, v)| pct_decode_cow(v))
}

/// Returns [`pct_decode`] that borrows when there is no `%`.
fn pct_decode_cow(s: &str) -> Cow<'_, str> {
    if s.contains('%') { Cow::Owned(pct_decode(s)) } else { Cow::Borrowed(s) }
}

/// The deserializer of [`from_text`].
struct Text<'a>(Cow<'a, str>);

impl Text<'_> {
    fn coerce(self) -> Coerce {
        Coerce(Value::String(self.0.into_owned()))
    }
}

macro_rules! text_num {
    ($method:ident, $visit:ident, $ty:ty) => {
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
            match self.0.trim().parse::<$ty>() {
                Ok(n) => v.$visit(n),
                Err(_) => Err(CoerceError(format!("`{}` is not a valid {}", self.0, stringify!($ty)))),
            }
        }
    };
}

macro_rules! text_forward {
    ($($method:ident),*) => {
        $(fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
            self.coerce().$method(v)
        })*
    };
}

impl<'de> Deserializer<'de> for Text<'_> {
    type Error = CoerceError;

    text_num!(deserialize_i8, visit_i8, i8);
    text_num!(deserialize_i16, visit_i16, i16);
    text_num!(deserialize_i32, visit_i32, i32);
    text_num!(deserialize_i64, visit_i64, i64);
    text_num!(deserialize_u8, visit_u8, u8);
    text_num!(deserialize_u16, visit_u16, u16);
    text_num!(deserialize_u32, visit_u32, u32);
    text_num!(deserialize_u64, visit_u64, u64);
    text_num!(deserialize_f32, visit_f32, f32);
    text_num!(deserialize_f64, visit_f64, f64);

    fn deserialize_bool<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0.trim() {
            "true" | "1" => v.visit_bool(true),
            "false" | "0" => v.visit_bool(false),
            _ => Err(CoerceError(format!("`{}` is not a boolean", self.0))),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        v.visit_some(self)
    }

    text_forward!(
        deserialize_any,
        deserialize_i128,
        deserialize_u128,
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_unit,
        deserialize_seq,
        deserialize_map,
        deserialize_identifier,
        deserialize_ignored_any
    );

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        self.coerce().deserialize_unit_struct(name, v)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        self.coerce().deserialize_newtype_struct(name, v)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, CoerceError> {
        self.coerce().deserialize_tuple(len, v)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.coerce().deserialize_tuple_struct(name, len, v)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.coerce().deserialize_struct(name, fields, v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.coerce().deserialize_enum(name, variants, v)
    }
}

/// Turns a parsed parameter into its typed value.
///
/// Missing becomes `null`. That is fine for `Option` and an error for required parameters. A missing one is decoded
/// strictly. The lenient [`Coerce`] would decode `null` as an empty list. Then a required array would pass when it
/// was not sent. `at` is where the parameter is in the request (`/query/limit`): a violation points to it.
pub fn from_param<T: DeserializeOwned>(v: Option<Value>, at: &str) -> Result<T, Reason> {
    match v {
        // Not sent: what the decoder says about `null` adds nothing.
        None => T::deserialize(Value::Null).map_err(|_not_optional| Reason::invalid(at, "is required")),
        Some(v) => T::deserialize(Coerce(v)).map_err(|e| Reason::invalid(at, e.to_string())),
    }
}

/// Returns a parameter declared with JSON `content`: the value travels as JSON text.
pub fn json_param<T: Serialize>(v: &T) -> Result<Value, ClientError> {
    let j = to_value(v)?;
    Ok(if j.is_null() { Value::Null } else { Value::String(j.to_string()) })
}

/// Deserializes a parameter value. The error points to the parameter by `at` (`/header/X-Filter`).
pub fn from_json_param<T: DeserializeOwned>(v: Option<Value>, at: &str) -> Result<T, Reason> {
    match v {
        Some(Value::String(s)) => serde_json::from_str(&s).map_err(|e| Reason::invalid(at, e.to_string())),
        other => from_param(other, at),
    }
}

#[cfg(test)]
mod tests {
    use super::{Shape, Style, decode_path};
    use serde_json::json;

    #[cfg(feature = "cookie")]
    #[test]
    fn a_cookie_is_found_by_its_name() {
        assert_eq!(super::decode_cookie(Some("a=1; b=2"), "b", Style::Form, Shape::Prim, true), Some(json!("2")));
    }

    #[test]
    fn a_label_object_splits_at_its_separator() {
        assert_eq!(decode_path(".a=1.b=2", "p", Style::Label, Shape::Object, true), Some(json!({"a": "1", "b": "2"})));
        assert_eq!(decode_path(".a,1,b,2", "p", Style::Label, Shape::Object, false), Some(json!({"a": "1", "b": "2"})));
    }
}
