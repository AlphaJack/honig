//! The XML encoder: a value written as the `xml` objects of its schemas shape it.
//!
//! Most bodies are written as they serialize: their fields come in the order of their shape. Any other body is written
//! again with each field rendered alone and then put in order. That is the attributes, the text and the elements in the
//! order of the shape, then the properties that the shape does not list in the order of a `serde_json::Map`. That
//! covers a field out of order, an unlisted property, a map and a `nodeType: none` field. A leaf that is not text, a
//! number or a boolean is written from its `serde_json::Value`, as JSON represents it. Examples are an `f32`, bytes, a
//! tuple, an enum variant with data and a raw JSON value such as a `Decimal`.

use std::fmt::{self, Write as _};

use serde::Serialize;
use serde::ser::{self, Impossible, SerializeMap, SerializeSeq, SerializeStruct};
use serde_json::{Map, Value};

use super::write::{self, Tag};
use super::{XmlField, XmlNode};
use crate::ClientError;

/// The name under which `serde_json` serializes a raw value. Only its own serializer writes one as written.
const RAW_VALUE: &str = "$serde_json::private::RawValue";
/// The most bytes of one `char` in UTF-8.
const CHAR_BYTES: usize = 4;

/// Why writing stopped. `None`: this way of writing cannot write the value, another way can. `Some`: the value does
/// not serialize.
#[derive(Debug)]
pub(super) struct Stop(Option<ClientError>);

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(e) => e.fmt(f),
            None => f.write_str("not written this way"),
        }
    }
}

impl std::error::Error for Stop {}

impl ser::Error for Stop {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self(None)
    }
}

type Result<T = ()> = std::result::Result<T, Stop>;

fn other_way<T>() -> Result<T> {
    Err(Stop(None))
}

/// How the fields of an element are written.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// As they come. A field out of the order of the shape stops it.
    Stream,
    /// Each one rendered alone and then put in the order of the shape.
    Ordered,
}

/// Returns `v` as an XML document with `node` at its root.
pub(super) fn body<T: Serialize + ?Sized>(v: &T, node: &'static XmlNode) -> std::result::Result<String, ClientError> {
    let ns = write::namespace(node);
    let mut out = String::from(write::DECLARATION);
    element(&mut out, v, write::root(node), Some(node), ns.as_deref().unwrap_or(""))?;
    Ok(out)
}

/// Returns a `Value` as an XML document whose root is `root`.
#[cfg(feature = "multipart")]
pub(super) fn document(root: &str, v: &Value) -> String {
    let mut out = String::from(write::DECLARATION);
    value_element(&mut out, v, Tag::plain(root));
    out
}

/// Returns a form field sent as `application/xml`: each property of an object as an element, any other value as its
/// text.
pub(super) fn fields(v: &Value) -> String {
    let mut out = String::new();
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                value_element(&mut out, x, Tag::plain(k));
            }
        }
        other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_)) => {
            out.push_str(&write::escape(&crate::scalar::scalar(other)));
        }
    }
    out
}

/// Returns `v` as the element `tag`: streamed, else ordered, else from its `Value`.
fn element<T: Serialize + ?Sized>(
    out: &mut String,
    v: &T,
    tag: Tag<'_>,
    node: Option<&'static XmlNode>,
    extra: &str,
) -> std::result::Result<(), ClientError> {
    let mark = out.len();
    for mode in [Mode::Stream, Mode::Ordered] {
        match v.serialize(Element { out, tag, node, extra, mode }) {
            Ok(()) => return Ok(()),
            Err(Stop(Some(e))) => return Err(e),
            Err(Stop(None)) => out.truncate(mark),
        }
    }
    let value = crate::json::to_value(v)?;
    value.serialize(Element { out, tag, node, extra, mode: Mode::Ordered }).map_err(written)
}

/// Writes a `Value` as the element `tag` without a shape. Every `Value` writes.
fn value_element(out: &mut String, v: &Value, tag: Tag<'_>) {
    let mark = out.len();
    if v.serialize(Element { out, tag, node: None, extra: "", mode: Mode::Ordered }).is_err() {
        out.truncate(mark);
    }
}

/// Returns the error of a `Value` that did not write. Every `Value` writes, so it is never `None`.
fn written(stop: Stop) -> ClientError {
    stop.0.unwrap_or_else(|| ClientError::Encode("a value XML cannot represent".to_owned()))
}

