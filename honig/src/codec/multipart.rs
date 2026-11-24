//! `multipart/form-data` bodies and parsing the parts of any `multipart/*` body.

use std::fmt::Write as _;

use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::base64::base64;
use crate::binary::{raw_mark, raw_part, raw_parts};
use crate::json::push_multi;
use crate::json::to_value;
#[cfg(feature = "xml")]
use crate::media::is_xml_media;
use crate::params::{key_order, ordered};
use crate::text::{decode_err, push_all};
use crate::{ClientError, base64::unbase64_mime, coerce::Coerce};

const BOUNDARY: &str = concat!(env!("CARGO_PKG_NAME"), "-boundary-7MA4YWxkTrZu0gW");

/// Multipart bodies nested deeper than this are kept as one part's bytes.
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
    pub fn choose<'a>(&'a self, declared: &[(&'a str, &'a str)]) -> Result<Vec<(&'a str, &'a str)>, ClientError> {
        if let Some((part, _)) = self.0.iter().find(|(p, _)| !declared.iter().any(|(d, _)| d == p)) {
            return Err(ClientError::Encode(format!("part `{part}` has no `encoding` with a list of media types")));
        }
        declared
            .iter()
            .map(|&(part, list)| {
                let mut listed = list.split(',').map(str::trim).filter(|m| !m.is_empty());
                let first = listed.clone().next().unwrap_or(list);
                match self.0.iter().rev().find(|(p, _)| p == part) {
                    None => Ok((part, first)),
                    Some((_, m)) if listed.any(|d| crate::media::media_matches(m, d)) => Ok((part, m.as_str())),
                    Some((_, m)) => {
                        Err(ClientError::Encode(format!("part `{part}` cannot be `{m}`: it lists `{list}`")))
                    }
                }
            })
            .collect()
    }
}

/// Returns `multipart/form-data`: one part per property (arrays: one per item).
/// Strings are sent as text. Everything else is sent as JSON. Returns (content type, body).
pub fn multipart_encode<T: Serialize>(v: &T, types: &[(&str, &str)]) -> Result<(String, Vec<u8>), ClientError> {
    multipart_encode_with(v, types, &[])
}

/// Returns [`multipart_encode`] with extra (property, header, value) part headers.
/// Errors: the value does not serialize.
pub fn multipart_encode_with<T: Serialize>(
    v: &T,
    types: &[(&str, &str)],
    headers: &[(&str, &str, String)],
) -> Result<(String, Vec<u8>), ClientError> {
    raw_parts(|| multipart_encode_in(v, types, headers))
}

/// Returns [`multipart_encode_with`] inside its conversion.
fn multipart_encode_in<T: Serialize>(
    v: &T,
    types: &[(&str, &str)],
    headers: &[(&str, &str, String)],
) -> Result<(String, Vec<u8>), ClientError> {
    let value = to_value(v)?;
    let mut parts = Vec::new();
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
                        parts.push(named_part(0, k, k, e, types, &extra)?);
                    }
                }
                other @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) | Value::Object(_)) => {
                    parts.push(named_part(0, k, k, other, types, &extra)?);
                }
            }
        }
    }
    Ok(assemble("multipart/form-data", &parts))
}

