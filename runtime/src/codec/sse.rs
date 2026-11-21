//! Server-Sent Events (`text/event-stream`).

use std::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};
use serde::{Serialize, Serializer};
use serde_json::{Map, Value};

use crate::json::to_value;
use crate::text::decode_err;
use crate::{ClientError, coerce::Coerce};

/// Returns Server-Sent Events. The fields of each item (`event`, `data`, `id`, `retry`, ...) become SSE fields.
/// Non-string values are written as JSON.
pub fn sse_encode<T: Serialize>(items: &[T]) -> Result<Vec<u8>, ClientError> {
    let mut out = vec![];
    for i in items {
        sse_write(&mut out, i)?;
    }
    Ok(out)
}

/// Appends one item to `out` as an event. A stream writes each item into the piece that it sends: no buffer per item.
pub(crate) fn sse_write<T: Serialize>(out: &mut Vec<u8>, item: &T) -> Result<(), ClientError> {
    // A struct decides by the fields that it declares. The fields that this
    // value sets do not count. `{id, data}` with `id: None` is still an event.
    // So `sse_item` decodes it back the same way. Other values decide by their keys.
    let declared = struct_shape(item);
    // A struct that is not an event is its data. It is written as JSON straight: through a `Value` each item
    // would be a map, and its keys would come out sorted and not in the order that JSON Lines writes them.
    if declared.is_some_and(|d| !d.is_event()) {
        out.extend_from_slice(b"data: ");
        crate::json::to_writer(out, item).map_err(ClientError::Encode)?;
        out.extend_from_slice(b"\n\n");
        return Ok(());
    }
    match to_value(item)? {
        // An item shaped like an event is written field by field. It has only
        // `data`, `event`, `id` and `retry` (3.2 section 4.14.5). Any other item
        // is the data of the event as JSON. Every `text/event-stream` parser
        // ignores other field names. `EventSource` does too.
        Value::Object(o) if declared.map_or_else(|| o.keys().all(|k| is_event_key(k)), Shape::is_event) => {
            for (k, v) in o {
                let text = match v {
                    Value::Null => continue,
                    Value::String(s) => s,
                    other @ (Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
                        other.to_string()
                    }
                };
                push_field(out, &k, &text)?;
            }
        }
        other @ (Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Array(_)
        | Value::Object(_)) => {
            push_text(out, &["data: ", &other.to_string(), "\n"]);
        }
    }
    out.push(b'\n');
    Ok(())
}

/// Appends each piece of text.
pub(crate) fn push_text(out: &mut Vec<u8>, pieces: &[&str]) {
    for p in pieces {
        out.extend_from_slice(p.as_bytes());
    }
}

/// Writes one field. A `data` value goes line by line. Any other value goes on its one line.
/// Errors: an `id` or `event` with a line break or an `id` with a NUL. No parser parses these back as written.
pub(crate) fn push_field(out: &mut Vec<u8>, key: &str, value: &str) -> Result<(), ClientError> {
    if key != "data" && (value.contains(['\r', '\n']) || (key == "id" && value.contains('\0'))) {
        return Err(ClientError::Encode(format!("an event's `{key}` cannot contain a line break or a NUL: {value:?}")));
    }
    for line in text_lines(value) {
        push_text(out, &[key, ": ", line, "\n"]);
    }
    Ok(())
}

/// Returns the lines of `text` split at `\r\n`, `\n` or `\r` as a parser splits them. Empty lines are kept.
pub(crate) fn text_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(text);
    std::iter::from_fn(move || {
        let text = rest?;
        let Some(end) = memchr::memchr2(b'\r', b'\n', text.as_bytes()) else {
            rest = None;
            return Some(text);
        };
        let ending = if text.as_bytes().get(end..end + 2) == Some(b"\r\n") { 2 } else { 1 };
        rest = text.get(end + ending..);
        text.get(..end)
    })
}

