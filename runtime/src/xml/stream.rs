//! An XML body decoded straight from the events of the parser into its type. It skips the values that
//! [`super::parse`] builds for every element. It handles the shapes that most bodies have: attributes, text,
//! elements and lists whose items come in a row.
//!
//! Anything else makes it give up. [`super::xml_decode`] then decodes the body through those values. They also say
//! what is wrong with a body that does not decode. Examples are an element that repeats apart from the first of its
//! name, a `nodeType: none` object, a leaf with attributes or children and a `nil` where no `null` fits. So the two
//! never disagree on a body. A leaf reads through the same [`XmlDe`] either way.

use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use serde::de::value::StrDeserializer;
use serde::de::{
    DeserializeOwned, DeserializeSeed, Deserializer, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor,
};

use super::XmlDe;
use crate::coerce::CoerceError;
use std::borrow::Cow;

use crate::xml_tree::name::local;

/// Returns the name of an element without its prefix, borrowed from it.
fn name_of<'n>(start: &'n BytesStart<'_>) -> &'n str {
    start.local_name().into_inner()
}
use crate::xml_tree::{MAX_DEPTH, Xml, XmlField, XmlNode};

/// Returns the value of the body or `None` where the stream gives up.
pub(super) fn decode<T: DeserializeOwned>(text: &str, root: &'static XmlNode) -> Option<T> {
    let mut s = Stream { reader: Reader::from_str(text), peeked: None, depth: 0 };
    loop {
        match s.next().ok()? {
            Ev::Start(start, empty) => {
                return T::deserialize(Element { s: &mut s, start, empty, role: Role::Shaped(root) }).ok();
            }
            Ev::Text(..) => {}
            Ev::End | Ev::Eof => return None,
        }
    }
}

/// Returns whether `name` is an element that a `nodeType: none` field of `node` holds in place, here or deeper. The
/// tree decodes those.
fn inline_element(node: &XmlNode, name: &str) -> bool {
    node.fields.iter().filter(|f| f.inline).any(|f| {
        f.name == name
            || f.child.is_some_and(|c| {
                let c = c();
                c.fields.iter().any(|g| g.is_element() && (g.name == name || g.item == name)) || inline_element(c, name)
            })
    })
}

/// Returns whether a `nodeType: none` field of `node` takes attributes or text of the element itself. Only the tree
/// reads those.
fn inline_takes_parent(node: &XmlNode) -> bool {
    node.fields
        .iter()
        .filter(|f| f.inline)
        .filter_map(|f| f.child)
        .map(|c| c())
        .any(|c| c.fields.iter().any(|g| g.attribute || g.text) || inline_takes_parent(c))
}

/// Returns the error that makes the stream give up: the body is decoded through the tree instead.
fn give_up() -> CoerceError {
    CoerceError::custom("decoded through the tree")
}

type Result<T> = std::result::Result<T, CoerceError>;

/// What decoding needs of a parser event. CDATA is text with `cdata` set.
enum Ev<'a> {
    /// An element opens. `true` when it is empty (`<a/>`). Then no end follows.
    Start(BytesStart<'a>, bool),
    End,
    /// Borrowed from the body when it holds no entity.
    Text(Cow<'a, str>, bool),
    Eof,
}

/// The parser with one event of look-ahead and how deep it is.
struct Stream<'a> {
    reader: Reader<&'a [u8]>,
    peeked: Option<Ev<'a>>,
    depth: usize,
}

