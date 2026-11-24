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

/// Splits a parameter value in its style. `None` for an object member without `=` or a list of names and values
/// that ends on a name.
fn split_value(raw: &str, shape: Shape, explode: bool, sep: char) -> Option<Value> {
    Some(match shape {
        Shape::Primitive => Value::String(raw.to_owned()),
        // An empty value is the empty (RFC 6570 "undefined") list. Inside a
        // list an empty item is the empty string. `a,,b` has three items.
        Shape::Array if raw.is_empty() => Value::Array(vec![]),
        Shape::Array => Value::Array(raw.split(sep).map(|s| Value::String(s.to_owned())).collect()),
        Shape::Object if raw.is_empty() => Value::Object(Map::new()),
        Shape::Object if explode => Value::Object(
            raw.split(sep)
                .map(|kv| kv.split_once('=').map(|(k, v)| (k.to_owned(), Value::String(v.to_owned()))))
                .collect::<Option<_>>()?,
        ),
        Shape::Object => {
            let parts: Vec<&str> = raw.split(sep).collect();
            let pairs = parts.chunks(2).map(|pair| match pair {
                [k, v] => Some(((*k).to_owned(), Value::String((*v).to_owned()))),
                _ => None,
            });
            Value::Object(pairs.collect::<Option<_>>()?)
        }
    })
}

/// Percent-decodes every name and value. `None` when one is not UTF-8.
fn decode_parts(v: Value) -> Option<Value> {
    Some(match v {
        Value::String(s) => Value::String(pct_decode(&s)?),
        Value::Array(a) => Value::Array(a.into_iter().map(decode_parts).collect::<Option<_>>()?),
        Value::Object(o) => {
            Value::Object(o.into_iter().map(|(k, v)| Some((pct_decode(&k)?, decode_parts(v)?))).collect::<Option<_>>()?)
        }
        other @ (Value::Null | Value::Bool(_) | Value::Number(_)) => other,
    })
}

/// Returns the error of a parameter at `at` whose value does not decode in its style.
fn malformed(at: &str) -> Reason {
    Reason::invalid(at, "does not decode in its style: a member without a value, or text that is not UTF-8")
}

/// Parses a raw (still percent-encoded) path segment.
///
/// # Errors
/// It does not decode in its style.
pub fn decode_path(raw: &str, at: &str, style: Style, shape: Shape, explode: bool) -> Result<Value, Reason> {
    let v = match style {
        Style::Label => {
            let body = raw.strip_prefix('.').unwrap_or(raw);
            let sep = if explode { '.' } else { ',' };
            split_value(body, shape, explode, sep)
        }
        Style::Matrix => {
            let body = raw.strip_prefix(';').unwrap_or(raw);
            match (shape, explode) {
                (Shape::Array, true) => body
                    .split(';')
                    .map(|p| p.split_once('=').map(|(_, v)| Value::String(v.to_owned())))
                    .collect::<Option<_>>()
                    .map(Value::Array),
                (Shape::Object, true) => split_value(body, shape, true, ';'),
                _ => body.split_once('=').and_then(|(_, v)| split_value(v, shape, false, ',')),
            }
        }
        // A Swagger 2.0 `collectionFormat` of a path array: split at its encoded separator, in either case of hex.
        Style::SpaceDelimited | Style::TabDelimited | Style::PipeDelimited if shape == Shape::Array => {
            let sep = style.delimiter().unwrap_or(",");
            let raw = raw.replace(&sep.to_ascii_lowercase(), sep);
            Some(Value::Array(raw.split(sep).map(|p| Value::String(p.to_owned())).collect()))
        }
        Style::Simple
        | Style::Form
        | Style::SpaceDelimited
        | Style::TabDelimited
        | Style::PipeDelimited
        | Style::DeepObject
        | Style::Cookie => split_value(raw, shape, explode, ','),
    };
    v.and_then(decode_parts).ok_or_else(|| malformed(at))
}