/// Returns the content type and the body of a `media` multipart body of `parts`, each its header lines and its bytes.
/// The boundary is [`BOUNDARY`], a dash and a counter that no part contains. Each place where a part contains the
/// boundary and its dash rules out one counter of the same width, so one of `places + 1` counters is free: one pass.
pub(super) fn assemble(media: &str, parts: &[(String, Vec<u8>)]) -> (String, Vec<u8>) {
    let mut boundary = [BOUNDARY, "-"].concat();
    let finder = memchr::memmem::Finder::new(boundary.as_bytes());
    let after: Vec<&[u8]> = parts
        .iter()
        .flat_map(|(head, data)| [head.as_bytes(), data.as_slice()])
        .flat_map(|bytes| finder.find_iter(bytes).map(|at| bytes.get(at + finder.needle().len()..).unwrap_or_default()))
        .collect();
    let width = after.len().to_string().len();
    let taken: std::collections::BTreeSet<&[u8]> = after.iter().filter_map(|a| a.get(..width)).collect();
    let stem = boundary.len();
    for n in 0..=after.len() {
        boundary.truncate(stem);
        write!(boundary, "{n:0width$}").ok();
        if !boundary.as_bytes().get(stem..).is_some_and(|c| taken.contains(c)) {
            break;
        }
    }
    let delimiter = |body: &mut Vec<u8>| {
        body.extend_from_slice(b"--");
        body.extend_from_slice(boundary.as_bytes());
    };
    // `--`, the boundary and three line ends per part, then `--`, the boundary and `--` with a line end.
    let framing = boundary.len() + "--\r\n\r\n\r\n".len();
    let size = parts.iter().map(|(head, data)| framing + head.len() + data.len()).sum::<usize>();
    let mut body = Vec::with_capacity(size + boundary.len() + "----\r\n".len());
    for (head, data) in parts {
        delimiter(&mut body);
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(head.as_bytes());
        body.extend_from_slice(b"\r\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n");
    }
    delimiter(&mut body);
    body.extend_from_slice(b"--\r\n");
    (format!("{media}; boundary={boundary}"), body)
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

/// Returns the header lines and the bytes of one `form-data` part named `name` (encodings at `path`) `depth` bodies
/// deep.
fn named_part(
    depth: usize,
    path: &str,
    name: &str,
    x: &Value,
    types: &[(&str, &str)],
    extra: &str,
) -> Result<(String, Vec<u8>), ClientError> {
    let (ct, data, file) = part_value(path, x, types, depth)?;
    // `Content-Transfer-Encoding: base64` among the part's headers (a `contentEncoding: base64` field): the body as
    // base64.
    let base64_part = extra.lines().any(|l| {
        l.split_once(':').is_some_and(|(k, v)| {
            k.trim().eq_ignore_ascii_case("content-transfer-encoding") && v.trim().eq_ignore_ascii_case("base64")
        })
    });
    let data = if base64_part { base64(&data).into_bytes() } else { data };
    let disposition = match file {
        // A file without a name of its own is named after its field.
        PartKind::File(filename) => {
            format!("name=\"{}\"; filename=\"{}\"", quote(name), quote(filename.as_deref().unwrap_or(name)))
        }
        PartKind::Field => format!("name=\"{}\"", quote(name)),
    };
    Ok((
        format!("Content-Disposition: form-data; {disposition}\r\nContent-Type: {}\r\n{extra}", media_type(&ct)?),
        data,
    ))
}

/// Returns `ct` for the `Content-Type` line of a part. Errors: it has a control character, which would end the line.
pub(super) fn media_type(ct: &str) -> Result<&str, ClientError> {
    if ct.chars().any(char::is_control) {
        return Err(ClientError::Encode(format!(
            "a part's media type `{}` has a control character",
            ct.escape_debug()
        )));
    }
    Ok(ct)
}

/// Returns `name` for a `Content-Disposition` parameter, escaped as an HTML form escapes it: a quote would end the
/// parameter and a line break would start a header of its own.
fn quote(name: &str) -> std::borrow::Cow<'_, str> {
    if name.contains(['"', '\r', '\n']) {
        name.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A").into()
    } else {
        name.into()
    }
}

/// Returns a `name` or `filename` that the encoder escaped as it was given.
fn unquote(n: &str) -> String {
    if n.contains('%') { n.replace("%22", "\"").replace("%0D", "\r").replace("%0A", "\n") } else { n.to_owned() }
}

/// What a part carries. That is the value of a field or a file with its own name if it has one.
enum PartKind {
    Field,
    File(Option<String>),
}