impl<'a> Stream<'a> {
    /// Returns the next event decoding reads. Comments, processing instructions, declarations and DOCTYPEs are skipped.
    fn next(&mut self) -> Result<Ev<'a>> {
        if let Some(e) = self.peeked.take() {
            return Ok(e);
        }
        loop {
            return Ok(match self.reader.read_event().map_err(CoerceError::custom)? {
                Event::Start(s) => {
                    self.depth += 1;
                    if self.depth > MAX_DEPTH {
                        return Err(give_up());
                    }
                    Ev::Start(s, false)
                }
                Event::Empty(s) => Ev::Start(s, true),
                Event::End(_) => {
                    self.depth = self.depth.saturating_sub(1);
                    Ev::End
                }
                // XML 1.0 turns a `\r\n` into `\n`. Text without a `\r` is the body as it is.
                Event::Text(t) if t.contains('\r') => {
                    Ev::Text(Cow::Owned(t.xml_content(XmlVersion::Implicit1_0).into_owned()), false)
                }
                Event::Text(t) => Ev::Text(t.into_inner(), false),
                Event::CData(c) if c.contains('\r') => {
                    Ev::Text(Cow::Owned(c.xml_content(XmlVersion::Implicit1_0).into_owned()), true)
                }
                Event::CData(c) => Ev::Text(c.into_inner(), true),
                Event::GeneralRef(r) => {
                    let name = r.xml_content(XmlVersion::Implicit1_0);
                    // An entity a DTD would define stays as it is written.
                    let text = match r.resolve_char_ref().map_err(CoerceError::custom)? {
                        Some(c) => c.to_string(),
                        None => resolve_predefined_entity(&name).map_or_else(|| format!("&{name};"), str::to_owned),
                    };
                    Ev::Text(Cow::Owned(text), false)
                }
                Event::Eof => Ev::Eof,
                Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => continue,
            });
        }
    }

    fn peek(&mut self) -> Result<&Ev<'a>> {
        if self.peeked.is_none() {
            self.peeked = Some(self.next()?);
        }
        self.peeked.as_ref().ok_or_else(give_up)
    }

    /// Reads past the end of an element that just opened.
    fn skip(&mut self, empty: bool) -> Result<()> {
        if empty {
            return Ok(());
        }
        let mut open = 0_usize;
        loop {
            match self.next()? {
                Ev::Start(_, false) => open += 1,
                Ev::End if open == 0 => return Ok(()),
                Ev::End => open -= 1,
                Ev::Eof => return Err(give_up()),
                Ev::Start(_, true) | Ev::Text(..) => {}
            }
        }
    }

    /// Returns the text of an element that just opened up to its end. It gives up at a child element.
    fn text(&mut self, empty: bool) -> Result<Cow<'a, str>> {
        let mut out = Cow::Borrowed("");
        if empty {
            return Ok(out);
        }
        loop {
            match self.next()? {
                // One piece is the text as it came, borrowed from the body. Only text in several pieces is joined.
                Ev::Text(t, _) if out.is_empty() => out = t,
                Ev::Text(t, _) => out.to_mut().push_str(&t),
                Ev::End => return Ok(out),
                Ev::Start(..) | Ev::Eof => return Err(give_up()),
            }
        }
    }
}

/// Returns whether an element is `xsi:nil="true"` and its attributes by local name. `xmlns` declarations are left out.
fn attributes(start: &BytesStart<'_>) -> Result<(bool, Vec<(String, String)>)> {
    let (mut nil, mut out) = (false, vec![]);
    for a in start.attributes() {
        let a = a.map_err(CoerceError::custom)?;
        if a.key.as_ref().starts_with("xmlns") {
            continue;
        }
        let value = a.normalized_value(XmlVersion::Implicit1_0).map_err(CoerceError::custom)?;
        let name = local(a.key);
        if name == "nil" && value == "true" {
            nil = true;
        } else {
            out.push((name, value.into_owned()));
        }
    }
    Ok((nil, out))
}

/// What an element becomes from the shape of its parent.
#[derive(Clone, Copy)]
enum Role {
    /// An element its schema describes.
    Shaped(&'static XmlNode),
    /// Text. That is a property without a shape of its own, an item or an element that no schema describes. The
    /// empty element of a free-form object is `{}`.
    Text { object: bool },
    /// The wrapping element of a `wrapped` list.
    Wrapper(&'static XmlField<XmlNode>),
}

/// Returns the role of an item of a list field.
fn item_role(f: &XmlField<XmlNode>) -> Role {
    f.child.map_or(Role::Text { object: false }, |c| Role::Shaped(c()))
}

/// One open element. It reads up to its end.
struct Element<'s, 'a> {
    s: &'s mut Stream<'a>,
    start: BytesStart<'a>,
    empty: bool,
    role: Role,
}

impl<'a> Element<'_, 'a> {
    /// Returns the text of a leaf without a copy, or the value that the tree holds for one that is not text: `null`
    /// when `nil` and `{}` for the empty element of a free-form object. Attributes or children make it an object. The
    /// tree builds that.
    fn leaf_text(self, object: bool) -> Result<std::result::Result<Cow<'a, str>, Xml>> {
        let (nil, attrs) = attributes(&self.start)?;
        let text = self.s.text(self.empty)?;
        if nil {
            return Ok(Err(Xml::Null));
        }
        if !attrs.is_empty() {
            return Err(give_up());
        }
        Ok(if object && text.trim().is_empty() { Err(Xml::Obj(Vec::new())) } else { Ok(text) })
    }