/// Decodes a `text/event-stream` body into one item per event. The body may end inside an event with no blank line
/// after it. That event is dropped as WHATWG says. The stream broke off before it was whole.
pub fn sse_decode<T: DeserializeOwned>(status: Option<u16>, body: &[u8]) -> Result<Vec<T>, ClientError> {
    let body = body.strip_prefix("\u{feff}".as_bytes()).unwrap_or(body);
    let mut items = vec![];
    let mut rest = body;
    while let Ok((block, consumed)) = sse_boundary(rest, 0, true) {
        let text = String::from_utf8_lossy(rest.get(..block).unwrap_or_default());
        if !sse_comment_only(&text) {
            items.push(sse_item(status, &text)?);
        }
        rest = rest.get(consumed..).unwrap_or_default();
    }
    Ok(items)
}

/// Returns what a block sets for the rest of the stream with or without data. That is its last `id` and its last
/// `retry` that is all digits in milliseconds. An empty `id` clears the last event ID. An `id` with a NUL is ignored.
pub(crate) fn sse_state(block: &str) -> (Option<&str>, Option<u64>) {
    let (mut id, mut retry) = (None, None);
    for line in sse_lines(block) {
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let v = v.strip_prefix(' ').unwrap_or(v);
        match k {
            "id" if !v.contains('\0') => id = Some(v),
            "retry" if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => retry = v.parse().ok().or(retry),
            _ => {}
        }
    }
    (id, retry)
}

/// Returns the end of the first event in `buf` and how much to consume with its
/// blank line. A blank line is a line ending (`\r\n`, `\n` or `\r`) right after another
/// (WHATWG "Parsing an event stream"). A trailing `\r` may be the start of
/// `\r\n`. It stays undecided until `done`.
///
/// The search starts at `from`. It must be where an earlier search of the same buffer stopped. Without a boundary
/// the error is that position. So a stream scans each byte once however its chunks arrive.
pub(crate) fn sse_boundary(buf: &[u8], from: usize, done: bool) -> Result<(usize, usize), usize> {
    // The length of a line ending. `None` when it is not one or not decided yet.
    let eol = |i: usize| -> Option<usize> {
        match (buf.get(i), buf.get(i + 1)) {
            (Some(b'\r'), Some(b'\n')) => Some(2),
            (Some(b'\r'), None) if !done => None,
            (Some(b'\r' | b'\n'), _) => Some(1),
            _ => None,
        }
    };
    // Whether the bytes at `i` are too few to tell a line ending.
    let pending = |i: usize| i >= buf.len() || (!done && buf.get(i) == Some(&b'\r') && i + 1 == buf.len());
    let mut i = from;
    while i < buf.len() {
        match eol(i) {
            Some(n) => match eol(i + n) {
                Some(m) => return Ok((i, i + n + m)),
                // The line ending may still start the blank line: look at it again.
                None if pending(i + n) => return Err(i),
                None => i += n,
            },
            None if pending(i) => return Err(i),
            None => i += 1,
        }
    }
    Err(i)
}

/// Returns a block that dispatches no event as WHATWG says. It has no `data` field. It has
/// only comments such as heartbeats or only `id` / `event` / `retry`.
pub(crate) fn sse_comment_only(block: &str) -> bool {
    !sse_lines(block).any(|l| l.split_once(':').map_or(l, |(k, _)| k) == "data")
}

/// Returns the lines of an event block split at `\r\n`, `\n` or `\r`, in one pass over the bytes. Empty lines are left
/// out.
fn sse_lines(block: &str) -> impl Iterator<Item = &str> {
    let mut rest = block;
    std::iter::from_fn(move || {
        loop {
            if rest.is_empty() {
                return None;
            }
            let end = memchr::memchr2(b'\r', b'\n', rest.as_bytes()).unwrap_or(rest.len());
            let line = rest.get(..end).unwrap_or_default();
            let ending = match rest.as_bytes().get(end..) {
                Some([b'\r', b'\n', ..]) => "\r\n".len(),
                Some([_, ..]) => 1,
                _ => 0,
            };
            rest = rest.get(end + ending..).unwrap_or_default();
            if !line.is_empty() {
                return Some(line);
            }
        }
    })
}

