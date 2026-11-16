//! `multipart/form-data` bodies and reading the parts of any `multipart/*` body.

use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::binary::{raw_mark, raw_part, raw_parts};
use crate::json::push_multi;
use crate::json::to_value;
#[cfg(feature = "xml")]
use crate::media::is_xml_media;
use crate::params::{key_order, ordered};
use crate::text::{decode_err, push_all};
use crate::{Coerce, Error, base64, unbase64_mime};

pub(super) const BOUNDARY: &str = concat!(env!("CARGO_PKG_NAME"), "-boundary-7MA4YWxkTrZu0gW");

/// Multipart bodies nested deeper than this are read as one part's bytes.
const MAX_NESTING: usize = 4;

/// The media type that each multipart part goes out as when the description lists several (`encoding: {icon:
/// {contentType: "image/png, image/jpeg"}}`).
///
/// A part not named here goes out as the first listed type.
///
/// 3.2 Encoding Object: "implementations MUST provide a mechanism for applications to
/// indicate which media type is intended".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartTypes(Vec<(String, String)>);

impl<K: Into<String>, V: Into<String>, const N: usize> From<[(K, V); N]> for PartTypes {
    fn from(pairs: [(K, V); N]) -> Self {
        Self(pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }
}

impl PartTypes {
    /// Creates a choice that sends every part as its first listed type.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Sends part `part` as `media`.
    #[must_use]
    pub fn with(mut self, part: impl Into<String>, media: impl Into<String>) -> Self {
        self.0.push((part.into(), media.into()));
        self
    }

    /// Returns one media type per part. That is the chosen one or else the first of the comma-separated list in
    /// `declared`. Errors: a chosen type that the part does not list or a part that the body does not declare. Ranges
    /// such as `image/*` allow any of their types.
    pub fn choose<'a>(&'a self, declared: &[(&'a str, &'a str)]) -> Result<Vec<(&'a str, &'a str)>, Error> {
        if let Some((part, _)) = self.0.iter().find(|(p, _)| !declared.iter().any(|(d, _)| d == p)) {
            return Err(Error::Encode(format!("part `{part}` has no `encoding` with a list of media types")));
        }
        declared
            .iter()
            .map(|&(part, list)| {
                let mut listed = list.split(',').map(str::trim).filter(|m| !m.is_empty());
                let first = listed.clone().next().unwrap_or(list);
                match self.0.iter().rev().find(|(p, _)| p == part) {
                    None => Ok((part, first)),
                    Some((_, m)) if listed.any(|d| crate::media::media_matches(m, d)) => Ok((part, m.as_str())),
                    Some((_, m)) => Err(Error::Encode(format!("part `{part}` cannot be `{m}`: it lists `{list}`"))),
                }
            })
            .collect()
    }
}

/// Returns `multipart/form-data`: one part per property (arrays: one per item).
/// Strings are sent as text. Everything else is sent as JSON. Returns (content type, body).
pub fn multipart_encode<T: Serialize>(v: &T, types: &[(&str, &str)]) -> Result<(String, Vec<u8>), Error> {
    multipart_encode_with(v, types, &[])
}

/// Returns [`multipart_encode`] with extra (property, header, value) part headers.
/// Errors: the value does not serialize.
pub fn multipart_encode_with<T: Serialize>(
    v: &T,
    types: &[(&str, &str)],
    headers: &[(&str, &str, String)],
) -> Result<(String, Vec<u8>), Error> {
    raw_parts(|| multipart_encode_in(v, types, headers))
}

