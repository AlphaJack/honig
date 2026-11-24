//! Writing a parameter value in its style: path, query, header and cookie.

use serde::Serialize;
use serde_json::{Map, Value};

use super::{Enc, Style, pct, pct_path};
use crate::auth::add_cookie;
use crate::scalar::{scalar, scalar_text};
use crate::{ClientError, Request, json::to_value};

/// Encodes a path parameter. Returns the percent-encoded segment text.
/// Errors: the value does not serialize.
pub fn encode_path<T: Serialize + ?Sized>(name: &str, v: &T, enc: Enc) -> Result<String, ClientError> {
    let Some(s) = scalar_text(v) else {
        let value = to_value(v)?;
        return Ok(encode_path_value(name, &value, enc, &object_order(&value, v)));
    };
    let e = pct_path(&s, enc.allow_reserved);
    Ok(match enc.style {
        Style::Label => format!(".{e}"),
        Style::Matrix => format!(";{name}={e}"),
        Style::Simple
        | Style::Form
        | Style::SpaceDelimited
        | Style::TabDelimited
        | Style::PipeDelimited
        | Style::DeepObject
        | Style::Cookie => e,
    })
}

/// Encodes a header value. `None` for null (absent).
///
/// Its style is `simple`, or for an array the delimited style of a Swagger 2.0 `collectionFormat`.
/// Errors: the value does not serialize, or an item contains the separator.
pub fn encode_header<T: Serialize + ?Sized>(v: &T, enc: Enc) -> Result<Option<String>, ClientError> {
    match scalar_text(v) {
        Some(s) => Ok(Some(s)),
        None => {
            let value = to_value(v)?;
            reject_separator(&value, &[enc.style.header_separator()], "header")?;
            Ok(encode_header_value(&value, enc, &object_order(&value, v)))
        }
    }
}

/// Appends encoded `key=value` pairs for a query parameter. Nulls are skipped.
/// Errors: the value does not serialize.
pub fn encode_query<T: Serialize + ?Sized>(
    out: &mut Vec<String>,
    name: &str,
    v: &T,
    enc: Enc,
) -> Result<(), ClientError> {
    match scalar_text(v) {
        Some(s) => out.push(format!("{}={}", pct(name, false), pct(&s, enc.allow_reserved))),
        None => {
            let value = to_value(v)?;
            if let Some(separator) = enc.style.separator() {
                reject_separator(&value, &[separator], name)?;
            }
            encode_query_value(out, name, &value, enc, &object_order(&value, v));
        }
    }
    Ok(())
}

/// Appends a `name=value` cookie pair. `Style::Cookie` sends the value as is.
/// Errors: the value does not serialize.
pub fn encode_cookie<T: Serialize + ?Sized>(
    out: &mut Vec<String>,
    name: &str,
    v: &T,
    enc: Enc,
) -> Result<(), ClientError> {
    match scalar_text(v) {
        Some(s) if enc.style == Style::Cookie => {
            reject_separator(&Value::String(s.clone()), COOKIE_RAW, name)?;
            out.push(format!("{name}={s}"));
        }
        Some(s) => out.push(format!("{name}={}", pct(&s, false))),
        None => {
            let value = to_value(v)?;
            if enc.style == Style::Cookie {
                reject_separator(&value, COOKIE_RAW, name)?;
            }
            encode_cookie_value(out, name, &value, enc, &object_order(&value, v));
        }
    }
    Ok(())
}

/// What a raw `cookie` style value cannot hold: the characters that end a cookie or a list in it (RFC 6265).
const COOKIE_RAW: &[char] = &[';', ',', ' '];