/// Returns one event of a `text/event-stream` body parsed as WHATWG says (3.2 section 4.14.5).
/// `data` lines join with `\n`. The last `event` and `id` win. `retry` is a
/// number that is kept only when all digits. Other fields and comments are ignored.
pub(crate) fn sse_item<T: DeserializeOwned>(status: Option<u16>, block: &str) -> Result<T, ClientError> {
    let mut o = Map::new();
    for line in sse_lines(block).filter(|l| !l.starts_with(':')) {
        let (k, v) = line.split_once(':').unwrap_or((line, ""));
        let v = v.strip_prefix(' ').unwrap_or(v);
        match k {
            "data" => match o.get_mut("data") {
                Some(Value::String(prev)) => {
                    prev.push('\n');
                    prev.push_str(v);
                }
                _ => {
                    o.insert("data".to_owned(), Value::String(v.to_owned()));
                }
            },
            "event" => {
                o.insert("event".to_owned(), Value::String(v.to_owned()));
            }
            "id" if !v.contains('\0') => {
                o.insert("id".to_owned(), Value::String(v.to_owned()));
            }
            // A number too large for `u64` is ignored, as any other field that is not one. So is an empty one.
            "retry" if v.bytes().all(|b| b.is_ascii_digit()) => {
                if let Ok(n) = v.parse::<u64>() {
                    o.insert("retry".to_owned(), Value::from(n));
                }
            }
            _ => {}
        }
    }
    // The mirror of `sse_encode`. A struct whose fields are all event fields was
    // written field by field. Its `data` is never a whole item even when it
    // contains a JSON object. Any other struct was written whole as JSON in `data`.
    // Without a declared field list a `data` that contains a JSON object with keys
    // other than the event fields is a whole item.
    let declared = declared_fields::<T>();
    let event_shaped = declared.is_some_and(is_event_shape);
    let data = o.get("data").and_then(Value::as_str);
    // A declared struct decodes its JSON text straight. Through a `Value` every key and string would be
    // allocated twice.
    if !event_shaped
        && declared.is_some()
        && let Some(d) = data.filter(|d| d.trim_start().starts_with('{'))
        && let Ok(item) = serde_json::from_str::<T>(d)
    {
        return Ok(item);
    }
    if !event_shaped
        && declared.is_none()
        && let Some(Ok(Value::Object(inner))) = data.map(serde_json::from_str::<Value>)
        && !inner.keys().all(|k| is_event_key(k))
        && let Ok(item) = T::deserialize(Value::Object(inner))
    {
        return Ok(item);
    }
    match T::deserialize(Coerce(Value::Object(o))) {
        Ok(item) => Ok(item),
        // Items that are not objects travel as `data: <json>`.
        Err(e) => {
            let data = sse_data(block);
            if let Some(item) = data.as_deref().and_then(|d| serde_json::from_str::<T>(d).ok()) {
                return Ok(item);
            }
            // An item written whole whose `data` is no JSON at all. An example is `[DONE]`, the end marker that some
            // APIs send. Say so and not which field the fields of the event lacked.
            match data.filter(|d| !event_shaped && serde_json::from_str::<de::IgnoredAny>(d).is_err()) {
                Some(d) => Err(decode_err(status, block.as_bytes(), format!("the event's data is not JSON: `{d}`"))),
                None => Err(decode_err(status, block.as_bytes(), e)),
            }
        }
    }
}