/// Returns the content type, bytes and [`PartKind`] of a part. A part declared
/// `multipart/*` that contains an object or a list nests its own parts. They are
/// named (`encoding`) or positional (`prefixEncoding` and `itemEncoding`).
fn part_value(
    path: &str,
    x: &Value,
    types: &[(&str, &str)],
    depth: usize,
) -> Result<(String, Vec<u8>, PartKind), ClientError> {
    let declared = declared(types, path);
    if let Some(d) = declared.filter(|d| is_multipart(d) && depth < MAX_NESTING)
        && matches!(x, Value::Object(_) | Value::Array(_))
    {
        let parts: Vec<(String, Vec<u8>)> = match x {
            Value::Object(o) => o
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| named_part(depth + 1, &format!("{path}/{k}"), k, v, types, ""))
                .collect::<Result<_, _>>()?,
            Value::Array(a) => a
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let (ct, data, _) = part_value(&format!("{path}/{i}"), v, types, depth + 1)?;
                    Ok((format!("Content-Type: {}\r\n", media_type(&ct)?), data))
                })
                .collect::<Result<_, ClientError>>()?,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => vec![],
        };
        // The enclosing body picks a boundary that this one does not contain.
        let (ct, body) = assemble(d.split(';').next().unwrap_or(d).trim(), &parts);
        return Ok((ct, body, PartKind::Field));
    }
    // An object or a list declared XML is an XML document. Its root has the name of the part.
    #[cfg(feature = "xml")]
    if let Some(d) = declared.filter(|d| is_xml_media(d))
        && matches!(x, Value::Object(_) | Value::Array(_))
    {
        let root = path.rsplit('/').find(|s| !s.is_empty() && !s.bytes().all(|b| b.is_ascii_digit())).unwrap_or("item");
        return Ok((d.to_owned(), crate::xml_tree::xml_document(root, x)?, PartKind::Field));
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
    // `encoding.<property>.contentType` wins over the default of the value's kind.
    Ok((declared.map_or(ct, str::to_owned), data, file))
}

/// Decodes a `multipart/form-data` body; file parts reach [`Binary`](crate::Binary) fields unchanged.
pub fn multipart_decode<T: DeserializeOwned>(
    status: Option<u16>,
    content_type: &str,
    body: &[u8],
) -> Result<T, ClientError> {
    raw_parts(|| multipart_decode_in(status, content_type, body))
}

/// Returns [`multipart_decode`] inside its conversion.
fn multipart_decode_in<T: DeserializeOwned>(
    status: Option<u16>,
    content_type: &str,
    body: &[u8],
) -> Result<T, ClientError> {
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

/// Returns every part of a multipart body. `multer` parses `form-data` parts. Other `multipart/*` bodies are split
/// here. Their parts may have no headers at all. Then they are `text/plain` (RFC 2046 section 5.1.1). `multer` would
/// parse the body of such a part as its headers. Errors: the body is not `form-data` with this boundary.
pub(super) fn parts(body: &[u8], boundary: &str, form: bool) -> Result<Vec<Part>, String> {
    let mut parts =
        if form { form_parts(body, boundary).map_err(|e| e.to_string())? } else { positional_parts(body, boundary)? };
    for part in &mut parts {
        transfer_decode(part)?;
    }
    Ok(parts)
}

/// Returns the bytes of a part sent with `Content-Transfer-Encoding: base64` or `quoted-printable`, decoded. In 3.2
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
fn positional_parts(body: &[u8], boundary: &str) -> Result<Vec<Part>, String> {
    let delim = ["--", boundary].concat();
    // A delimiter starts the body or a line (RFC 2046 section 5.1.1).
    let starts: Vec<usize> = memchr::memmem::find_iter(body, delim.as_bytes())
        .filter(|at| *at == 0 || at.checked_sub(2).is_some_and(|start| body.get(start..*at) == Some(b"\r\n")))
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
        let headers: Vec<(String, String)> = std::str::from_utf8(head)
            .map_err(|e| format!("the headers of a part are not UTF-8: {e}"))?
            .lines()
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        let disposition = headers.iter().find(|(k, _)| k == "content-disposition").map(|(_, v)| v.as_str());
        let param = |key: &str| disposition.and_then(|d| parameter(d, key)).map(|n| unquote(n.trim_matches('"')));
        let (name, filename) = (param("name="), param("filename="));
        out.push(Part { headers, name, filename, data: Bytes::copy_from_slice(data) });
    }
    Ok(out)
}