    /// Returns the value of a leaf as the tree would hold it.
    fn leaf(self, object: bool) -> Result<Xml> {
        Ok(match self.leaf_text(object)? {
            Ok(text) => Xml::Text(text.into_owned()),
            Err(value) => value,
        })
    }
}

/// A leaf method. The text goes through [`XmlDe`] as the tree reads it. The tree takes an element of another role.
macro_rules! leaf {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
            match self.role {
                Role::Text { object } => XmlDe(self.leaf(object)?).$method(v),
                Role::Shaped(_) | Role::Wrapper(_) => Err(give_up()),
            }
        }
    )*};
}

/// A number leaf read from its text without a copy, as [`crate::coerce::Coerce`] reads text.
macro_rules! number_leaf {
    ($method:ident, $visit:ident, $ty:ty, $format:literal, $ok:expr) => {
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
            match self.role {
                Role::Text { object } => match self.leaf_text(object)? {
                    Ok(text) => match crate::coerce::number::<$ty>(&text, $ok) {
                        Some(n) => v.$visit(n),
                        None => Err(crate::coerce::not_a_number(&text, $format)),
                    },
                    Err(value) => XmlDe(value).$method(v),
                },
                Role::Shaped(_) | Role::Wrapper(_) => Err(give_up()),
            }
        }
    };
}

impl<'de> Deserializer<'de> for Element<'_, '_> {
    type Error = CoerceError;

    crate::coerce::numbers!(number_leaf);