/// Returns the JSON text of a header declared with JSON `content` (`X-Rate: {"limit":10}`) for [`from_json_param`],
/// which refuses text that does not parse.
pub fn json_header(raw: &str) -> Value {
    Value::String(raw.trim().to_owned())
}

/// Returns a header declared with JSON `content`: the value as compact JSON text.
pub fn encode_json_header(v: &Value) -> Option<String> {
    (!v.is_null()).then(|| v.to_string())
}

/// Parses a header value. `None` when the header is absent.
///
/// # Errors
/// It does not decode in its style.
pub fn decode_header(
    raw: Option<&str>,
    at: &str,
    style: Style,
    shape: Shape,
    explode: bool,
) -> Result<Option<Value>, Reason> {
    raw.map(|r| split_value(r.trim(), shape, explode, style.header_separator()).ok_or_else(|| malformed(at)))
        .transpose()
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
    let one_value = shape == Shape::Primitive
        || matches!(style, Style::SpaceDelimited | Style::PipeDelimited | Style::TabDelimited);
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
                if keys.len() > MAX_DEPTH {
                    return Err(Reason::invalid(at, format!("is nested deeper than {MAX_DEPTH} levels")));
                }
                insert_deep(&mut root, &keys, v);
            }
            any.then_some(Value::Object(root))
        }
        (_, Shape::Primitive) => values().next().map(|v| Value::String(v.to_owned())),
        (Style::SpaceDelimited, Shape::Array | Shape::Object) => split_one(values().next(), shape, ' ', at)?,
        (Style::PipeDelimited, Shape::Array | Shape::Object) => split_one(values().next(), shape, '|', at)?,
        (Style::TabDelimited, Shape::Array | Shape::Object) => split_one(values().next(), shape, '\t', at)?,
        (_, Shape::Array) => {
            let Some(first) = values().next() else { return Ok(None) };
            if explode {
                Some(Value::Array(values().map(|v| Value::String(v.to_owned())).collect()))
            } else if values().nth(1).is_none() {
                split_one(Some(first), shape, ',', at)?
            } else {
                // `explode: false` sent exploded anyway (`tags=a&tags=b`) as some
                // clients do. Accept every value and not only the first.
                let mut items = vec![];
                for v in values() {
                    if let Some(Value::Array(a)) = split_one(Some(v), shape, ',', at)? {
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
                split_one(values().next(), shape, ',', at)?
            }
        }
    })
}

/// Splits the one value of a delimited query parameter at `at`, when it is sent.
fn split_one(v: Option<&str>, shape: Shape, sep: char, at: &str) -> Result<Option<Value>, Reason> {
    v.map(|v| split_value(v, shape, false, sep).ok_or_else(|| malformed(at))).transpose()
}

/// How deep a `deepObject` name may nest: as deep as a JSON body (`serde_json`'s limit), so no query decodes into a
/// value that the JSON decoder would refuse.
const MAX_DEPTH: usize = 128;