/// Fails when a text in `v` holds a character of `separators`: an item or a key that holds the character that its style
/// splits on would arrive as two. A scalar that is not text, and a list or object of such, hold none.
fn reject_separator(v: &Value, separators: &[char], name: &str) -> Result<(), ClientError> {
    let texts: Vec<String> = match v {
        Value::Array(a) => a.iter().map(scalar).collect(),
        Value::Object(o) => o.iter().flat_map(|(k, x)| [k.clone(), scalar(x)]).collect(),
        Value::String(s) => vec![s.clone()],
        Value::Null | Value::Bool(_) | Value::Number(_) => vec![],
    };
    match texts.iter().find(|t| t.contains(separators)) {
        Some(t) => Err(ClientError::Encode(format!(
            "`{name}`: `{t}` holds a character that separates the items of its style, so it would arrive as several"
        ))),
        None => Ok(()),
    }
}

/// Returns [`key_order`] of `v` when its `value` is an object. A null, a scalar or a list has no keys: they cost no
/// round trip through JSON text.
fn object_order<T: Serialize + ?Sized>(value: &Value, v: &T) -> Vec<String> {
    if value.is_object() { key_order(v) } else { Vec::new() }
}

pub(crate) use super::keys::{key_order, members};

/// Returns the entries of an object in `order`. RFC 6570 expands an associative array in
/// its own order. Keys that `order` lacks follow in sorted order.
pub(crate) fn ordered<'a>(o: &'a Map<String, Value>, order: &[String]) -> Vec<(&'a String, &'a Value)> {
    // One lookup per key: scanning `order` for each would make a map with many entries quadratic.
    let rank: std::collections::BTreeMap<&str, usize> =
        order.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let mut out: Vec<_> = o.iter().collect();
    out.sort_by_cached_key(|(k, _)| rank.get(k.as_str()).copied().unwrap_or(usize::MAX));
    out
}

/// Encodes exploded object members as `<prefix>key=value` each. Label is `.k=v.k2=v2`. Matrix is `;k=v;k2=v2`.
fn pairs(prefix: &str, members: Vec<(&String, &Value)>, e: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    for (k, x) in members {
        crate::text::push_all(&mut out, &[prefix, &e(k), "=", &e(&scalar(x))]);
    }
    out
}