/// Returns [`multipart_encode_with`] inside its conversion.
fn multipart_encode_in<T: Serialize>(
    v: &T,
    types: &[(&str, &str)],
    headers: &[(&str, &str, String)],
) -> Result<(String, Vec<u8>), Error> {
    let value = to_value(v)?;
    let mut body = Vec::new();
    if let Value::Object(o) = &value {
        let order = raw_parts(|| key_order(v));
        for (k, x) in ordered(o, &order) {
            let mut extra = String::new();
            for (_, h, v) in headers.iter().filter(|(p, ..)| p == k) {
                push_all(&mut extra, &[h, ": ", &v.replace(['\r', '\n'], " "), "\r\n"]);
            }
            match x {
                Value::Null => {}
                Value::Array(a) if !declared(types, k).is_some_and(is_multipart) => {
                    for e in a {
                        named_part(&mut body, Nest::TOP, k, k, e, types, &extra);
                    }
                }
                other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) | Value::Object(_)) => {
                    named_part(&mut body, Nest::TOP, k, k, other, types, &extra);
                }
            }
        }
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={BOUNDARY}"), body))
}

/// Returns the content type that `encoding` declares for a part path (`meta`, `meta/inner`
/// or `files/0`). The fallback is the `*` entry of the path (`files/*`).
fn declared<'t>(types: &[(&'t str, &'t str)], path: &str) -> Option<&'t str> {
    let find = |p: &str| types.iter().find(|(k, _)| *k == p).map(|(_, ct)| *ct);
    find(path).or_else(|| path.rsplit_once('/').and_then(|(parent, _)| find(&format!("{parent}/*"))))
}

fn is_multipart(ct: &str) -> bool {
    ct.trim_start().to_ascii_lowercase().starts_with("multipart/")
}

/// Where a part sits: the boundary of its body and how deeply it nests.
#[derive(Clone, Copy)]
struct Nest<'b> {
    boundary: &'b str,
    depth: usize,
}

impl Nest<'_> {
    const TOP: Nest<'static> = Nest { boundary: BOUNDARY, depth: 0 };
}

/// Appends one `form-data` part named `name` (encodings at `path`).
fn named_part(
    body: &mut Vec<u8>,
    nest: Nest<'_>,
    path: &str,
    name: &str,
    x: &Value,
    types: &[(&str, &str)],
    extra: &str,
) {
    let (ct, data, file) = part_value(path, x, types, nest.depth);
    // `Content-Transfer-Encoding: base64` among the part's headers (a `contentEncoding: base64` field): the body as
    // base64.
    let base64_part = extra.lines().any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("content-transfer-encoding") && v.trim().eq_ignore_ascii_case("base64")
        })
    });
    let data = if base64_part { base64(&data).into_bytes() } else { data };
    let boundary = nest.boundary;
    let quote = |s: &str| s.replace('"', "%22");
    let disposition = match file {
        // A file without a name of its own is named after its field.
        PartKind::File(filename) => {
            format!("name=\"{}\"; filename=\"{}\"", quote(name), quote(filename.as_deref().unwrap_or(name)))
        }
        PartKind::Field => format!("name=\"{}\"", quote(name)),
    };
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; {disposition}\r\nContent-Type: {ct}\r\n{extra}\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(&data);
    body.extend_from_slice(b"\r\n");
}

/// What a part carries. That is the value of a field or a file with its own name if it has one.
enum PartKind {
    Field,
    File(Option<String>),
}

