//! Media types: how a body or message payload is serialized, and what of its schema each one checks.

use serde_json::Value;

use super::types::{Builder, Ty};
use crate::json::join;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Serialization of a body.
pub(crate) enum MediaKind {
    /// JSON and `+json` types.
    Json,
    /// Text (`text/*` or XML) as a string.
    Text,
    /// Raw bytes (`application/octet-stream`, images, ...).
    Bytes,
    /// `application/x-www-form-urlencoded`.
    Form,
    /// `multipart/form-data`.
    Multipart,
    /// Other `multipart/*` types (`mixed` or `related`) with one part per array item.
    MultipartMixed,
    /// JSON Lines (`application/jsonl`).
    JsonLines,
    /// JSON text sequences (`application/json-seq` and `+json-seq`).
    JsonSeq,
    /// Server-Sent Events (`text/event-stream`).
    Sse,
    /// `application/xml`, `text/xml` and `+xml` for object bodies.
    Xml,
    /// `application/linkset` (RFC 9264): links in `Link` header syntax typed by their `+json` model.
    Linkset,
}

impl MediaKind {
    /// Classifies a media type (parameters ignored).
    pub(crate) fn of(media: &str) -> Self {
        let m = media.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        match m.as_str() {
            "application/jsonl" | "application/x-ndjson" | "application/x-jsonlines" | "application/jsonlines" => {
                Self::JsonLines
            }
            "application/json-seq" => Self::JsonSeq,
            // RFC 8091 structured syntax suffix: `application/geo+json-seq` is a JSON text sequence too.
            _ if m.ends_with("+json-seq") => Self::JsonSeq,
            "text/event-stream" => Self::Sse,
            "application/linkset" => Self::Linkset,
            "application/x-www-form-urlencoded" => Self::Form,
            "multipart/form-data" => Self::Multipart,
            _ if m.starts_with("multipart/") => Self::MultipartMixed,
            _ if m == "application/json" || m.ends_with("+json") || m.ends_with("/json") => Self::Json,
            _ if m.ends_with("/xml") || m.ends_with("+xml") => Self::Xml,
            _ if m.starts_with("text/") => Self::Text,
            _ => Self::Bytes,
        }
    }

    /// Returns whether the media type carries a list of items.
    pub(crate) fn is_sequence(self) -> bool {
        matches!(self, Self::JsonLines | Self::JsonSeq | Self::Sse)
    }

    /// Returns `multipart/*`: parts between the delimiters of a boundary.
    pub(crate) fn is_multipart(self) -> bool {
        matches!(self, Self::Multipart | Self::MultipartMixed)
    }
}

/// The keywords that the type of a schema checks on a string: a text body with one of them takes that type.
const TEXT_KEYWORDS: [&str; 5] = ["minLength", "maxLength", "pattern", "enum", "const"];

/// Is any of [`TEXT_KEYWORDS`] in `schema`, or a string `format` that a type checks?
pub(crate) fn limits_text(schema: Option<&Value>) -> bool {
    schema.is_some_and(|s| {
        TEXT_KEYWORDS.iter().any(|k| s.get(*k).is_some())
            || s.get("format").and_then(Value::as_str).and_then(crate::ir::types::string_format).is_some()
    })
}

/// Warns about each of [`TEXT_KEYWORDS`] in the schema of a binary body: the body is bytes, which no check of the keyword
/// reads.
pub(crate) fn unchecked_keywords(b: &mut Builder<'_>, kind: MediaKind, schema: Option<&Value>, ptr: &str, media: &str) {
    // A text body is a string: no check reads a schema of another type against it.
    if kind == MediaKind::Text
        && let Some(t) = schema.and_then(|s| s.get("type")).and_then(Value::as_str).filter(|t| *t != "string")
    {
        b.diags.warning(join(ptr, "type"), format!("a `{media}` body is text: it is not checked as `{t}`"));
    }
    if kind != MediaKind::Bytes {
        return;
    }
    for key in TEXT_KEYWORDS.iter().filter(|k| schema.is_some_and(|s| s.get(**k).is_some())) {
        b.diags.warning(join(ptr, key), format!("`{key}` is not checked on a `{media}` body: it is bytes"));
    }
}

/// Returns `ty` when XML maps it: an object. Anything else stays the raw text, `None`, with a warning unless the schema
/// asks for text or says nothing.
pub(crate) fn xml_ty(b: &mut Builder<'_>, ty: Ty, ptr: &str, media: &str) -> Option<Ty> {
    if b.is_struct(&ty) {
        return Some(ty);
    }
    if !matches!(ty, Ty::String | Ty::Any) {
        b.diags.warning(join(ptr, "schema"), format!("a `{media}` body maps only an object: this one stays raw text"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::MediaKind;

    /// The sequential JSON of the media-type registry with the `+json-seq` suffix of RFC 8091.
    #[test]
    fn sequential_json_media_types_are_sequences() {
        for (media, kind) in [
            ("application/json-seq", MediaKind::JsonSeq),
            ("application/geo+json-seq", MediaKind::JsonSeq),
            ("application/vnd.example+json-seq; charset=utf-8", MediaKind::JsonSeq),
            ("application/jsonl", MediaKind::JsonLines),
            ("application/x-ndjson", MediaKind::JsonLines),
            ("application/geo+json", MediaKind::Json),
        ] {
            assert_eq!(MediaKind::of(media), kind, "{media}");
            assert_eq!(MediaKind::of(media).is_sequence(), kind != MediaKind::Json, "{media}");
        }
    }
}
