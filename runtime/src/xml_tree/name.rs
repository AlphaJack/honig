//! XML names without their namespace prefix. `write::Tag` writes them with it.

/// Returns a tag's or an attribute's name without its prefix.
pub(crate) fn local(name: quick_xml::name::QName<'_>) -> String {
    name.local_name().as_ref().to_owned()
}