/// Sets `v` at the path `keys` of `root`. A loop: no input sets the depth of the stack.
fn insert_deep(root: &mut Map<String, Value>, keys: &[&str], v: &str) {
    let Some((last, path)) = keys.split_last() else { return };
    let mut node = root;
    for k in path {
        node = match node.entry((*k).to_owned()).or_insert_with(|| Value::Object(Map::new())) {
            Value::Object(m) => m,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => return,
        };
    }
    match node.get_mut(*last) {
        Some(Value::Array(a)) => a.push(Value::String(v.to_owned())),
        Some(existing) => {
            let prev = existing.take();
            *existing = Value::Array(vec![prev, Value::String(v.to_owned())]);
        }
        None => {
            node.insert((*last).to_owned(), Value::String(v.to_owned()));
        }
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
///
/// # Errors
/// A JSON query string does not parse.
pub fn decode_querystring(raw_query: &str, at: &str, form: bool) -> Result<Option<Value>, Reason> {
    if raw_query.is_empty() {
        return Ok(None);
    }
    if form {
        let mut o = Map::new();
        for (k, v) in parse_query(raw_query) {
            push_multi(&mut o, k.into_owned(), Value::String(v.into_owned()));
        }
        Ok(Some(Value::Object(o)))
    } else {
        let text = pct_decode(raw_query).ok_or_else(|| malformed(at))?;
        serde_json::from_str(&text).map(Some).map_err(|e| Reason::invalid(at, e.to_string()))
    }
}

/// Parses a cookie parameter from (name, value) pairs.
#[cfg(feature = "cookie")]
///
/// # Errors
/// It does not decode in its style.
pub fn decode_cookie(
    header: Option<&str>,
    name: &str,
    at: &str,
    style: Style,
    shape: Shape,
    explode: bool,
) -> Result<Option<Value>, Reason> {
    // Borrowed (name, value) pairs. Only the cookie that was asked for is copied.
    let pairs = || cookie_pairs(header.unwrap_or(""));
    // `style: cookie` sends values as they are (3.2): only `form` percent-encodes them.
    let dec = |s: &str| if style == Style::Cookie { Some(s.to_owned()) } else { pct_decode(s) };
    let text = |s: &str| dec(s).map(Value::String).ok_or_else(|| malformed(at));
    Ok(match (shape, explode) {
        (Shape::Array, true) => {
            let vs: Vec<Value> = pairs().filter(|(k, _)| *k == name).map(|(_, v)| text(v)).collect::<Result<_, _>>()?;
            (!vs.is_empty()).then_some(Value::Array(vs))
        }
        (Shape::Object, true) => {
            Some(Value::Object(pairs().map(|(k, v)| Ok((k.to_owned(), text(v)?))).collect::<Result<_, Reason>>()?))
        }
        // One string. It is decoded straight into the value and not split first.
        (Shape::Primitive, _) => pairs().find(|(k, _)| *k == name).map(|(_, v)| text(v)).transpose()?,
        _ => pairs()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| {
                let split = split_value(v, shape, false, ',');
                if style == Style::Cookie { split } else { split.and_then(decode_parts) }.ok_or_else(|| malformed(at))
            })
            .transpose()?,
    })
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

/// Decodes a text body as `T`, the type of its schema, as [`from_text`] decodes a parameter. Invalid UTF-8 is replaced.
///
/// # Errors
/// The text is not a value of `T`: a string outside its limits, or not one of its values.
pub fn text_decode_as<T: DeserializeOwned>(status: Option<u16>, body: &[u8]) -> Result<T, ClientError> {
    let text = crate::text::utf8_text(status, body)?;
    T::deserialize(Text(Cow::Borrowed(text))).map_err(|e| crate::text::decode_err(status, body, e))
}

/// Returns the scalar path parameter at `at` (`/path/id`) as `T`. It is the `index`th variable of the matched
/// template.
///
/// # Errors
/// The parameter is absent or does not decode as `T`.
pub fn path_param<T: DeserializeOwned>(caps: &[&str], index: usize, style: Style, at: &str) -> Result<T, Reason> {
    from_text(caps.get(index).map(|c| path_text(c, style).ok_or_else(|| malformed(at))).transpose()?, at)
}

/// Returns a scalar path segment's text: `.x` (label) and `;name=x` (matrix) unwrapped,
/// then percent-decoded (borrowed when there is nothing to decode). `None` when it is not UTF-8.
pub fn path_text(raw: &str, style: Style) -> Option<Cow<'_, str>> {
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

/// Returns the text of a scalar cookie: percent-decoded unless `style` is `cookie`, which sends it as it is.
///
/// # Errors
/// The decoded text is not UTF-8.
#[cfg(feature = "cookie")]
pub fn cookie_text<'h>(
    header: Option<&'h str>,
    name: &str,
    at: &str,
    style: Style,
) -> Result<Option<Cow<'h, str>>, Reason> {
    let Some(v) = header.and_then(|h| cookie_pairs(h).find(|(k, _)| *k == name).map(|(_, v)| v)) else {
        return Ok(None);
    };
    if style == Style::Cookie {
        return Ok(Some(Cow::Borrowed(v)));
    }
    pct_decode_cow(v).map(Some).ok_or_else(|| malformed(at))
}

