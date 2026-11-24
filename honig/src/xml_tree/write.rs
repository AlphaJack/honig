//! The pieces of an XML body that both encoders write: the one place for each rule of the output.

use std::borrow::Cow;

use super::{XmlField, XmlNode};

/// The namespace of `xsi:nil`.
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// What every body starts with.
pub(super) const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>";

/// The name of an element or an attribute: `prefix:name` or `name`.
#[derive(Clone, Copy)]
pub(super) struct Tag<'n> {
    pub(super) prefix: &'n str,
    pub(super) name: &'n str,
}

impl<'n> Tag<'n> {
    /// Returns a name without a prefix.
    pub(super) const fn plain(name: &'n str) -> Self {
        Self { prefix: "", name }
    }

    fn push(self, out: &mut String) {
        if !self.prefix.is_empty() {
            out.push_str(self.prefix);
            out.push(':');
        }
        out.push_str(self.name);
    }
}

/// Returns the element of a root.
pub(super) const fn root(node: &XmlNode) -> Tag<'static> {
    Tag { prefix: node.prefix, name: node.name }
}

/// Returns the element of a field.
pub(super) const fn field(f: &XmlField<XmlNode>) -> Tag<'static> {
    Tag { prefix: f.prefix, name: f.name }
}

/// Returns the element of each item of a field and the element around them. A wrapped list has its field's element
/// around items named `item`. An unwrapped one has the items alone, `item` with the field's prefix. Any other value is
/// the field's element once per item.
pub(super) const fn items(f: &XmlField<XmlNode>) -> (Tag<'static>, Option<Tag<'static>>) {
    match (f.list, f.wrapped) {
        (true, true) => (Tag::plain(f.item), Some(field(f))),
        (true, false) => (Tag { prefix: f.prefix, name: f.item }, None),
        (false, _) => (field(f), None),
    }
}

/// Returns the namespace declaration of a root element: ` xmlns="..."` or ` xmlns:prefix="..."`.
pub(super) fn namespace(node: &XmlNode) -> Option<String> {
    (!node.namespace.is_empty()).then(|| {
        if node.prefix.is_empty() {
            format!(" xmlns=\"{}\"", escape(node.namespace))
        } else {
            format!(" xmlns:{}=\"{}\"", node.prefix, escape(node.namespace))
        }
    })
}

/// Returns text with `<`, `>`, `&`, `'` and `"` escaped.
pub(crate) fn escape(s: &str) -> Cow<'_, str> {
    quick_xml::escape::escape(s)
}

/// Writes `<tag extra`: the caller ends the start tag.
pub(super) fn open(out: &mut String, tag: Tag<'_>, extra: &str) {
    out.push('<');
    tag.push(out);
    out.push_str(extra);
}

/// Writes `<tag extra>`.
pub(super) fn start(out: &mut String, tag: Tag<'_>, extra: &str) {
    open(out, tag, extra);
    out.push('>');
}

/// Writes `</tag>`.
pub(super) fn close(out: &mut String, tag: Tag<'_>) {
    out.push_str("</");
    tag.push(out);
    out.push('>');
}

/// Writes `null`: an empty element with `xsi:nil="true"`.
pub(super) fn nil(out: &mut String, tag: Tag<'_>, extra: &str) {
    open(out, tag, extra);
    out.push_str(" xmlns:xsi=\"");
    out.push_str(XSI);
    out.push_str("\" xsi:nil=\"true\"/>");
}

/// Writes ` name="`: the escaped value and [`attribute_end`] follow.
pub(super) fn attribute_start(out: &mut String, tag: Tag<'_>) {
    out.push(' ');
    tag.push(out);
    out.push_str("=\"");
}

pub(super) fn attribute_end(out: &mut String) {
    out.push('"');
}

/// Writes the start of a CDATA section: [`cdata_text`] and [`cdata_end`] follow.
pub(super) fn cdata_start(out: &mut String) {
    out.push_str("<![CDATA[");
}

/// Writes text in a CDATA section. `]]>` cannot appear inside one: it is split.
pub(super) fn cdata_text(out: &mut String, text: &str) {
    out.push_str(&text.replace("]]>", "]]]]><![CDATA[>"));
}

pub(super) fn cdata_end(out: &mut String) {
    out.push_str("]]>");
}