/// Returns the `data` of an event block with its lines joined with `\n`. This is what [`sse_item`] decoded. The rare
/// item that needs it again uses it.
fn sse_data(block: &str) -> Option<String> {
    let lines: Vec<&str> = sse_lines(block)
        .map(|l| l.split_once(':').unwrap_or((l, "")))
        .filter(|(k, _)| *k == "data")
        .map(|(_, v)| v.strip_prefix(' ').unwrap_or(v))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Returns a field that `text/event-stream` defines. Every parser ignores other names. `EventSource` does too.
fn is_event_key(key: &str) -> bool {
    matches!(key, "data" | "event" | "id" | "retry")
}

/// Returns an item type that describes the event itself. It declares fields and all of them are event fields.
fn is_event_shape(fields: &[&str]) -> bool {
    !fields.is_empty() && fields.iter().all(|k| is_event_key(k))
}

/// Returns the shape of the fields that a struct declares from its `Serialize`, set or skipped. `None`
/// for anything else such as a map, a scalar or a struct with flattened fields.
fn struct_shape<T: Serialize + ?Sized>(v: &T) -> Option<Shape> {
    v.serialize(FieldNames).ok()
}

/// Returns the fields that a struct declares from its `Deserialize`. `None` for anything else.
fn declared_fields<T: DeserializeOwned>() -> Option<&'static [&'static str]> {
    match T::deserialize(FieldNames) {
        Err(FieldsError(found)) => found,
        Ok(_) => None,
    }
}

/// Reads the field names of a struct through serde without its values.
struct FieldNames;

/// Not a struct. Or (as a deserializer) the fields that a struct asked for.
#[derive(Debug)]
struct FieldsError(Option<&'static [&'static str]>);

impl fmt::Display for FieldsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a struct")
    }
}
impl std::error::Error for FieldsError {}
impl serde::ser::Error for FieldsError {
    fn custom<M: fmt::Display>(_: M) -> Self {
        Self(None)
    }
}
impl de::Error for FieldsError {
    fn custom<M: fmt::Display>(_: M) -> Self {
        Self(None)
    }
}

impl<'de> Deserializer<'de> for FieldNames {
    type Error = FieldsError;

    fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, FieldsError> {
        Err(FieldsError(None))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        fields: &'static [&'static str],
        _: V,
    ) -> Result<V::Value, FieldsError> {
        Err(FieldsError(Some(fields)))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit
        unit_struct newtype_struct seq tuple tuple_struct map enum identifier ignored_any
    }
}

/// Collects the keys a struct serializes or skips.
struct FieldList(Shape);

/// How many fields a struct declares and how many of them are event fields. Counted, not collected: a stream asks
/// for each item.
#[derive(Clone, Copy, Default)]
struct Shape {
    fields: usize,
    event_fields: usize,
}

impl Shape {
    /// Returns whether the struct declares fields and all of them are event fields.
    const fn is_event(self) -> bool {
        self.fields > 0 && self.fields == self.event_fields
    }
}

impl serde::ser::SerializeStruct for FieldList {
    type Ok = Shape;
    type Error = FieldsError;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, _: &T) -> Result<(), FieldsError> {
        self.skip_field(key)
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), FieldsError> {
        self.0.fields += 1;
        self.0.event_fields += usize::from(is_event_key(key));
        Ok(())
    }
    fn end(self) -> Result<Shape, FieldsError> {
        Ok(self.0)
    }
}

/// Every non-struct shape: no field list.
macro_rules! not_a_struct {
    ($($f:ident($($t:ty),*)),*) => {
        $(fn $f(self, $(_: $t),*) -> Result<Shape, FieldsError> { Err(FieldsError(None)) })*
    };
}

impl Serializer for FieldNames {
    type Ok = Shape;
    type Error = FieldsError;
    type SerializeSeq = serde::ser::Impossible<Shape, FieldsError>;
    type SerializeTuple = serde::ser::Impossible<Shape, FieldsError>;
    type SerializeTupleStruct = serde::ser::Impossible<Shape, FieldsError>;
    type SerializeTupleVariant = serde::ser::Impossible<Shape, FieldsError>;
    type SerializeMap = serde::ser::Impossible<Shape, FieldsError>;
    type SerializeStruct = FieldList;
    type SerializeStructVariant = serde::ser::Impossible<Shape, FieldsError>;