/// Returns the `Value` of `v` for a part that is not written as it serializes.
fn value_of<T: Serialize + ?Sized>(v: &T) -> Result<Value> {
    crate::json::to_value(v).map_err(|e| Stop(Some(e)))
}

/// Returns a number as `serde_json` writes it in a `Value`.
fn float(out: &mut String, v: f64) -> Result {
    let n = serde_json::Number::from_f64(v).ok_or(Stop(None))?;
    write!(out, "{n}").map_err(|_| Stop(None))
}

/// One value as the element `tag`. A list is that element once per item.
struct Element<'o, 'n> {
    out: &'o mut String,
    tag: Tag<'n>,
    node: Option<&'static XmlNode>,
    extra: &'n str,
    mode: Mode,
}

impl<'o, 'n> Element<'o, 'n> {
    /// Returns a leaf: `<tag>text</tag>`, where `text` writes the text.
    fn leaf(self, text: impl FnOnce(&mut String) -> Result) -> Result {
        write::start(self.out, self.tag, self.extra);
        text(self.out)?;
        write::close(self.out, self.tag);
        Ok(())
    }

    fn fields(self, inline: bool) -> Result<Fields<'o, 'n>> {
        Fields::new(self.out, self.tag, self.node, self.extra, self.mode, inline)
    }
}

macro_rules! integer_leaf {
    ($($method:ident: $ty:ty),*) => {$(
        fn $method(self, v: $ty) -> Result {
            self.leaf(|out| write!(out, "{v}").map_err(|_| Stop(None)))
        }
    )*};
}

impl<'o, 'n> ser::Serializer for Element<'o, 'n> {
    type Ok = ();
    type Error = Stop;
    type SerializeSeq = Items<'o, 'n>;
    type SerializeTuple = Impossible<(), Stop>;
    type SerializeTupleStruct = Impossible<(), Stop>;
    type SerializeTupleVariant = Impossible<(), Stop>;
    type SerializeMap = Fields<'o, 'n>;
    type SerializeStruct = Fields<'o, 'n>;
    type SerializeStructVariant = Impossible<(), Stop>;

    integer_leaf!(
        serialize_i8: i8, serialize_i16: i16, serialize_i32: i32, serialize_i64: i64,
        serialize_u8: u8, serialize_u16: u16, serialize_u32: u32, serialize_u64: u64
    );