/// Returns the content type, bytes and [`PartKind`] of a part. A part declared
/// `multipart/*` that holds an object or a list nests its own parts. They are
/// named (`encoding`) or positional (`prefixEncoding` and `itemEncoding`).
fn part_value(path: &str, x: &Value, types: &[(&str, &str)], depth: usize) -> (String, Vec<u8>, PartKind) {
    let declared = declared(types, path);
    if let Some(d) = declared.filter(|d| is_multipart(d) && depth < MAX_NESTING)
        && matches!(x, Value::Object(_) | Value::Array(_))
    {
        // Never containing an enclosing boundary: splitting the outer body
        // must not cut at a nested delimiter.
        let boundary = format!("{}-part{}-Qz3vK9wLmB", env!("CARGO_PKG_NAME"), depth + 1);
        let mut body = Vec::new();
        match x {
            Value::Object(o) => {
                for (k, v) in o.iter().filter(|(_, v)| !v.is_null()) {
                    let nest = Nest { boundary: &boundary, depth: depth + 1 };
                    named_part(&mut body, nest, &format!("{path}/{k}"), k, v, types, "");
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter().enumerate() {
                    let (ct, data, _) = part_value(&format!("{path}/{i}"), v, types, depth + 1);
                    body.extend_from_slice(format!("--{boundary}\r\nContent-Type: {ct}\r\n\r\n").as_bytes());
                    body.extend_from_slice(&data);
                    body.extend_from_slice(b"\r\n");
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let base = d.split(';').next().unwrap_or(d).trim();
        return (format!("{base}; boundary={boundary}"), body, PartKind::Field);
    }
    // An object or a list declared XML is an XML document. Its root has the name of the part.
    #[cfg(feature = "xml")]
    if let Some(d) = declared.filter(|d| is_xml_media(d))
        && matches!(x, Value::Object(_) | Value::Array(_))
    {
        let root = path.rsplit('/').find(|s| !s.is_empty() && !s.bytes().all(|b| b.is_ascii_digit())).unwrap_or("item");
        return (d.to_owned(), crate::xml_tree::xml_document(root, x), PartKind::Field);
    }
    let (ct, data, file) = match x {
        Value::String(s) => match raw_part(s) {
            Some(raw) => {
                let ct = raw.content_type.unwrap_or_else(|| "application/octet-stream".to_owned());
                (ct, Vec::from(raw.bytes), PartKind::File(raw.filename))
            }
            None => ("text/plain".to_owned(), s.clone().into_bytes(), PartKind::Field),
        },
        // 3.2 Encoding Object defaults: `number`, `integer` and `boolean` are
        // `text/plain`. `object` and `array` are `application/json`.
        x @ (Value::Bool(_) | Value::Number(_)) => {
            ("text/plain".to_owned(), x.to_string().into_bytes(), PartKind::Field)
        }
        other @ (Value::Null | Value::Array(_) | Value::Object(_)) => {
            ("application/json".to_owned(), other.to_string().into_bytes(), PartKind::Field)
        }
    };
    // `encoding.<property>.contentType` wins over the type guessed from the value.
    (declared.map_or(ct, str::to_owned), data, file)
}

/// Decodes a `multipart/form-data` body; file parts reach [`Binary`](crate::Binary) fields unchanged.
pub fn multipart_decode<T: DeserializeOwned>(status: u16, content_type: &str, body: &[u8]) -> Result<T, Error> {
    raw_parts(|| multipart_decode_in(status, content_type, body))
}

/// Returns [`multipart_decode`] inside its conversion.
fn multipart_decode_in<T: DeserializeOwned>(status: u16, content_type: &str, body: &[u8]) -> Result<T, Error> {
    let boundary =
        boundary_of(content_type).ok_or_else(|| decode_err(status, body, "multipart body without boundary"))?;
    let value = parts_value(body, &boundary, true, 0).map_err(|e| decode_err(status, body, e))?;
    raw_parts(|| T::deserialize(Coerce(value))).map_err(|e| decode_err(status, body, e))
}

/// Returns the `key` parameter of a header value such as `boundary=` or `name=`. It is still quoted.
fn parameter<'v>(value: &'v str, key: &str) -> Option<&'v str> {
    value.split(';').find_map(|p| p.trim().strip_prefix(key))
}

/// Returns the `boundary` parameter of a `multipart/*` content type.
pub(crate) fn boundary_of(content_type: &str) -> Option<String> {
    parameter(content_type, "boundary=").map(|b| b.trim_matches('"').to_owned())
}

/// One part of a multipart body.
pub(super) struct Part {
    /// Its headers with lowercase names.
    headers: Vec<(String, String)>,
    /// The `name` of its `Content-Disposition`.
    name: Option<String>,
    /// The `filename` of its `Content-Disposition`.
    filename: Option<String>,
    pub(super) data: Bytes,
}

impl Part {
    /// Returns the value of header `key` if it is text.
    pub(super) fn header(&self, key: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

/// Returns every part of a multipart body. `multer` reads `form-data` parts. Other `multipart/*` bodies are split here.
/// Their parts may have no headers at all. Then they are `text/plain` (RFC 2046 section 5.1.1). `multer` would read the
/// body of such a part as its headers. Errors: the body is not `form-data` with this boundary.
pub(super) fn parts(body: &[u8], boundary: &str, form: bool) -> Result<Vec<Part>, String> {
    let mut parts =
        if form { form_parts(body, boundary).map_err(|e| e.to_string())? } else { positional_parts(body, boundary) };
    for part in &mut parts {
        transfer_decode(part)?;
    }
    Ok(parts)
}

/// Returns a part sent with `Content-Transfer-Encoding: base64` or `quoted-printable` holds the decoded bytes. In 3.2
/// `contentEncoding` on a multipart field means that header. `7bit`, `8bit` and `binary` are as sent.
fn transfer_decode(part: &mut Part) -> Result<(), String> {
    let Some(cte) = part.header("content-transfer-encoding").map(|v| v.trim().to_ascii_lowercase()) else {
        return Ok(());
    };
    part.data = match cte.as_str() {
        "base64" => unbase64_mime(&part.data).ok_or("a base64 part is not base64")?.into(),
        "quoted-printable" => quoted_printable(&part.data).into(),
        _ => return Ok(()),
    };
    Ok(())
}

/// Returns RFC 2045 section 6.7: `=XX` is a byte. `=` at a line end joins the lines.
fn quoted_printable(data: &[u8]) -> Vec<u8> {
    let hex = crate::escape::hex_digit;
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while let Some(&b) = data.get(i) {
        if b != b'=' {
            out.push(b);
            i += 1;
            continue;
        }
        match (data.get(i + 1).copied(), data.get(i + 2).copied()) {
            (Some(b'\r'), Some(b'\n')) => i += crate::escape::PCT_ESCAPE,
            (Some(b'\n'), _) => i += 2,
            (Some(h), Some(l)) if hex(h).is_some() && hex(l).is_some() => {
                out.push(u8::try_from(hex(h).unwrap_or(0) * crate::escape::HEX + hex(l).unwrap_or(0)).unwrap_or(b'='));
                i += crate::escape::PCT_ESCAPE;
            }
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Returns the parts between each `--boundary` line and the next up to the closing `--boundary--`.
fn positional_parts(body: &[u8], boundary: &str) -> Vec<Part> {
    let delim = ["--", boundary].concat();
    // A delimiter starts the body or a line (RFC 2046 section 5.1.1).
    let starts: Vec<usize> = memchr::memmem::find_iter(body, delim.as_bytes())
        .filter(|at| *at == 0 || body.get(at - 2..*at) == Some(b"\r\n"))
        .collect();
    let mut out = vec![];
    for (at, next) in starts.iter().zip(starts.iter().skip(1)) {
        let chunk = body.get(at + delim.len()..*next).unwrap_or_default();
        if chunk.starts_with(b"--") {
            break;
        }
        // The line ending after the boundary. Then the one that belongs to the next delimiter.
        let chunk = chunk.strip_prefix(b"\r\n").unwrap_or(chunk);
        let chunk = chunk.strip_suffix(b"\r\n").unwrap_or(chunk);
        let (head, data) = match chunk.strip_prefix(b"\r\n") {
            Some(data) => (&b""[..], data),
            None => match memchr::memmem::find(chunk, b"\r\n\r\n") {
                Some(end) => {
                    (chunk.get(..end).unwrap_or_default(), chunk.get(end + "\r\n\r\n".len()..).unwrap_or_default())
                }
                None => (chunk, &b""[..]),
            },
        };
        let headers: Vec<(String, String)> = String::from_utf8_lossy(head)
            .lines()
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        let disposition = headers.iter().find(|(k, _)| k == "content-disposition").map(|(_, v)| v.as_str());
        let param =
            |key: &str| disposition.and_then(|d| parameter(d, key)).map(|n| n.trim_matches('"').replace("%22", "\""));
        let (name, filename) = (param("name="), param("filename="));
        out.push(Part { headers, name, filename, data: Bytes::copy_from_slice(data) });
    }
    out
}

/// Returns the parts of a `multipart/form-data` body. `multer` reads them from a body already in memory.
fn form_parts(body: &[u8], boundary: &str) -> Result<Vec<Part>, multer::Error> {
    let chunk = futures_util::stream::iter([Ok::<_, std::convert::Infallible>(Bytes::copy_from_slice(body))]);
    let mut form = multer::Multipart::new(chunk, boundary);
    // The whole body is one ready chunk: nothing here waits.
    crate::block_on(async {
        let mut out = vec![];
        while let Some(field) = form.next_field().await? {
            // The encoder escapes a quote in a name as `%22`.
            let unquote = |n: &str| n.replace("%22", "\"");
            let name = field.name().map(unquote);
            let filename = field.file_name().map(unquote);
            let headers = field
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
                .collect();
            out.push(Part { headers, name, filename, data: field.bytes().await? });
        }
        Ok(out)
    })
}

/// Returns the parts of a multipart body. That is an object of the named ones or else a list.
/// A `multipart/*` part is decoded in the same way one level down.
fn parts_value(body: &[u8], boundary: &str, form: bool, depth: usize) -> Result<Value, String> {
    let mut named = Map::new();
    let mut list = vec![];
    for part in parts(body, boundary, form)? {
        let raw_type = part.header("content-type").unwrap_or("");
        let part_type = raw_type.to_ascii_lowercase();
        let nested = is_multipart(&part_type).then(|| boundary_of(raw_type)).flatten().filter(|_| depth < MAX_NESTING);
        let value = if let Some(b) = nested {
            parts_value(&part.data, &b, part_type.starts_with("multipart/form-data"), depth + 1)?
        } else {
            let textual = part_type.is_empty() || part_type.starts_with("text/") || part_type.contains("json");
            // A file or a binary part is never text: no UTF-8 pass over its bytes.
            let text = (textual && part.filename.is_none()).then(|| crate::text::utf8(&part.data)).flatten();
            match text {
                Some(text) => Value::String(text.to_owned()),
                None => Value::String(raw_mark(
                    &part.data,
                    part.filename.as_deref(),
                    Some(raw_type).filter(|t| !t.is_empty()),
                )),
            }
        };
        match part.name {
            Some(name) => push_multi(&mut named, name, value),
            None => list.push(value),
        }
    }
    Ok(if named.is_empty() && !list.is_empty() { Value::Array(list) } else { Value::Object(named) })
}

/// Returns the `header` of the first `multipart/form-data` part named `name`.
pub fn part_header(content_type: &str, body: &[u8], name: &str, header: &str) -> Option<String> {
    parts(body, &boundary_of(content_type)?, true)
        .ok()?
        .into_iter()
        .find(|p| p.name.as_deref() == Some(name))
        .and_then(|p| p.header(header).map(str::to_owned))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_types_choose_a_listed_media_type_or_the_first() {
        let declared = [("icon", "image/png, image/jpeg"), ("doc", "application/*")];
        let none = PartTypes::default();
        assert_eq!(none.choose(&declared).unwrap(), [("icon", "image/png"), ("doc", "application/*")]);
        let chosen = PartTypes::from([("icon", "image/jpeg")]).with("doc", "application/pdf");
        assert_eq!(chosen.choose(&declared).unwrap(), [("icon", "image/jpeg"), ("doc", "application/pdf")]);
        let unlisted = PartTypes::from([("icon", "image/gif")]);
        assert!(unlisted.choose(&declared).unwrap_err().to_string().contains("cannot be `image/gif`"));
        let unknown = PartTypes::from([("logo", "image/png")]);
        assert!(unknown.choose(&declared).unwrap_err().to_string().contains("part `logo`"));
    }
}
