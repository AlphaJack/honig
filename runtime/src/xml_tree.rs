//! XML shaped by the `xml` objects of the schemas: the shapes, the encoder and the parser into elements.

use std::borrow::Cow;
use std::collections::BTreeMap;

use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::ClientError;
use crate::json::push_multi;

mod encode;
pub(crate) mod name;
mod write;

use self::name::local;

/// How a type maps to XML: generated from the schemas' `xml` objects.
pub trait XmlShape {
    /// Returns the element description of the type.
    fn xml() -> &'static XmlNode;
}

/// An element: its name and how each property maps.
#[derive(Debug)]
pub struct XmlNode {
    /// Element name.
    pub name: &'static str,
    /// Namespace URI, or empty.
    pub namespace: &'static str,
    /// Namespace prefix, or empty.
    pub prefix: &'static str,
    /// One entry per property.
    pub fields: &'static [XmlField<Self>],
}

/// How one property maps to XML. `N` is the [`XmlNode`] of an object value.
#[derive(Debug)]
pub struct XmlField<N: 'static> {
    /// Property name in the JSON model.
    pub wire: &'static str,
    /// Element or attribute name.
    pub name: &'static str,
    /// Namespace prefix, or empty.
    pub prefix: &'static str,
    /// An attribute of the parent element.
    pub attribute: bool,
    /// The text content of the parent element.
    pub text: bool,
    /// An array: one element per item.
    pub list: bool,
    /// An array inside one wrapping element.
    pub wrapped: bool,
    /// The property is required: an absent unwrapped list is empty.
    pub required: bool,
    /// Element name of each array item.
    pub item: &'static str,
    /// The shape of an object (or object item) value.
    pub child: Option<fn() -> &'static N>,
    /// The text is written as a CDATA section (3.2 `nodeType: cdata`).
    pub cdata: bool,
    /// No element of its own: an object's fields sit in the parent (3.2 `nodeType: none`).
    pub inline: bool,
    /// The property may be `null`: an omitted attribute decodes as `null` (3.2 "Handling `null` Values").
    pub nullable: bool,
    /// A free-form object (a map): an empty element decodes as `{}`.
    pub object: bool,
}