fn encode_path_value(name: &str, v: &Value, enc: Enc, order: &[String]) -> String {
    let Enc { style, explode, allow_reserved } = enc;
    let e = |s: &str| pct_path(s, allow_reserved);
    match (style, v) {
        // An undefined value expands to nothing with no prefix (3.2.1 Style
        // Examples after RFC 6570 section 2.3). It is not `.` or `;name=`.
        (Style::Label | Style::Matrix, Value::Null) => String::new(),
        (Style::Label, Value::Array(a)) => {
            let sep = if explode { "." } else { "," };
            format!(".{}", a.iter().map(|x| e(&scalar(x))).collect::<Vec<_>>().join(sep))
        }
        (Style::Label, Value::Object(o)) => {
            if explode {
                pairs(".", ordered(o, order), &e)
            } else {
                format!(
                    ".{}",
                    ordered(o, order)
                        .into_iter()
                        .flat_map(|(k, x)| key_and_value(&e, k, x))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
        (Style::Label, x) => format!(".{}", e(&scalar(x))),
        // A Swagger 2.0 `collectionFormat` of a path array: its separator, encoded.
        (Style::SpaceDelimited | Style::PipeDelimited | Style::TabDelimited, Value::Array(a)) => {
            a.iter().map(|x| e(&scalar(x))).collect::<Vec<_>>().join(style.delimiter().unwrap_or(","))
        }
        (Style::Matrix, Value::Array(a)) => {
            if explode {
                let mut out = String::new();
                for x in a {
                    crate::text::push_all(&mut out, &[";", name, "=", &e(&scalar(x))]);
                }
                out
            } else {
                format!(";{name}={}", a.iter().map(|x| e(&scalar(x))).collect::<Vec<_>>().join(","))
            }
        }
        (Style::Matrix, Value::Object(o)) => {
            if explode {
                pairs(";", ordered(o, order), &e)
            } else {
                format!(
                    ";{name}={}",
                    ordered(o, order)
                        .into_iter()
                        .flat_map(|(k, x)| key_and_value(&e, k, x))
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
        (Style::Matrix, x) => format!(";{name}={}", e(&scalar(x))),
        (_, x) => simple(x, explode, &e, order),
    }
}

/// Returns `key=value` with each encoded by `e`.
fn assignment(e: &dyn Fn(&str) -> String, key: &str, value: &Value) -> String {
    format!("{}={}", e(key), e(&scalar(value)))
}

/// Returns a key and its value with each encoded by `e`. They join with the other members.
fn key_and_value(e: &dyn Fn(&str) -> String, key: &str, value: &Value) -> [String; 2] {
    [e(key), e(&scalar(value))]
}

fn simple(v: &Value, explode: bool, e: &dyn Fn(&str) -> String, order: &[String]) -> String {
    match v {
        Value::Array(a) => a.iter().map(|x| e(&scalar(x))).collect::<Vec<_>>().join(","),
        Value::Object(o) => {
            if explode {
                ordered(o, order).into_iter().map(|(k, x)| assignment(&e, k, x)).collect::<Vec<_>>().join(",")
            } else {
                ordered(o, order).into_iter().flat_map(|(k, x)| key_and_value(&e, k, x)).collect::<Vec<_>>().join(",")
            }
        }
        x @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)) => e(&scalar(x)),
    }
}

fn encode_header_value(v: &Value, enc: Enc, order: &[String]) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Array(a) if enc.style.delimiter().is_some() => {
            Some(a.iter().map(scalar).collect::<Vec<_>>().join(&enc.style.header_separator().to_string()))
        }
        Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) | Value::Object(_) => {
            Some(simple(v, enc.explode, &|s: &str| s.to_owned(), order))
        }
    }
}

pub(crate) fn encode_query_value(out: &mut Vec<String>, name: &str, v: &Value, enc: Enc, order: &[String]) {
    let Enc { style, explode, allow_reserved } = enc;
    let e = |s: &str| pct(s, allow_reserved);
    let n = pct(name, false);
    match (style, v) {
        (_, Value::Null) => {}
        (Style::DeepObject, Value::Object(_)) => deep(out, &n, v, &e, order),
        (_, Value::Array(a)) => {
            if let Some(sep) = style.delimiter() {
                out.push(format!("{n}={}", a.iter().map(|x| e(&scalar(x))).collect::<Vec<_>>().join(sep)));
            } else if explode {
                out.extend(a.iter().map(|x| format!("{n}={}", e(&scalar(x)))));
            } else {
                out.push(format!("{n}={}", a.iter().map(|x| e(&scalar(x))).collect::<Vec<_>>().join(",")));
            }
        }
        // Delimited objects join keys and values with the delimiter of their arrays: `color=R%20100%20G%20200`
        // (3.2 section 4.12.6). Other exploded objects send a pair per key. The rest join with commas.
        (_, Value::Object(o)) => {
            let pairs = ordered(o, order).into_iter().filter(|(_, x)| !x.is_null());
            let sep = style.delimiter();
            if explode && sep.is_none() {
                out.extend(pairs.map(|(k, x)| assignment(&e, k, x)));
            } else {
                let joined = pairs.flat_map(|(k, x)| key_and_value(&e, k, x)).collect::<Vec<_>>();
                out.push(format!("{n}={}", joined.join(sep.unwrap_or(","))));
            }
        }
        (_, x) => {
            out.push(format!("{n}={}", e(&scalar(x))));
        }
    }
}

fn deep(out: &mut Vec<String>, prefix: &str, v: &Value, e: &dyn Fn(&str) -> String, order: &[String]) {
    match v {
        Value::Object(o) => {
            for (k, x) in ordered(o, order) {
                deep(out, &format!("{prefix}%5B{}%5D", pct(k, false)), x, e, &[]);
            }
        }
        Value::Array(a) => {
            for x in a {
                deep(out, prefix, x, e, &[]);
            }
        }
        Value::Null => {}
        x @ (Value::Bool(_) | Value::Number(_) | Value::String(_)) => {
            out.push(format!("{prefix}={}", e(&scalar(x))));
        }
    }
}

fn encode_cookie_value(out: &mut Vec<String>, name: &str, v: &Value, enc: Enc, order: &[String]) {
    let Enc { style, explode, .. } = enc;
    let raw = style == Style::Cookie;
    let e = |s: &str| if raw { s.to_owned() } else { pct(s, false) };
    match v {
        Value::Null => {}
        Value::Array(a) if explode => {
            out.extend(a.iter().map(|x| format!("{name}={}", e(&scalar(x)))));
        }
        Value::Object(o) if explode => {
            out.extend(ordered(o, order).into_iter().map(|(k, x)| format!("{k}={}", e(&scalar(x)))));
        }
        x @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) | Value::Object(_)) => {
            out.push(format!("{name}={}", simple(x, false, &e, order)));
        }
    }
}

