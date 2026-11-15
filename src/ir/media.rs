//! Media types: how a body or message payload is serialized.

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
    /// JSON and `+json` types.
    JsonLines,
    /// JSON and `+json` types.
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