impl<N> XmlField<N> {
    /// Returns the property `wire` as an element of that name with every other choice off: the start of
    /// `XmlField { attribute: true, ..XmlField::new("id") }`.
    #[must_use]
    pub const fn new(wire: &'static str) -> Self {
        Self {
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

    /// Returns a child element of its own or the items of one. It is not an attribute and not the text of the parent.
    pub(crate) const fn is_element(&self) -> bool {
        !self.attribute && !self.text
    }
}

/// Returns whether `fields` lists the member `key`.
fn lists(fields: &[XmlField<XmlNode>], key: &str) -> bool {
    fields.iter().any(|f| f.wire == key)
}

/// Encodes an XML body. `null` is written as `xsi:nil="true"`.
/// Errors: the value does not serialize.
pub fn xml_encode<T: Serialize + XmlShape>(v: &T) -> Result<Vec<u8>, ClientError> {
    Ok(encode::body(v, T::xml())?.into_bytes())
}

/// Returns a form field sent as `application/xml`. In 3.2 "Parsing and Serializing" `extra: {info: abc}` becomes
/// `extra=<info>abc</info>`. The properties of an object are elements. The field itself gets no element. Its name is
/// the form key. Any other value is its escaped text.
pub(crate) fn xml_fields(v: &Value) -> String {
    encode::fields(v)
}

/// Returns a multipart part declared `application/xml` that contains an object or a list. An example is `addresses` as
/// `application/xml; charset=utf-8` in 3.2 "Multipart Form with Encoding Objects". It is one document. Its root has
/// the name of the part.
#[cfg(feature = "multipart")]
pub(crate) fn xml_document(root: &str, v: &Value) -> Vec<u8> {
    encode::document(root, v).into_bytes()
}

/// Returns the reverse of [`xml_document`]. The children of the root become an object whatever the name of the root is.
pub(crate) fn xml_document_value(bytes: &[u8]) -> Result<Value, String> {
    parse(std::str::from_utf8(bytes).map_err(|e| e.to_string())?, Role::Unshaped).map(Xml::into_value)
}

/// Returns the reverse of [`xml_fields`]. Elements become the properties of an object. Plain text becomes a string.
pub(crate) fn xml_fields_value(text: &str) -> Result<Value, String> {
    parse(&format!("<fields>{text}</fields>"), Role::Unshaped).map(Xml::into_value)
}

/// What an element becomes once it closes. The shape of its parent tells this when it opens. The decoder converts
/// each element as it closes. So the body is never stored as a tree of raw elements and as its value too.
#[derive(Clone, Copy)]
pub(crate) enum Role {
    /// An element its schema describes.
    Shaped(&'static XmlNode),
    /// An element that no schema describes. Its text or its attributes and children by name.
    Unshaped,
    /// The wrapping element of a `wrapped` list: its `item` children are the list.
    Wrapper(&'static XmlField<XmlNode>),
}

/// An element still open: what it has parsed so far. Names are local (prefixes dropped).
struct Open {
    name: String,
    role: Role,
    attrs: Vec<(String, String)>,
    /// Its children. Each one is already converted.
    children: Vec<(String, Xml)>,
    text: String,
    /// The CDATA sections alone for a `nodeType: cdata` field. The text around them is not kept. That text is the
    /// formatting whitespace of a pretty-printed document.
    cdata: Option<String>,
    nil: bool,
}

/// Returns the role of a child named `name`. It comes from the shape of its parent and the children that it already
/// contains. A field gets the first element of its name because fields go in document order. A second one is an element
/// that no schema describes.
fn role_in(node: &'static XmlNode, name: &str, held: &[(String, Xml)]) -> Role {
    let shaped = |f: &XmlField<XmlNode>| f.child.map_or(Role::Unshaped, |c| Role::Shaped(c()));
    let taken = || held.iter().any(|(n, _)| n == name);
    for f in node.fields {
        if f.attribute || f.text {
            continue;
        }
        if f.inline {
            // `nodeType: none`: the object's fields are this element's children.
            if let Some(inner) = f.child.map(|c| c())
                && inner.fields.iter().any(|g| [g.name, g.item].contains(&name))
            {
                return role_in(inner, name, held);
            }
            continue;
        }
        if f.list && !f.wrapped {
            if f.item == name {
                return shaped(f);
            }
        } else if f.name == name {
            if taken() {
                return Role::Unshaped;
            }
            return if f.list { Role::Wrapper(f) } else { shaped(f) };
        }
    }
    Role::Unshaped
}

/// Returns the role of a child named `name` of an open element.
fn child_role(parent: &Open, name: &str) -> Role {
    match parent.role {
        Role::Shaped(node) => role_in(node, name, &parent.children),
        Role::Unshaped => Role::Unshaped,
        Role::Wrapper(f) => f.child.map_or(Role::Unshaped, |c| Role::Shaped(c())),
    }
}

/// Returns the value of an element that just closed.
fn close(e: Open) -> Xml {
    let Open { role, attrs, children, text, cdata, nil, .. } = e;
    if nil {
        return Xml::Null;
    }
    match role {
        Role::Shaped(node) => shaped(node, attrs, children.into_iter().map(Some).collect(), text, cdata.as_deref()),
        Role::Unshaped => unshaped(attrs, children, text),
        Role::Wrapper(f) => Xml::List(children.into_iter().filter(|(n, _)| n == f.item).map(|(_, v)| v).collect()),
    }
}

/// Returns an element that no schema describes. Its text or an object of its attributes and children by name.
fn unshaped(attrs: Vec<(String, String)>, children: Vec<(String, Xml)>, text: String) -> Xml {
    if children.is_empty() && attrs.is_empty() {
        return Xml::Text(text);
    }
    let mut o = Vec::with_capacity(attrs.len() + children.len());
    let attrs = attrs.into_iter().map(|(k, v)| (Cow::Owned(k), Xml::Text(v)));
    push_named(&mut o, attrs.chain(children.into_iter().map(|(name, v)| (Cow::Owned(name), v))));
    Xml::Obj(o)
}

/// Returns the value of an attribute field from the attributes of the element. In 3.2 "Handling `null` Values" an
/// omitted attribute is `null` where the schema allows it.
fn attribute_value(attrs: &mut [(String, String)], f: &XmlField<XmlNode>) -> Option<Xml> {
    match attrs.iter_mut().find(|(k, _)| k == f.name) {
        Some((_, v)) => Some(Xml::Text(std::mem::take(v))),
        None => f.nullable.then_some(Xml::Null),
    }
}

/// Returns the value of a child element for its field. `<related></related>` for a free-form object is `{}` and not
/// `""`.
fn element_value(v: Xml, f: &XmlField<XmlNode>) -> Xml {
    if f.object && matches!(&v, Xml::Text(s) if s.trim().is_empty()) { Xml::Obj(Vec::new()) } else { v }
}

/// Returns the JSON model of an element that its schema describes. It comes from its converted children. A child leaves
/// the list when a field claims it. The ones left over are kept by name.
fn shaped(
    node: &XmlNode,
    mut attrs: Vec<(String, String)>,
    mut children: Vec<Option<(String, Xml)>>,
    text: String,
    cdata: Option<&str>,
) -> Xml {
    let mut text = Some(text);
    let mut o: Vec<(Cow<'static, str>, Xml)> = Vec::with_capacity(node.fields.len());
    let named = |c: &Option<(String, Xml)>, own: &str| c.as_ref().is_some_and(|(n, _)| n == own);
    for f in node.fields {
        if f.attribute {
            if let Some(v) = attribute_value(&mut attrs, f) {
                o.push((Cow::Borrowed(f.wire), v));
            }
            continue;
        }
        if f.text {
            // No text. An optional text field stays absent. A required one is empty.
            // A CDATA field is the content of its sections without the formatting around them.
            let t = text.take().map(|t| if f.cdata { cdata.map_or(t, str::to_owned) } else { t }).unwrap_or_default();
            if f.required || !t.is_empty() {
                o.push((Cow::Borrowed(f.wire), Xml::Text(t)));
            }
            continue;
        }
        // `nodeType: none`: the object's fields are this element's children.
        if f.inline
            && let Some(inner) = f.child.map(|c| c())
        {
            let belongs = |c: &Option<(String, Xml)>| {
                c.as_ref().is_some_and(|(n, _)| inner.fields.iter().any(|g| [g.name, g.item].contains(&n.as_str())))
            };
            let taken: Vec<Option<(String, Xml)>> =
                children.iter_mut().filter(|c| belongs(c)).map(Option::take).collect();
            // An inline object exists only through its fields.
            if !taken.is_empty() {
                let all = shaped(inner, attrs.clone(), taken, text.clone().unwrap_or_default(), cdata);
                let own = match all {
                    Xml::Obj(all) => all.into_iter().filter(|(k, _)| lists(inner.fields, k)).collect(),
                    Xml::Null | Xml::Text(_) | Xml::List(_) => Vec::new(),
                };
                o.push((Cow::Borrowed(f.wire), Xml::Obj(own)));
            }
            continue;
        }
        if f.list && !f.wrapped {
            let items: Vec<Xml> =
                children.iter_mut().filter(|c| named(c, f.item)).filter_map(Option::take).map(|(_, v)| v).collect();
            // An unwrapped empty list writes nothing. Absent means empty
            // when the property is required. Otherwise it means not given.
            if !items.is_empty() || f.required {
                o.push((Cow::Borrowed(f.wire), Xml::List(items)));
            }
        } else if let Some((_, v)) = children.iter_mut().find(|c| named(c, f.name)).and_then(Option::take) {
            // A wrapped list's wrapper already closed into its items.
            o.push((Cow::Borrowed(f.wire), if f.list { v } else { element_value(v, f) }));
        }
    }
    push_named(&mut o, children.into_iter().flatten().map(|(name, v)| (Cow::Owned(name), v)));
    Xml::Obj(o)
}

/// Elements nested deeper than this make a body malformed.
pub(crate) const MAX_DEPTH: usize = 256;

/// Returns an element's local name (prefix dropped) and its attributes. `xmlns` declarations are left out and
/// `xsi:nil="true"` marks the element nil.
fn open(start: &BytesStart<'_>, role: Role) -> Result<Open, String> {
    let name = local(start.name());
    let mut e = Open { name, role, attrs: vec![], children: vec![], text: String::new(), cdata: None, nil: false };
    for a in start.attributes() {
        let a = a.map_err(|err| err.to_string())?;
        let key = a.key.as_ref();
        let value = a.normalized_value(XmlVersion::Implicit1_0).map_err(|err| err.to_string())?;
        if key.starts_with("xmlns") {
            continue;
        }
        if a.key.local_name().as_ref() == "nil" && value == "true" {
            e.nil = true;
        } else {
            e.attrs.push((local(a.key), value.into_owned()));
        }
    }
    Ok(e)
}

/// Returns the value of the root element of a document in the role `root`. It is converted element by element as each
/// one closes. Prologs, comments, processing instructions and DOCTYPEs are skipped.
pub(crate) fn parse(text: &str, root: Role) -> Result<Xml, String> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Open> = vec![];
    // A closed element goes to its parent or is the value of the document.
    let deliver = |stack: &mut Vec<Open>, e: Open| -> Option<Xml> {
        let name = e.name.clone();
        let v = close(e);
        match stack.last_mut() {
            Some(parent) => {
                parent.children.push((name, v));
                None
            }
            None => Some(v),
        }
    };
    let role_of = |stack: &[Open], s: &BytesStart<'_>| match stack.last() {
        Some(parent) => child_role(parent, s.local_name().as_ref()),
        None => root,
    };
    loop {
        let event = reader.read_event().map_err(|e| format!("at byte {}: {e}", reader.error_position()))?;
        let text_of = |stack: &mut Vec<Open>, s: &str| {
            if let Some(e) = stack.last_mut() {
                e.text.push_str(s);
            }
        };
        match event {
            Event::Start(s) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(format!("elements nest deeper than {MAX_DEPTH}"));
                }
                let e = open(&s, role_of(&stack, &s))?;
                stack.push(e);
            }
            Event::Empty(s) => {
                let e = open(&s, role_of(&stack, &s))?;
                if let Some(root) = deliver(&mut stack, e) {
                    return Ok(root);
                }
            }
            Event::End(_) => {
                let e = stack.pop().ok_or("an end tag without a start")?;
                if let Some(root) = deliver(&mut stack, e) {
                    return Ok(root);
                }
            }
            Event::Text(t) => text_of(&mut stack, &t.xml_content(XmlVersion::Implicit1_0)),
            Event::CData(c) => {
                let s = c.xml_content(XmlVersion::Implicit1_0);
                if let Some(e) = stack.last_mut() {
                    e.cdata.get_or_insert_default().push_str(&s);
                }
                text_of(&mut stack, &s);
            }
            Event::GeneralRef(r) => {
                let name = r.xml_content(XmlVersion::Implicit1_0);
                // An entity a DTD would define stays as it is written.
                let resolved = match r.resolve_char_ref().map_err(|e| e.to_string())? {
                    Some(c) => c.to_string(),
                    None => resolve_predefined_entity(&name).map_or_else(|| format!("&{name};"), str::to_owned),
                };
                text_of(&mut stack, &resolved);
            }
            Event::Eof => {
                return Err(match stack.last() {
                    Some(e) => format!("`{}` is not closed", e.name),
                    None => "expected an element".to_owned(),
                });
            }
            Event::Comment(_) | Event::Decl(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
}

/// A decoded XML element before it becomes its type. It is much smaller than a `serde_json::Value`, where each object
/// contains a map and every key is an allocation. Here the property names of a schema are borrowed and an object is a
/// list of its properties.
#[derive(Debug)]
pub(crate) enum Xml {
    Null,
    Text(String),
    List(Vec<Self>),
    Obj(Vec<(Cow<'static, str>, Self)>),
}

impl Xml {
    /// Returns the same value as a `serde_json::Value`: what a deserializer decodes where `Xml` has no shortcut.
    pub(crate) fn into_value(self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Text(s) => Value::String(s),
            Self::List(items) => Value::Array(items.into_iter().map(Self::into_value).collect()),
            Self::Obj(props) => {
                let mut o = Map::new();
                for (k, v) in props {
                    push_multi(&mut o, k.into_owned(), v.into_value());
                }
                Value::Object(o)
            }
        }
    }
}

/// Adds properties to an object. A name that it already has gets one more value and its values become a list. This
/// is what [`push_multi`] does for a `Value`. An index finds the names. So an element with many attributes or
/// children of its own costs linear time.
fn push_named(o: &mut Vec<(Cow<'static, str>, Xml)>, props: impl IntoIterator<Item = (Cow<'static, str>, Xml)>) {
    let mut at: BTreeMap<Cow<'static, str>, usize> = o.iter().enumerate().map(|(i, (k, _))| (k.clone(), i)).collect();
    for (key, v) in props {
        match at.get(&key) {
            Some(&i) => match &mut o[i].1 {
                Xml::List(items) => items.push(v),
                existing @ (Xml::Null | Xml::Text(_) | Xml::Obj(_)) => {
                    let prev = std::mem::replace(existing, Xml::Null);
                    *existing = Xml::List(vec![prev, v]);
                }
            },
            None => {
                at.insert(key.clone(), o.len());
                o.push((key, v));
            }
        }
    }
}