/// Sets the `Cookie` header from `name=value` pairs.
///
/// # Errors
/// A pair contains a line break or another control character.
pub fn set_cookies(req: &mut Request, pairs: Vec<String>) -> Result<(), ClientError> {
    let mut joined = String::new();
    for p in pairs {
        if !joined.is_empty() {
            joined.push_str("; ");
        }
        joined.push_str(&p);
    }
    if joined.is_empty() { Ok(()) } else { add_cookie(req, &joined) }
}

#[cfg(test)]
mod tests {
    #[derive(serde::Serialize)]
    struct Pair {
        b: u8,
        a: u8,
    }

    /// An item that holds the character its style splits on would arrive as several: the encoder refuses it.
    #[test]
    fn an_item_with_its_separator_is_refused() {
        use super::{Enc, Style, encode_cookie, encode_header, encode_query};
        let enc = |style| Enc { style, explode: false, allow_reserved: false };
        let mut out = vec![];
        assert!(encode_query(&mut out, "city", &["New York", "Paris"], enc(Style::SpaceDelimited)).is_err());
        assert!(encode_query(&mut out, "city", &["a|b", "c"], enc(Style::PipeDelimited)).is_err());
        assert!(encode_header(&["a,b", "c"], Enc::SIMPLE).is_err());
        assert!(encode_cookie(&mut out, "role", "x; admin=true", enc(Style::Cookie)).is_err());
        assert!(encode_cookie(&mut out, "role", &["a b"], enc(Style::Cookie)).is_err());
        assert!(out.is_empty(), "{out:?}");
        encode_query(&mut out, "city", &["Paris", "Rome"], enc(Style::SpaceDelimited)).unwrap();
        encode_cookie(&mut out, "role", "admin", enc(Style::Cookie)).unwrap();
        encode_cookie(&mut out, "name", "a b", enc(Style::Form)).unwrap();
        assert!(encode_header(&"a,b", Enc::SIMPLE).is_ok(), "a scalar header may hold a comma");
        assert_eq!(out, ["city=Paris%20Rome", "role=admin", "name=a%20b"]);
    }

    #[test]
    fn keys_keep_the_order_of_their_fields() {
        assert_eq!(super::key_order(&Pair { b: 1, a: 2 }), ["b", "a"]);
        assert_eq!(super::key_order(&5), Vec::<String>::new());
    }

    /// A value that JSON cannot write: refuses to serialize.
    struct Unwritable;

    impl serde::Serialize for Unwritable {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("not written"))
        }
    }

    #[derive(serde::Serialize)]
    struct Holder {
        b: Unwritable,
        #[serde(flatten)]
        rest: std::collections::BTreeMap<String, u8>,
    }

    /// The names come without writing the values: a value that does not serialize still gives its name, and a
    /// flattened map gives its own after the fields.
    #[test]
    fn keys_come_without_their_values() {
        let holder = Holder { b: Unwritable, rest: [("a".to_owned(), 1)].into() };
        assert_eq!(super::key_order(&holder), ["b", "a"]);
    }
}