/// Returns [`pct_decode`] that borrows when there is no `%`.
fn pct_decode_cow(s: &str) -> Option<Cow<'_, str>> {
    if s.contains('%') { pct_decode(s).map(Cow::Owned) } else { Some(Cow::Borrowed(s)) }
}

/// The deserializer of [`from_text`].
struct Text<'a>(Cow<'a, str>);

impl Text<'_> {
    fn coerce(self) -> Coerce {
        Coerce(Value::String(self.0.into_owned()))
    }
}

/// Text parsed as a number of `$ty` in the grammar of JSON, as [`crate::coerce::number`] parses it everywhere.
macro_rules! text_num {
    ($method:ident, $visit:ident, $ty:ty, $format:literal, $ok:expr) => {
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
            match crate::coerce::number::<$ty>(&self.0, $ok) {
                Some(n) => v.$visit(n),
                None => Err(crate::coerce::not_a_number(&self.0, $format)),
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

    crate::coerce::numbers!(text_num);

    fn deserialize_bool<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        crate::coerce::boolean(&self.0).map_or_else(|| Err(crate::coerce::not_a_boolean(&self.0)), |b| v.visit_bool(b))
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

    /// A `deepObject` name nested past 128 levels is refused before it is decoded: on a 2 MB stack, as a server thread
    /// may have, 8000 levels would overflow it. 128 levels still decode.
    /// A number in a parameter is a JSON number, as it is in a list or a body.
    #[test]
    fn a_scalar_parameter_is_a_json_number() {
        let text = |t: &'static str| Some(std::borrow::Cow::Borrowed(t));
        for bad in ["NaN", "inf", "-inf", "infinity", "1e999", "+5", " 5 ", "007"] {
            assert!(super::from_text::<f64>(text(bad), "/q").is_err(), "{bad} as f64");
            assert!(super::from_text::<i64>(text(bad), "/q").is_err(), "{bad} as i64");
        }
        assert_eq!(super::from_text::<i64>(text("5"), "/q").ok(), Some(5));
        assert_eq!(super::from_text::<i64>(text("-5"), "/q").ok(), Some(-5));
        assert_eq!(super::from_text::<f64>(text("5.0e0"), "/q").ok(), Some(5.0));
    }

    #[test]
    fn a_deep_object_nested_too_deep_is_refused() {
        let query = |levels: usize| format!("a{}=1", "[b]".repeat(levels));
        let decode = |levels: usize| {
            std::thread::Builder::new()
                .stack_size(2 << 20)
                .spawn(move || {
                    let q = query(levels);
                    let pairs = super::parse_query(&q);
                    super::decode_query(&pairs, "a", "/query/a", Style::DeepObject, Shape::Object, true)
                        .map(|v| v.is_some())
                })
                .expect("a thread")
                .join()
                .expect("no overflow")
        };
        assert!(decode(8000).is_err());
        assert_eq!(decode(128), Ok(true));
    }

    /// A JSON query string that does not parse is an error at its pointer, not an absent parameter.
    #[test]
    fn a_json_query_string_that_does_not_parse_is_refused() {
        assert!(super::decode_querystring("%7B", "/querystring/q", false).is_err());
        assert_eq!(super::decode_querystring("%7B%7D", "/querystring/q", false), Ok(Some(json!({}))));
    }

    #[cfg(feature = "cookie")]
    #[test]
    fn a_cookie_is_found_by_its_name() {
        assert_eq!(
            super::decode_cookie(Some("a=1; b=2"), "b", "/cookie/b", Style::Form, Shape::Primitive, true),
            Ok(Some(json!("2")))
        );
    }

    #[test]
    fn a_label_object_splits_at_its_separator() {
        assert_eq!(
            decode_path(".a=1.b=2", "/path/p", Style::Label, Shape::Object, true),
            Ok(json!({"a": "1", "b": "2"}))
        );
        assert_eq!(
            decode_path(".a,1,b,2", "/path/p", Style::Label, Shape::Object, false),
            Ok(json!({"a": "1", "b": "2"}))
        );
    }
}