    fn serialize_bool(self, v: bool) -> Result {
        self.leaf(|out| {
            out.push_str(if v { "true" } else { "false" });
            Ok(())
        })
    }
    fn serialize_f32(self, _: f32) -> Result {
        other_way()
    }
    fn serialize_f64(self, v: f64) -> Result {
        self.leaf(|out| float(out, v))
    }
    fn serialize_char(self, v: char) -> Result {
        self.serialize_str(v.encode_utf8(&mut [0; CHAR_BYTES]))
    }
    fn serialize_str(self, v: &str) -> Result {
        self.leaf(|out| {
            out.push_str(&write::escape(v));
            Ok(())
        })
    }
    fn serialize_bytes(self, _: &[u8]) -> Result {
        other_way()
    }
    fn serialize_none(self) -> Result {
        write::nil(self.out, self.tag, self.extra);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result {
        self.serialize_none()
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result {
        self.serialize_none()
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result {
        self.serialize_str(variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Result {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result {
        other_way()
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Items<'o, 'n>> {
        Ok(Items { out: self.out, tag: self.tag, node: self.node, extra: self.extra, mode: self.mode })
    }
    fn serialize_tuple(self, _: usize) -> Result<Impossible<(), Stop>> {
        other_way()
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Impossible<(), Stop>> {
        other_way()
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Impossible<(), Stop>> {
        other_way()
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Fields<'o, 'n>> {
        self.fields(false)
    }
    fn serialize_struct(self, name: &'static str, _: usize) -> Result<Fields<'o, 'n>> {
        if name == RAW_VALUE {
            return other_way();
        }
        self.fields(false)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Impossible<(), Stop>> {
        other_way()
    }
}

/// The items of a list: each one the element `tag`.
struct Items<'o, 'n> {
    out: &'o mut String,
    tag: Tag<'n>,
    node: Option<&'static XmlNode>,
    extra: &'n str,
    mode: Mode,
}

impl SerializeSeq for Items<'_, '_> {
    type Ok = ();
    type Error = Stop;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result {
        v.serialize(Element { out: self.out, tag: self.tag, node: self.node, extra: self.extra, mode: self.mode })
    }
    fn end(self) -> Result {
        Ok(())
    }
}

/// The fields of an object element.
///
/// Streamed, attributes go in its start tag as they come, then its text, then its child elements. Each kind comes
/// in the order of the shape. Ordered, each field is rendered into its slot and the element is written at its end.
struct Fields<'o, 'n> {
    out: &'o mut String,
    tag: Tag<'n>,
    extra: &'n str,
    /// The fields of the shape. None without one: every property is then one that it does not list.
    shape: &'static [XmlField<XmlNode>],
    mode: Mode,
    /// `nodeType: none`: only the text and the elements, inside the element of the parent.
    inline: bool,
    /// Streamed: the start tag still accepts attributes.
    open: bool,
    /// Streamed: the index in the shape of the last attribute, text and element written.
    attribute: Option<usize>,
    text: Option<usize>,
    element: Option<usize>,
    /// Ordered: what each field of the shape rendered and each property that it does not list by name. Those keep
    /// the order of a `serde_json::Map`, as in a `Value`: sorted by name, or as they came with `preserve_order`.
    slots: Vec<Option<String>>,
    unlisted: Map<String, Value>,
    /// The key of the map entry whose value comes next.
    key: Option<String>,
}

/// Returns whether `i` comes after `last` in the shape. It records it.
fn after(last: &mut Option<usize>, i: usize) -> bool {
    let later = last.is_none_or(|l| i > l);
    *last = Some(i);
    later
}

impl<'o, 'n> Fields<'o, 'n> {
    fn new(
        out: &'o mut String,
        tag: Tag<'n>,
        node: Option<&'static XmlNode>,
        extra: &'n str,
        mode: Mode,
        inline: bool,
    ) -> Result<Self> {
        let shape = node.map_or(&[][..], |n| n.fields);
        if mode == Mode::Stream {
            // A property without a shape is one that no shape lists: they sort by name.
            if node.is_none() || inline {
                return other_way();
            }
            write::open(out, tag, extra);
        }
        let slots = if mode == Mode::Ordered { shape.iter().map(|_| None).collect() } else { Vec::new() };
        Ok(Self {
            out,
            tag,
            extra,
            shape,
            mode,
            inline,
            open: true,
            attribute: None,
            text: None,
            element: None,
            slots,
            unlisted: Map::new(),
            key: None,
        })
    }

    fn end_start_tag(&mut self) {
        if self.open {
            self.out.push('>');
            self.open = false;
        }
    }

    fn field<T: Serialize + ?Sized>(&mut self, key: &str, v: &T) -> Result {
        let found = self.shape.iter().enumerate().find(|(_, f)| f.wire == key);
        match self.mode {
            Mode::Stream => {
                let (i, f) = found.ok_or(Stop(None))?;
                self.stream(i, f, v)
            }
            Mode::Ordered => match found {
                // An inline object keeps only its text and its elements.
                Some((_, f)) if self.inline && f.attribute => Ok(()),
                Some((i, f)) => {
                    if let Some(slot) = self.slots.get_mut(i) {
                        *slot = rendered(f, v)?;
                    }
                    Ok(())
                }
                None if self.inline => Ok(()),
                None => {
                    let mut s = String::new();
                    or_value(&mut s, v, |out, v| v.write(element_in(out, Tag::plain(key), None).into()))?;
                    self.unlisted.insert(key.to_owned(), Value::String(s));
                    Ok(())
                }
            },
        }
    }

    /// Writes field `i` of the shape as it comes.
    fn stream<T: Serialize + ?Sized>(&mut self, i: usize, f: &'static XmlField<XmlNode>, v: &T) -> Result {
        if f.inline {
            return other_way();
        }
        if f.attribute {
            if !self.open || !after(&mut self.attribute, i) {
                return other_way();
            }
            let mark = self.out.len();
            write::attribute_start(self.out, write::field(f));
            if v.serialize(Scalar { out: self.out, cdata: false })? {
                write::attribute_end(self.out);
            } else {
                // `null` writes no attribute.
                self.out.truncate(mark);
            }
            return Ok(());
        }
        self.end_start_tag();
        if f.text {
            if self.element.is_some() || !after(&mut self.text, i) {
                return other_way();
            }
            let mark = self.out.len();
            if f.cdata {
                write::cdata_start(self.out);
            }
            if !v.serialize(Scalar { out: self.out, cdata: f.cdata })? {
                self.out.truncate(mark);
            } else if f.cdata {
                write::cdata_end(self.out);
            }
            return Ok(());
        }
        if !after(&mut self.element, i) {
            return other_way();
        }
        v.serialize(Field { out: self.out, f, mode: Mode::Stream })
    }
}

/// Returns an element without a shape, ordered: a property that no shape lists.
fn element_in<'o, 'n>(out: &'o mut String, tag: Tag<'n>, node: Option<&'static XmlNode>) -> Element<'o, 'n> {
    Element { out, tag, node, extra: "", mode: Mode::Ordered }
}

/// Returns `write` of `v` into `out`. Where `v` is not written as it serializes, `write` of its `Value`.
fn or_value<T: Serialize + ?Sized>(
    out: &mut String,
    v: &T,
    write: impl Fn(&mut String, &dyn Writes) -> Result,
) -> Result {
    match write(out, &Of(v)) {
        Err(Stop(None)) => {
            out.clear();
            let value = value_of(v)?;
            write(out, &Of(&value)).map_err(|s| Stop(Some(written(s))))
        }
        other => other,
    }
}

/// Writes the field `f` of an ordered element alone. `None` writes nothing: an attribute or text that is `null`.
fn rendered<T: Serialize + ?Sized>(f: &'static XmlField<XmlNode>, v: &T) -> Result<Option<String>> {
    let mut s = String::new();
    if f.attribute || f.text {
        let present = match v.serialize(Scalar { out: &mut s, cdata: f.cdata }) {
            Err(Stop(None)) => {
                s.clear();
                // An array or an object as an attribute or text is its JSON.
                let value = value_of(v)?;
                if value.is_null() {
                    false
                } else {
                    let text = crate::scalar::scalar_ref(&value);
                    if f.cdata {
                        write::cdata_text(&mut s, &text);
                    } else {
                        s.push_str(&write::escape(&text));
                    }
                    true
                }
            }
            other => other?,
        };
        if !present {
            return Ok(None);
        }
        let mut out = String::new();
        if f.attribute {
            write::attribute_start(&mut out, write::field(f));
            out.push_str(&s);
            write::attribute_end(&mut out);
        } else if f.cdata {
            write::cdata_start(&mut out);
            out.push_str(&s);
            write::cdata_end(&mut out);
        } else {
            out = s;
        }
        return Ok(Some(out));
    }
    or_value(&mut s, v, |out, v| v.write(Field { out, f, mode: Mode::Ordered }.into()))?;
    Ok(Some(s))
}

/// A value that writes as an element or as a field through `&dyn`: [`or_value`] writes a value or its `Value` with
/// one closure.
trait Writes {
    fn write(&self, to: Target<'_, '_>) -> Result;
}

/// Where a value writes.
enum Target<'o, 'n> {
    Element(Element<'o, 'n>),
    Field(Field<'o>),
}

impl<'o, 'n> From<Element<'o, 'n>> for Target<'o, 'n> {
    fn from(e: Element<'o, 'n>) -> Self {
        Self::Element(e)
    }
}

impl<'o> From<Field<'o>> for Target<'o, '_> {
    fn from(f: Field<'o>) -> Self {
        Self::Field(f)
    }
}

struct Of<'v, T: ?Sized>(&'v T);

impl<T: Serialize + ?Sized> Writes for Of<'_, T> {
    fn write(&self, to: Target<'_, '_>) -> Result {
        match to {
            Target::Element(e) => self.0.serialize(e),
            Target::Field(f) => self.0.serialize(f),
        }
    }
}

impl SerializeStruct for Fields<'_, '_> {
    type Ok = ();
    type Error = Stop;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result {
        self.field(key, v)
    }

    fn end(mut self) -> Result {
        if self.mode == Mode::Stream {
            self.end_start_tag();
            write::close(self.out, self.tag);
            return Ok(());
        }
        // The slots of one kind of field in the order of the shape.
        let push = |out: &mut String, slots: &[Option<String>], kind: fn(&XmlField<XmlNode>) -> bool| {
            for (_, s) in self.shape.iter().zip(slots).filter(|(f, _)| kind(f)) {
                out.push_str(s.as_deref().unwrap_or_default());
            }
        };
        if !self.inline {
            write::open(self.out, self.tag, self.extra);
            push(self.out, &self.slots, |f| f.attribute);
            self.out.push('>');
        }
        push(self.out, &self.slots, |f| f.text);
        push(self.out, &self.slots, XmlField::is_element);
        if !self.inline {
            for s in self.unlisted.values().filter_map(Value::as_str) {
                self.out.push_str(s);
            }
            write::close(self.out, self.tag);
        }
        Ok(())
    }
}

impl SerializeMap for Fields<'_, '_> {
    type Ok = ();
    type Error = Stop;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result {
        let mut text = String::new();
        if !key.serialize(Scalar { out: &mut text, cdata: true })? {
            return other_way();
        }
        self.key = Some(text);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result {
        let key = self.key.take().ok_or(Stop(None))?;
        self.field(&key, v)
    }

    fn end(self) -> Result {
        SerializeStruct::end(self)
    }
}

/// The value of a child element field: a list writes its items as the shape says.
struct Field<'o> {
    out: &'o mut String,
    f: &'static XmlField<XmlNode>,
    mode: Mode,
}

impl<'o> Field<'o> {
    fn element(self) -> Element<'o, 'static> {
        let node = self.f.child.map(|c| c());
        Element { out: self.out, tag: write::field(self.f), node, extra: "", mode: self.mode }
    }

    /// Returns the fields of an object value: inlined for `nodeType: none` with a shape, else its own element.
    fn fields(self) -> Result<Fields<'o, 'static>> {
        let inline = self.f.inline && self.f.child.is_some();
        self.element().fields(inline)
    }
}

/// A method that writes the field as its element.
macro_rules! as_element {
    ($($method:ident($($arg:ident: $ty:ty),*);)*) => {$(
        fn $method(self, $($arg: $ty),*) -> Result {
            self.element().$method($($arg),*)
        }
    )*};
}

impl<'o> ser::Serializer for Field<'o> {
    type Ok = ();
    type Error = Stop;
    type SerializeSeq = Wrapped<'o>;
    type SerializeTuple = Impossible<(), Stop>;
    type SerializeTupleStruct = Impossible<(), Stop>;
    type SerializeTupleVariant = Impossible<(), Stop>;
    type SerializeMap = Fields<'o, 'static>;
    type SerializeStruct = Fields<'o, 'static>;
    type SerializeStructVariant = Impossible<(), Stop>;

    as_element! {
        serialize_bool(v: bool);
        serialize_i8(v: i8);
        serialize_i16(v: i16);
        serialize_i32(v: i32);
        serialize_i64(v: i64);
        serialize_u8(v: u8);
        serialize_u16(v: u16);
        serialize_u32(v: u32);
        serialize_u64(v: u64);
        serialize_f32(v: f32);
        serialize_f64(v: f64);
        serialize_char(v: char);
        serialize_str(v: &str);
        serialize_bytes(v: &[u8]);
        serialize_none();
        serialize_unit();
        serialize_unit_struct(name: &'static str);
        serialize_unit_variant(name: &'static str, index: u32, variant: &'static str);
    }

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result {
        v.serialize(self)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Result {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result {
        other_way()
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Wrapped<'o>> {
        let node = self.f.child.map(|c| c());
        let (tag, wrapper) = write::items(self.f);
        if let Some(w) = wrapper {
            write::start(self.out, w, "");
        }
        Ok(Wrapped { items: Items { out: self.out, tag, node, extra: "", mode: self.mode }, wrapper })
    }
    fn serialize_tuple(self, _: usize) -> Result<Impossible<(), Stop>> {
        other_way()
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Impossible<(), Stop>> {
        other_way()
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Impossible<(), Stop>> {
        other_way()
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Fields<'o, 'static>> {
        self.fields()
    }
    fn serialize_struct(self, name: &'static str, _: usize) -> Result<Fields<'o, 'static>> {
        if name == RAW_VALUE {
            return other_way();
        }
        self.fields()
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Impossible<(), Stop>> {
        other_way()
    }
}

/// The items of a list field and the element around them when it is wrapped.
struct Wrapped<'o> {
    items: Items<'o, 'static>,
    wrapper: Option<Tag<'static>>,
}

impl SerializeSeq for Wrapped<'_> {
    type Ok = ();
    type Error = Stop;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result {
        self.items.serialize_element(v)
    }
    fn end(self) -> Result {
        if let Some(w) = self.wrapper {
            write::close(self.items.out, w);
        }
        Ok(())
    }
}

/// The text of an attribute, of an element's text field or of a map key. `Ok(false)` for `null`, which writes
/// nothing.
struct Scalar<'o> {
    out: &'o mut String,
    /// In a CDATA section: unescaped, with `]]>` split. A map key is unescaped too.
    cdata: bool,
}

impl Scalar<'_> {
    /// Writes `v`. It is never `null`.
    fn text(self, v: &str) -> bool {
        if self.cdata {
            write::cdata_text(self.out, v);
        } else {
            self.out.push_str(&write::escape(v));
        }
        true
    }
}

macro_rules! integer_text {
    ($($method:ident: $ty:ty),*) => {$(
        fn $method(self, v: $ty) -> Result<bool> {
            write!(self.out, "{v}").map_err(|_| Stop(None))?;
            Ok(true)
        }
    )*};
}

impl ser::Serializer for Scalar<'_> {
    type Ok = bool;
    type Error = Stop;
    type SerializeSeq = Impossible<bool, Stop>;
    type SerializeTuple = Impossible<bool, Stop>;
    type SerializeTupleStruct = Impossible<bool, Stop>;
    type SerializeTupleVariant = Impossible<bool, Stop>;
    type SerializeMap = Impossible<bool, Stop>;
    type SerializeStruct = Impossible<bool, Stop>;
    type SerializeStructVariant = Impossible<bool, Stop>;

    integer_text!(
        serialize_i8: i8, serialize_i16: i16, serialize_i32: i32, serialize_i64: i64,
        serialize_u8: u8, serialize_u16: u16, serialize_u32: u32, serialize_u64: u64
    );

    fn serialize_bool(self, v: bool) -> Result<bool> {
        Ok(self.text(if v { "true" } else { "false" }))
    }
    fn serialize_f32(self, _: f32) -> Result<bool> {
        other_way()
    }
    fn serialize_f64(self, v: f64) -> Result<bool> {
        float(self.out, v)?;
        Ok(true)
    }
    fn serialize_char(self, v: char) -> Result<bool> {
        Ok(self.text(v.encode_utf8(&mut [0; CHAR_BYTES])))
    }
    fn serialize_str(self, v: &str) -> Result<bool> {
        Ok(self.text(v))
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<bool> {
        other_way()
    }
    fn serialize_none(self) -> Result<bool> {
        Ok(false)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<bool> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<bool> {
        Ok(false)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<bool> {
        Ok(false)
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<bool> {
        Ok(self.text(variant))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Result<bool> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<bool> {
        other_way()
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Impossible<bool, Stop>> {
        other_way()
    }
    fn serialize_tuple(self, _: usize) -> Result<Impossible<bool, Stop>> {
        other_way()
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Impossible<bool, Stop>> {
        other_way()
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Impossible<bool, Stop>> {
        other_way()
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Impossible<bool, Stop>> {
        other_way()
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Impossible<bool, Stop>> {
        other_way()
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Impossible<bool, Stop>> {
        other_way()
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::json;

    #[cfg(feature = "multipart")]
    use super::super::xml_document;
    use super::super::{XmlField, XmlNode, XmlShape, xml_encode, xml_fields};

    const fn field(wire: &'static str) -> XmlField<XmlNode> {
        XmlField {
            wire,
            name: wire,
            prefix: "",
            attribute: false,
            text: false,
            list: false,
            wrapped: false,
            required: false,
            item: wire,
            child: None,
            cdata: false,
            inline: false,
            nullable: false,
            object: false,
        }
    }

    #[derive(Serialize)]
    struct Book {
        year: Option<i64>,
        title: String,
    }

    static BOOK: XmlNode = XmlNode {
        name: "book",
        namespace: "",
        prefix: "",
        fields: &[XmlField { attribute: true, ..field("year") }, field("title")],
    };

    fn book() -> &'static XmlNode {
        &BOOK
    }

    #[derive(Serialize)]
    struct Note {
        id: i64,
        body: String,
    }

    static NOTE: XmlNode = XmlNode {
        name: "note",
        namespace: "",
        prefix: "",
        fields: &[XmlField { attribute: true, ..field("id") }, XmlField { text: true, cdata: true, ..field("body") }],
    };

    fn note() -> &'static XmlNode {
        &NOTE
    }

    static SHELF: XmlNode = XmlNode {
        name: "shelf",
        namespace: "urn:lib",
        prefix: "lib",
        fields: &[
            XmlField { attribute: true, ..field("id") },
            XmlField { attribute: true, prefix: "lib", name: "c", ..field("code") },
            XmlField { text: true, ..field("caption") },
            XmlField { list: true, wrapped: true, item: "book", child: Some(book), ..field("books") },
            XmlField { list: true, prefix: "lib", item: "tag", ..field("tags") },
            XmlField { nullable: true, ..field("owner") },
            XmlField { inline: true, child: Some(note), ..field("held") },
            field("score"),
            field("ratio"),
        ],
    };

    /// The fields in the order of the shape: written as they come.
    #[derive(Serialize)]
    struct Shelf {
        id: i64,
        code: String,
        caption: Option<String>,
        books: Vec<Book>,
        tags: Vec<String>,
        owner: Option<String>,
        score: f64,
    }

    impl XmlShape for Shelf {
        fn xml() -> &'static XmlNode {
            &SHELF
        }
    }

    /// The same fields in another order, an inline field, an `f32`, and a property the shape does not list.
    #[derive(Serialize)]
    struct Mixed {
        score: f64,
        extra: (i64, bool),
        books: Vec<Book>,
        held: Note,
        id: i64,
        ratio: f32,
        caption: Option<String>,
    }

    impl XmlShape for Mixed {
        fn xml() -> &'static XmlNode {
            &SHELF
        }
    }

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn a_shaped_body_writes_attributes_then_text_then_elements() {
        let books =
            vec![Book { year: Some(1965), title: "Dune & <co>".into() }, Book { year: None, title: "\"q\"".into() }];
        let shelf = Shelf {
            id: 7,
            code: "a\"b'c".into(),
            caption: Some("x < y".into()),
            books,
            tags: vec!["sf".into(), "classic".into()],
            owner: None,
            score: 0.1,
        };
        assert_eq!(
            text(xml_encode(&shelf).unwrap()),
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8"?><lib:shelf xmlns:lib="urn:lib" id="7" lib:c="a&quot;b&apos;c">"#,
                r#"x &lt; y<books><book year="1965"><title>Dune &amp; &lt;co&gt;</title></book><book>"#,
                r#"<title>&quot;q&quot;</title></book></books><lib:tag>sf</lib:tag><lib:tag>classic</lib:tag>"#,
                r#"<owner xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/><score>0.1</score>"#,
                "</lib:shelf>"
            )
        );
    }

    #[test]
    fn fields_out_of_order_held_in_place_or_unlisted_write_in_the_order_of_the_shape() {
        let mixed = Mixed {
            score: 1e300,
            extra: (3, true),
            books: vec![Book { year: Some(2001), title: "A".into() }],
            held: Note { id: 9, body: "a ]]> b".into() },
            id: -1,
            ratio: 0.5,
            caption: None,
        };
        assert_eq!(
            text(xml_encode(&mixed).unwrap()),
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8"?><lib:shelf xmlns:lib="urn:lib" id="-1"><books>"#,
                r#"<book year="2001"><title>A</title></book></books><![CDATA[a ]]]]><![CDATA[> b]]>"#,
                "<score>1e+300</score><ratio>0.5</ratio><extra>3</extra><extra>true</extra></lib:shelf>"
            )
        );
    }

    #[test]
    fn a_value_writes_as_a_document_or_as_form_fields() {
        // In the order of their names, which is the order of a `serde_json::Map` with `preserve_order` or without.
        let v = json!({"name": "a&b", "none": null, "tags": ["x", "y"]});
        let fields = concat!(
            "<name>a&amp;b</name>",
            r#"<none xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>"#,
            "<tags>x</tags><tags>y</tags>"
        );
        #[cfg(feature = "multipart")]
        assert_eq!(
            text(xml_document("root", &v)),
            format!(r#"<?xml version="1.0" encoding="UTF-8"?><root>{fields}</root>"#)
        );
        assert_eq!(xml_fields(&v), fields);
        assert_eq!(xml_fields(&json!("plain <text>")), "plain &lt;text&gt;");
    }

    struct Bytes;
    impl Serialize for Bytes {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_bytes(b"ab")
        }
    }
    #[derive(Serialize)]
    struct Unit;
    #[derive(Serialize)]
    struct Wrap(i64);
    #[derive(Serialize)]
    enum Pick {
        A,
        B(i64),
    }
    #[derive(Serialize)]
    struct Kinds {
        flag: bool,
        ratio: f32,
        letter: char,
        bytes: Bytes,
        unit: (),
        marker: Unit,
        pick_a: Pick,
        wrap: Wrap,
        pick_b: Pick,
        some: Option<i64>,
        none: Option<i64>,
        opts: Vec<Option<i64>>,
        wraps: Vec<Wrap>,
        picks: Vec<Pick>,
    }

    /// Each kind of the serde data model, as written fast, is written as its `Value` is: as elements and as
    /// attributes, in the order of the shape and out of it.
    #[test]
    fn every_kind_writes_as_its_value() {
        let kinds = Kinds {
            flag: true,
            ratio: 0.5,
            letter: 'x',
            bytes: Bytes,
            unit: (),
            marker: Unit,
            pick_a: Pick::A,
            wrap: Wrap(2),
            pick_b: Pick::B(3),
            some: Some(4),
            none: None,
            opts: vec![Some(5), None],
            wraps: vec![Wrap(6)],
            picks: vec![Pick::B(7)],
        };
        let value = serde_json::to_value(&kinds).unwrap();
        let nil = r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xsi:nil="true"/>"#;
        // Each field as an element and as an attribute: `null` writes no attribute.
        let pieces: [(&str, String, &str); 14] = [
            ("flag", "<flag>true</flag>".into(), r#" flag="true""#),
            ("ratio", "<ratio>0.5</ratio>".into(), r#" ratio="0.5""#),
            ("letter", "<letter>x</letter>".into(), r#" letter="x""#),
            ("bytes", "<bytes>97</bytes><bytes>98</bytes>".into(), r#" bytes="[97,98]""#),
            ("unit", format!("<unit {nil}"), ""),
            ("marker", format!("<marker {nil}"), ""),
            ("pick_a", "<pick_a>A</pick_a>".into(), r#" pick_a="A""#),
            ("wrap", "<wrap>2</wrap>".into(), r#" wrap="2""#),
            ("pick_b", "<pick_b><B>3</B></pick_b>".into(), r#" pick_b="{&quot;B&quot;:3}""#),
            ("some", "<some>4</some>".into(), r#" some="4""#),
            ("none", format!("<none {nil}"), ""),
            ("opts", format!("<opts>5</opts><opts {nil}"), r#" opts="[5,null]""#),
            ("wraps", "<wraps>6</wraps>".into(), r#" wraps="[6]""#),
            ("picks", "<picks><B>7</B></picks>".into(), r#" picks="[{&quot;B&quot;:7}]""#),
        ];
        for attribute in [false, true] {
            for reversed in [false, true] {
                let mut fields: Vec<XmlField<XmlNode>> =
                    pieces.iter().map(|p| XmlField { attribute, ..field(p.0) }).collect();
                if reversed {
                    fields.reverse();
                }
                let node: &'static XmlNode =
                    Box::leak(Box::new(XmlNode { name: "k", namespace: "", prefix: "", fields: fields.leak() }));
                let mut written: Vec<&(&str, String, &str)> = pieces.iter().collect();
                if reversed {
                    written.reverse();
                }
                let (start, inside): (String, String) = if attribute {
                    (written.iter().map(|p| p.2).collect(), String::new())
                } else {
                    (String::new(), written.iter().map(|p| p.1.as_str()).collect())
                };
                let fast = super::body(&kinds, node).unwrap();
                assert_eq!(fast, format!(r#"<?xml version="1.0" encoding="UTF-8"?><k{start}>{inside}</k>"#));
                assert_eq!(fast, super::body(&value, node).unwrap(), "attribute {attribute}, reversed {reversed}");
            }
        }
    }

    /// A field that comes before one that the shape lists earlier, and an attribute after an element, stop the
    /// streamed writing: the element is written again in the order of the shape.
    #[test]
    fn a_field_out_of_order_is_written_in_the_order_of_the_shape() {
        #[derive(Serialize)]
        struct Pair {
            b: String,
            a: i64,
        }
        let pair = Pair { b: "x".into(), a: 1 };
        let written = |fields: Vec<XmlField<XmlNode>>| {
            let node = XmlNode { name: "k", namespace: "", prefix: "", fields: fields.leak() };
            let node: &'static XmlNode = Box::leak(Box::new(node));
            super::body(&pair, node).unwrap()
        };
        let declared = r#"<?xml version="1.0" encoding="UTF-8"?>"#;
        assert_eq!(written(vec![field("a"), field("b")]), format!("{declared}<k><a>1</a><b>x</b></k>"));
        let attribute = vec![XmlField { attribute: true, ..field("a") }, field("b")];
        assert_eq!(written(attribute), format!(r#"{declared}<k a="1"><b>x</b></k>"#));
    }
}
