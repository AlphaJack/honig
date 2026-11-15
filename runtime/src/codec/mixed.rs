//! Positional `multipart/*` bodies (`multipart/mixed` and the like): one part per array item.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::multipart::{BOUNDARY, boundary_of, parts};
use crate::binary::{raw_mark, raw_part, raw_parts};
use crate::json::to_value;
use crate::media::json_type;
use crate::text::{decode_err, lossy, utf8};
use crate::{Coerce, Error};

/// Encodes positional multipart: one part per array item.
///
/// `types` maps a position or `"*"` to a content
/// type. `("prefix", n)` marks a trailing list. Returns (content type, body).
/// Errors: the value is not an array or does not serialize.
pub fn mixed_encode<T: Serialize>(v: &T, media: &str, types: &[(&str, &str)]) -> Result<(String, Vec<u8>), Error> {
    raw_parts(|| mixed_encode_in(v, media, types))
}

/// Returns [`mixed_encode`] inside its conversion.
fn mixed_encode_in<T: Serialize>(v: &T, media: &str, types: &[(&str, &str)]) -> Result<(String, Vec<u8>), Error> {
    let value = to_value(v)?;
    let Value::Array(mut items) = value else {
        return Err(Error::Encode(format!("{media} body must be an array")));
    };
    let prefix = types.iter().find(|(k, _)| *k == "prefix").and_then(|(_, n)| n.parse::<usize>().ok());
    if let Some(n) = prefix
        && items.len() == n + 1
        && let Some(Value::Array(rest)) = items.pop()
    {
        items.extend(rest);
    }
    let mut body = Vec::new();
    for (i, x) in items.iter().enumerate() {
        let mut index = std::fmt::NumBuffer::new();
        let index = i.format_into(&mut index);
        let declared = types
            .iter()
            .find(|(k, _)| *k == index)
            .or_else(|| types.iter().find(|(k, _)| *k == "*"))
            .map(|(_, ct)| *ct);
        let (ct, data) = match x {
            Value::String(s) => match raw_part(s).map(|p| Vec::from(p.bytes)) {
                Some(raw) => (declared.unwrap_or("application/octet-stream"), raw),
                None if declared.is_some_and(json_type) => (declared.unwrap_or_default(), x.to_string().into_bytes()),
                None => (declared.filter(|d| d.starts_with("text/")).unwrap_or("text/plain"), s.clone().into_bytes()),
            },
            // Structured values travel as JSON. A non-JSON declared type
            // (a nested multipart) cannot carry them. So JSON wins.
            other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
                (declared.filter(|d| json_type(d)).unwrap_or("application/json"), other.to_string().into_bytes())
            }
        };
        body.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Type: {ct}\r\n\r\n").as_bytes());
        body.extend_from_slice(&data);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    let base = media.split(';').next().unwrap_or(media).trim();
    let base = if base.contains('*') { "multipart/mixed" } else { base };
    Ok((format!("{base}; boundary={BOUNDARY}"), body))
}

/// Decodes a positional multipart body (see [`mixed_encode`]). `prefix` groups the rest into a list.
/// Errors: no boundary or the parts do not fit `T`.
pub fn mixed_decode<T: DeserializeOwned>(
    status: u16,
    content_type: &str,
    body: &[u8],
    prefix: Option<usize>,
) -> Result<T, Error> {
    raw_parts(|| mixed_decode_in(status, content_type, body, prefix))
}

/// Returns [`mixed_decode`] inside its conversion.
fn mixed_decode_in<T: DeserializeOwned>(
    status: u16,
    content_type: &str,
    body: &[u8],
    prefix: Option<usize>,
) -> Result<T, Error> {
    let boundary =
        boundary_of(content_type).ok_or_else(|| decode_err(status, body, "multipart body without boundary"))?;
    let mut items = vec![];
    for part in parts(body, &boundary, false).map_err(|e| decode_err(status, body, e))? {
        let ct = part.header("content-type").map_or_else(|| "text/plain".to_owned(), |v| v.trim().to_ascii_lowercase());
        let data = &part.data[..];
        let value = if json_type(&ct) {
            serde_json::from_slice(data).unwrap_or_else(|_| Value::String(lossy(data).into_owned()))
        } else {
            // A binary part is never text: no UTF-8 pass over its bytes.
            match ct.starts_with("text/").then(|| utf8(data)).flatten() {
                Some(text) => Value::String(text.to_owned()),
                None => Value::String(raw_mark(&part.data, None, Some(ct.as_str()))),
            }
        };
        items.push(value);
    }
    if let Some(n) = prefix {
        let rest = items.split_off(n.min(items.len()));
        items.push(Value::Array(rest));
    }
    raw_parts(|| T::deserialize(Coerce(Value::Array(items)))).map_err(|e| decode_err(status, body, e))
}