    not_a_struct!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
        serialize_none(),
        serialize_unit(),
        serialize_unit_struct(&'static str),
        serialize_unit_variant(&'static str, u32, &'static str)
    );

    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<Shape, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, _: &T) -> Result<Shape, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Shape, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeTupleStruct, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, FieldsError> {
        Err(FieldsError(None))
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<FieldList, FieldsError> {
        Ok(FieldList(Shape::default()))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, FieldsError> {
        Err(FieldsError(None))
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    /// A struct that is not an event is its JSON as `data`, with its fields in the order that it declares them.
    #[test]
    fn a_struct_that_is_not_an_event_keeps_its_field_order() {
        #[derive(serde::Serialize)]
        struct Tile {
            tile: &'static str,
            rows: i64,
        }
        let body = sse_encode(&[Tile { tile: "a", rows: 1 }, Tile { tile: "b\nc", rows: 2 }]).unwrap();
        assert_eq!(
            String::from_utf8(body).unwrap(),
            "data: {\"tile\":\"a\",\"rows\":1}\n\ndata: {\"tile\":\"b\\nc\",\"rows\":2}\n\n"
        );
    }

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Frame {
        data: String,
        event: Option<String>,
        id: Option<String>,
        retry: Option<u64>,
    }

    fn events(body: &str) -> Vec<Frame> {
        sse_decode(Some(200), body.as_bytes()).expect("decodes")
    }

    #[test]
    fn every_line_ending_separates_events() {
        for body in [
            "data: a\n\ndata: b\n\n",
            "data: a\r\n\r\ndata: b\r\n\r\n",
            "data: a\r\rdata: b\r\r",
            "data: a\n\r\ndata: b\n\r",
        ] {
            let got: Vec<String> = events(body).into_iter().map(|e| e.data).collect();
            assert_eq!(got, ["a", "b"], "{body:?}");
        }
    }

    #[test]
    fn data_lines_join_and_other_fields_take_the_last_value() {
        let e = events("event: x\nevent: y\ndata: 1\ndata: 2\nid: 7\nretry: 3000\nfoo: bar\n\n");
        assert_eq!(
            e,
            [Frame { data: "1\n2".into(), event: Some("y".into()), id: Some("7".into()), retry: Some(3000) }]
        );
    }

    #[test]
    fn a_block_without_data_dispatches_nothing() {
        let e = events(": heartbeat\n\nid: 5\n\nevent: ping\n\ndata: x\n\n");
        assert_eq!(e.len(), 1, "only the block with data");
        assert_eq!(e[0].data, "x");
    }

    #[test]
    fn an_event_the_body_ends_inside_is_dropped() {
        let got: Vec<String> = events("data: a\n\ndata: b\n").into_iter().map(|e| e.data).collect();
        assert_eq!(got, ["a"]);
    }

    #[test]
    fn a_block_sets_the_last_event_id_and_retry_with_or_without_data() {
        assert_eq!(sse_state("id: 5\nretry: 10\nretry: x"), (Some("5"), Some(10)));
        assert_eq!(sse_state("id\ndata: a"), (Some(""), None), "an empty id clears it");
        assert_eq!(sse_state("id: a\0b\n: c"), (None, None), "an id with a NUL is ignored");
    }

    #[test]
    fn data_splits_at_every_line_ending_and_other_fields_refuse_one() {
        let e = serde_json::json!({"data": "a\r\nb\rc\n\nd"});
        let body = sse_encode(&[e]).unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "data: a\ndata: b\ndata: c\ndata: \ndata: d\n\n");
        assert_eq!(events(&String::from_utf8_lossy(&body))[0].data, "a\nb\nc\n\nd");
        for bad in [serde_json::json!({"data": "x", "id": "a\rb"}), serde_json::json!({"data": "x", "event": "a\nb"})] {
            assert_matches!(sse_encode(&[bad]), Err(ClientError::Encode(_)));
        }
        assert_matches!(sse_encode(&[serde_json::json!({"data": "x", "id": "a\0"})]), Err(ClientError::Encode(_)));
    }

    #[test]
    fn a_non_numeric_retry_is_ignored_and_a_bom_is_skipped() {
        let e = events("\u{feff}data: x\nretry: 10s\n\n");
        assert_eq!(e, [Frame { data: "x".into(), event: None, id: None, retry: None }]);
        assert_eq!(events("data: x\nretry:\n\n")[0].retry, None, "empty");
    }

    #[test]
    fn an_item_that_is_not_an_event_travels_as_json_data() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct Product {
            name: String,
            stock: u32,
        }
        let p = Product { name: "x".into(), stock: 2 };
        let body = sse_encode(std::slice::from_ref(&p)).unwrap();
        let text = String::from_utf8(body.clone()).unwrap();
        let json = text.strip_prefix("data: ").and_then(|t| t.strip_suffix("\n\n")).expect("one data line");
        assert_eq!(serde_json::from_str::<Value>(json).unwrap(), serde_json::json!({"name": "x", "stock": 2}));
        assert_eq!(sse_decode::<Product>(Some(200), &body).unwrap(), [p]);
        let e = serde_json::json!({"data": "hi", "event": "greet"});
        assert_eq!(String::from_utf8(sse_encode(&[e]).unwrap()).unwrap(), "data: hi\nevent: greet\n\n");
    }

    /// The learn.openapis.org Sequential Media Types example: `itemSchema` with
    /// `id` and an object `data`.
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Update {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Map<String, Value>>,
    }

    #[test]
    fn an_event_shaped_item_with_object_data_round_trips() {
        let data = serde_json::json!({"k": "v"}).as_object().cloned();
        for u in [Update { id: Some("s".into()), data: data.clone() }, Update { id: None, data }] {
            let body = sse_encode(std::slice::from_ref(&u)).unwrap();
            assert_eq!(sse_decode::<Update>(Some(200), &body).unwrap(), [u], "{}", String::from_utf8_lossy(&body));
        }
    }

    #[test]
    fn a_struct_decides_by_its_declared_fields_not_by_the_ones_it_sets() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct Tagged {
            #[serde(skip_serializing_if = "Option::is_none")]
            data: Option<Map<String, Value>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            kind: Option<String>,
        }
        // Only `data` is set but `kind` is declared. So the whole item travels as JSON.
        let t = Tagged { data: serde_json::json!({"k": "v"}).as_object().cloned(), kind: None };
        let body = sse_encode(std::slice::from_ref(&t)).unwrap();
        assert_eq!(String::from_utf8_lossy(&body), "data: {\"data\":{\"k\":\"v\"}}\n\n");
        assert_eq!(sse_decode::<Tagged>(Some(200), &body).unwrap(), [t]);
        assert_eq!(declared_fields::<Update>(), Some(&["id", "data"][..]));
        assert_eq!(declared_fields::<Value>(), None);
    }

    #[test]
    fn a_trailing_cr_waits_for_its_lf_while_streaming() {
        assert_eq!(sse_boundary(b"data: a\n\r", 0, false), Err(7), "may be \\r\\n");
        assert_eq!(sse_boundary(b"data: a\n\r", 0, true), Ok((7, 9)));
        assert_eq!(sse_boundary(b"data: a\n\r\nx", 0, false), Ok((7, 10)));
    }

    /// An event-shaped item decodes its `data` as the field. A `data` object is not the whole item.
    #[test]
    fn an_event_shaped_item_keeps_object_data_whole() {
        let inner = serde_json::json!({"data": {"k": "v"}}).as_object().cloned();
        let body = b"data: {\"data\": {\"k\": \"v\"}}\n\n";
        assert_eq!(sse_decode::<Update>(Some(200), body).unwrap(), [Update { id: None, data: inner }]);
    }

    #[test]
    fn an_id_with_nul_is_ignored_and_an_option_is_its_value() {
        let items = sse_decode::<Update>(Some(200), b"id: a\0b\ndata: {}\n\n").unwrap();
        assert_eq!(items[0].id, None);
        assert_eq!(String::from_utf8(sse_encode(&[Some(5)]).unwrap()).unwrap(), "data: 5\n\n");
    }
}