/// Returns the parts of a `multipart/form-data` body. `multer` parses them from a body already in memory.
fn form_parts(body: &[u8], boundary: &str) -> Result<Vec<Part>, multer::Error> {
    let chunk = futures_util::stream::iter([Ok::<_, std::convert::Infallible>(Bytes::copy_from_slice(body))]);
    let mut form = multer::Multipart::new(chunk, boundary);
    // The whole body is one ready chunk: nothing here waits.
    crate::block_on(async {
        let mut out = vec![];
        while let Some(field) = form.next_field().await? {
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

    /// A part declared `multipart/*` nests its own parts, up to `MAX_NESTING` levels. A part of another type does not.
    /// A value that contains a delimiter of the body: the constant, or the constant and the counter of a body that
    /// does not contain it.
    fn forged(boundary: &str) -> String {
        format!("x\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"admin\"\r\n\r\ntrue")
    }

    #[test]
    fn a_line_break_in_a_file_name_or_media_type_adds_no_header() {
        #[derive(serde::Serialize, serde::Deserialize)]
        struct Upload {
            file: crate::Binary,
        }
        let name = "a.txt\"\r\nX-Injected: yes";
        let file = |content_type: Option<&str>| crate::Binary {
            bytes: b"x".to_vec().into(),
            filename: Some(name.to_owned()),
            content_type: content_type.map(str::to_owned),
        };
        let (ct, body) = multipart_encode(&Upload { file: file(None) }, &[]).unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("\nX-Injected"), "{}", String::from_utf8_lossy(&body));
        let back: Upload = multipart_decode(None, &ct, &body).unwrap();
        assert_eq!(back.file.filename.as_deref(), Some(name));
        let refused = multipart_encode(&Upload { file: file(Some("text/plain\r\nX-Injected: yes")) }, &[]);
        assert!(matches!(refused, Err(ClientError::Encode(_))), "{refused:?}");
    }

    #[test]
    fn a_value_that_contains_a_delimiter_stays_one_part() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct Form {
            comment: String,
            file: crate::Binary,
        }
        for b in [BOUNDARY.to_owned(), format!("{BOUNDARY}-0")] {
            let file = crate::Binary { bytes: forged(&b).into_bytes().into(), filename: None, content_type: None };
            let form = Form { comment: forged(&b), file };
            let (ct, body) = multipart_encode(&form, &[]).unwrap();
            let back: Value = multipart_decode(None, &ct, &body).unwrap();
            assert_eq!(back.as_object().map(Map::len), Some(2), "{back}");
            let back: Form = multipart_decode(None, &ct, &body).unwrap();
            assert_eq!(back.comment, form.comment);
            assert_eq!(back.file.bytes, form.file.bytes);
            let items = [forged(&b)];
            let (ct, body) = crate::codec::mixed_encode(&items, "multipart/mixed", &[]).unwrap();
            assert_eq!(crate::codec::mixed_decode::<Vec<String>>(None, &ct, &body, None).unwrap(), items);
        }
    }

    #[test]
    fn parts_nest_only_when_declared_multipart_and_not_too_deep() {
        let text = |value: Value, types: &[(&str, &str)]| {
            String::from_utf8(multipart_encode(&value, types).unwrap().1).unwrap()
        };
        let meta = text(serde_json::json!({"meta": {"a": 1}}), &[("meta", "application/json")]);
        assert!(meta.contains(r#"{"a":1}"#) && !meta.contains("boundary="), "{meta}");
        let mixed = "multipart/mixed";
        let types = [("f", mixed), ("f/*", mixed), ("f/0/*", mixed), ("f/0/0/*", mixed), ("f/0/0/0/*", mixed)];
        let deep = text(serde_json::json!({"f": [[[[[1]]]]]}), &types);
        // Each nested body is a part whose content type gives its boundary.
        assert_eq!(deep.matches("; boundary=").count(), 4, "{deep}");
    }
}