    fn deserialize_bool<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        match self.role {
            Role::Text { object } => match self.leaf_text(object)? {
                Ok(text) => match crate::coerce::boolean(&text) {
                    Some(b) => v.visit_bool(b),
                    None => Err(crate::coerce::not_a_boolean(&text)),
                },
                Err(value) => XmlDe(value).deserialize_bool(v),
            },
            Role::Shaped(_) | Role::Wrapper(_) => Err(give_up()),
        }
    }

    leaf!(
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_unit,
        deserialize_identifier
    );

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        match self.role {
            Role::Shaped(_) => self.deserialize_map(v),
            Role::Wrapper(_) => self.deserialize_seq(v),
            Role::Text { object } => XmlDe(self.leaf(object)?).deserialize_any(v),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        if attributes(&self.start)?.0 {
            self.s.skip(self.empty)?;
            return v.visit_none();
        }
        match self.role {
            Role::Text { object } => XmlDe(self.leaf(object)?).deserialize_option(v),
            Role::Shaped(_) | Role::Wrapper(_) => v.visit_some(self),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value> {
        match self.role {
            Role::Text { object } => XmlDe(self.leaf(object)?).deserialize_unit_struct(name, v),
            Role::Shaped(_) | Role::Wrapper(_) => Err(give_up()),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value> {
        match self.role {
            Role::Text { object } => XmlDe(self.leaf(object)?).deserialize_newtype_struct(name, v),
            Role::Shaped(_) | Role::Wrapper(_) => v.visit_newtype_struct(self),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        match self.role {
            Role::Wrapper(f) if !attributes(&self.start)?.0 => v.visit_seq(Wrapped { s: self.s, f, done: self.empty }),
            Role::Text { object } => XmlDe(self.leaf(object)?).deserialize_seq(v),
            Role::Wrapper(_) | Role::Shaped(_) => Err(give_up()),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, v: V) -> Result<V::Value> {
        self.deserialize_seq(v)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _n: &'static str, _len: usize, v: V) -> Result<V::Value> {
        self.deserialize_seq(v)
    }

    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        match self.role {
            Role::Shaped(node) => {
                let (nil, attrs) = attributes(&self.start)?;
                // The tree takes `nil` where no `null` fits, and a `nodeType: none` object that holds attributes or
                // text of this element. Its child elements give up only where they appear: most bodies have none.
                if nil || inline_takes_parent(node) {
                    return Err(give_up());
                }
                let mut queue = vec![];
                for f in node.fields.iter().filter(|f| f.attribute) {
                    // 3.2 "Handling `null` Values": an omitted attribute is `null` where the schema allows it.
                    match attrs.iter().find(|(k, _)| k == f.name) {
                        Some((_, value)) => queue.push((f.wire, Xml::Text(value.clone()))),
                        None if f.nullable => queue.push((f.wire, Xml::Null)),
                        None => {}
                    }
                }
                queue.reverse();
                let fields = Fields {
                    s: self.s,
                    node,
                    queue,
                    done: self.empty,
                    ended: false,
                    text: String::new(),
                    cdata: None,
                    seen: Seen::default(),
                    pending: None,
                };
                v.visit_map(fields)
            }
            Role::Text { object } => XmlDe(self.leaf(object)?).deserialize_map(v),
            Role::Wrapper(_) => Err(give_up()),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _n: &'static str,
        _f: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        self.deserialize_map(v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value> {
        match self.role {
            Role::Text { object } => XmlDe(self.leaf(object)?).deserialize_enum(name, variants, v),
            Role::Shaped(_) | Role::Wrapper(_) => Err(give_up()),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        self.s.skip(self.empty)?;
        v.visit_unit()
    }
}

/// The value of a property still to read. That is one that the element held, a child element or the items of an
/// unwrapped list.
enum Pending<'a> {
    Value(Xml),
    Element(BytesStart<'a>, bool, Role),
    List(&'static XmlField<XmlNode>, BytesStart<'a>, bool),
}

/// The properties of an element that its schema describes. First its attributes and then its children as they
/// come. Then its text and the required lists that it did not hold.
struct Fields<'s, 'a> {
    s: &'s mut Stream<'a>,
    node: &'static XmlNode,
    /// Properties known before the next child with the last one first.
    queue: Vec<(&'static str, Xml)>,
    /// The element's end was read (or it was empty).
    done: bool,
    /// The properties its end decides were queued.
    ended: bool,
    text: String,
    /// The CDATA sections alone for a `nodeType: cdata` field.
    cdata: Option<String>,
    /// Every property given so far. A second one with a name is a repeat apart. The tree joins it.
    seen: Seen,
    pending: Option<Pending<'a>>,
}

/// The properties that an element gave so far, without allocating for those that its shape lists: one bit per field
/// of the shape. The names of elements that it does not list are kept apart.
#[derive(Default)]
struct Seen {
    fields: u128,
    other: std::collections::BTreeSet<String>,
}

impl Seen {
    /// Returns the bit of the field of `node` whose name on the wire is `key`.
    fn bit(node: &XmlNode, key: &str) -> Option<u128> {
        let i = node.fields.iter().position(|f| f.wire == key)?;
        u32::try_from(i).ok().filter(|i| *i < u128::BITS).map(|i| 1 << i)
    }

    fn contains(&self, node: &XmlNode, key: &str) -> bool {
        match Self::bit(node, key) {
            Some(bit) => self.fields & bit != 0,
            None => self.other.contains(key),
        }
    }

    fn insert(&mut self, node: &XmlNode, key: Cow<'static, str>) {
        match Self::bit(node, &key) {
            Some(bit) => self.fields |= bit,
            None => {
                self.other.insert(key.into_owned());
            }
        }
    }
}

impl<'a> Fields<'_, 'a> {
    /// Returns the property that a child element is and how to read it.
    fn child(&self, start: BytesStart<'a>, empty: bool) -> (Cow<'static, str>, Pending<'a>) {
        let name = name_of(&start);
        for f in self.node.fields.iter().filter(|f| f.is_element()) {
            if f.list && !f.wrapped && f.item == name {
                return (Cow::Borrowed(f.wire), Pending::List(f, start, empty));
            }
            if f.name == name && (!f.list || f.wrapped) {
                let role = match (f.list, f.child) {
                    (true, _) => Role::Wrapper(f),
                    (false, Some(c)) => Role::Shaped(c()),
                    (false, None) => Role::Text { object: f.object },
                };
                return (Cow::Borrowed(f.wire), Pending::Element(start, empty, role));
            }
        }
        (Cow::Owned(name.to_owned()), Pending::Element(start, empty, Role::Text { object: false }))
    }

    /// Queues what the end of the element decides. That is its text and the required unwrapped lists that it did not
    /// hold.
    fn end(&mut self) {
        let mut text = Some(std::mem::take(&mut self.text));
        for f in self.node.fields {
            if f.text {
                // No text. An optional text field stays absent. A required one is empty.
                let t = text.take().unwrap_or_default();
                let t = if f.cdata { self.cdata.take().unwrap_or(t) } else { t };
                if f.required || !t.is_empty() {
                    self.queue.push((f.wire, Xml::Text(t)));
                }
            } else if f.list && !f.wrapped && f.required && !self.seen.contains(self.node, f.wire) {
                self.queue.push((f.wire, Xml::List(Vec::new())));
            }
        }
    }
}

impl<'de> MapAccess<'de> for Fields<'_, '_> {
    type Error = CoerceError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        loop {
            if let Some((key, v)) = self.queue.pop() {
                if self.seen.contains(self.node, key) {
                    return Err(give_up());
                }
                self.seen.insert(self.node, Cow::Borrowed(key));
                self.pending = Some(Pending::Value(v));
                return seed.deserialize(StrDeserializer::<CoerceError>::new(key)).map(Some);
            }
            if self.done {
                if self.ended {
                    return Ok(None);
                }
                self.ended = true;
                self.end();
                continue;
            }
            match self.s.next()? {
                Ev::Text(t, cdata) => {
                    if cdata {
                        self.cdata.get_or_insert_default().push_str(&t);
                    }
                    self.text.push_str(&t);
                }
                Ev::End => self.done = true,
                Ev::Eof => return Err(give_up()),
                Ev::Start(start, empty) => {
                    if inline_element(self.node, name_of(&start)) {
                        return Err(give_up());
                    }
                    let (key, pending) = self.child(start, empty);
                    if self.seen.contains(self.node, &key) {
                        return Err(give_up());
                    }
                    let k = seed.deserialize(StrDeserializer::<CoerceError>::new(&key))?;
                    self.seen.insert(self.node, key);
                    self.pending = Some(pending);
                    return Ok(Some(k));
                }
            }
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        match self.pending.take().ok_or_else(give_up)? {
            Pending::Value(v) => seed.deserialize(XmlDe(v)),
            Pending::Element(start, empty, role) => seed.deserialize(Element { s: self.s, start, empty, role }),
            Pending::List(f, start, empty) => {
                seed.deserialize(Items { s: self.s, f, first: Some((start, empty)), text: &mut self.text })
            }
        }
    }
}

/// The items of an unwrapped list: its first element and those of its name right after it. Text between them is
/// the parent's.
struct Items<'s, 'a, 't> {
    s: &'s mut Stream<'a>,
    f: &'static XmlField<XmlNode>,
    first: Option<(BytesStart<'a>, bool)>,
    text: &'t mut String,
}

impl<'de> Deserializer<'de> for Items<'_, '_, '_> {
    type Error = CoerceError;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        v.visit_seq(self)
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value> {
        v.visit_some(self)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(mut self, v: V) -> Result<V::Value> {
        while self.next_element::<IgnoredAny>()?.is_some() {}
        v.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf unit unit_struct
        newtype_struct seq tuple tuple_struct map struct enum identifier
    }
}

impl<'de> SeqAccess<'de> for Items<'_, '_, '_> {
    type Error = CoerceError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        let role = item_role(self.f);
        if let Some((start, empty)) = self.first.take() {
            return seed.deserialize(Element { s: self.s, start, empty, role }).map(Some);
        }
        loop {
            match self.s.peek()? {
                Ev::Text(..) => {
                    if let Ev::Text(t, _) = self.s.next()? {
                        self.text.push_str(&t);
                    }
                }
                Ev::Start(start, _) if name_of(start) == self.f.item => {
                    let Ev::Start(start, empty) = self.s.next()? else { return Err(give_up()) };
                    return seed.deserialize(Element { s: self.s, start, empty, role }).map(Some);
                }
                Ev::Start(..) | Ev::End | Ev::Eof => return Ok(None),
            }
        }
    }
}

/// The items of a wrapped list. That is the children of the wrapper with the name of the item. Others are left out as
/// the tree does.
struct Wrapped<'s, 'a> {
    s: &'s mut Stream<'a>,
    f: &'static XmlField<XmlNode>,
    done: bool,
}

impl<'de> SeqAccess<'de> for Wrapped<'_, '_> {
    type Error = CoerceError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        while !self.done {
            match self.s.next()? {
                Ev::Start(start, empty) if name_of(&start) == self.f.item => {
                    let role = item_role(self.f);
                    return seed.deserialize(Element { s: self.s, start, empty, role }).map(Some);
                }
                Ev::Start(_, empty) => self.s.skip(empty)?,
                Ev::End => self.done = true,
                Ev::Eof => return Err(give_up()),
                Ev::Text(..) => {}
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::super::XmlDe;
    use super::decode;
    use crate::xml_tree::{Role as TreeRole, XmlField, XmlNode, parse};

    /// A field as the generator writes one. It has the same name in XML and JSON.
    const fn field(name: &'static str) -> XmlField<XmlNode> {
        XmlField {
            wire: name,
            name,
            prefix: "",
            attribute: false,
            text: false,
            list: false,
            wrapped: false,
            required: false,
            item: name,
            child: None,
            cdata: false,
            inline: false,
            nullable: false,
            object: false,
        }
    }

    static LINE: XmlNode = XmlNode { name: "line", namespace: "", prefix: "", fields: &[field("sku"), field("qty")] };

    fn line() -> &'static XmlNode {
        &LINE
    }

    static ORDER: XmlNode = XmlNode {
        name: "order",
        namespace: "",
        prefix: "",
        fields: &[
            XmlField { attribute: true, ..field("id") },
            XmlField { list: true, item: "line", child: Some(line), ..field("lines") },
            XmlField { list: true, wrapped: true, item: "tag", ..field("tags") },
            field("note"),
        ],
    };

    #[derive(Debug, PartialEq, Deserialize)]
    struct Line {
        sku: String,
        qty: i32,
    }

    #[derive(Debug, PartialEq, Deserialize)]
    struct Order {
        id: i64,
        #[serde(default)]
        lines: Vec<Line>,
        #[serde(default)]
        tags: Vec<String>,
        note: Option<String>,
    }

    /// The same body through the converted elements. `xml_decode` falls back to this.
    fn through_tree(text: &str) -> Order {
        Order::deserialize(XmlDe(parse(text, TreeRole::Shaped(&ORDER)).unwrap())).unwrap()
    }

    #[test]
    fn a_common_body_decodes_from_the_events_as_the_tree_reads_it() {
        let text = r#"<?xml version="1.0"?>
            <order id="7">
              <line><sku>A</sku><qty>2</qty></line>
              <line><sku>B&amp;C</sku><qty>3</qty></line>
              <tags><tag>x</tag><other/><tag>y</tag></tags>
              <unknown><deep>skipped</deep></unknown>
            </order>"#;
        let streamed: Order = decode(text, &ORDER).expect("no give-up on a common shape");
        assert_eq!(streamed.lines.len(), 2);
        assert_eq!(streamed.lines[1].sku, "B&C");
        assert_eq!(streamed.tags, ["x", "y"]);
        assert_eq!(streamed, through_tree(text));
    }

    #[test]
    fn a_repeat_apart_from_its_first_is_the_tree_s() {
        let text = "<order id=\"1\"><line><sku>A</sku><qty>1</qty></line><note>n</note><line><sku>B</sku><qty>2</qty></line></order>";
        assert_eq!(decode::<Order>(text, &ORDER), None);
        assert_eq!(through_tree(text).lines.len(), 2, "the tree joins them");
    }

    #[test]
    fn nil_and_empty_elements() {
        let text = r#"<order id="2" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><note xsi:nil="true"/><tags/></order>"#;
        let streamed: Order = decode(text, &ORDER).expect("streamed");
        assert_eq!(streamed, Order { id: 2, lines: vec![], tags: vec![], note: None });
        assert_eq!(streamed, through_tree(text));
    }
}
