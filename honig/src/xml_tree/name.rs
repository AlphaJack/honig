//! XML names without their namespace prefix. `write::Tag` writes them with it.

/// Returns a tag's or an attribute's name without its prefix.
pub(crate) fn local(name: quick_xml::name::QName<'_>) -> String {
    name.local_name().as_ref().to_owned()
}

/// Is `name` an XML name without a namespace prefix (`NCName`, Namespaces in XML 1.0 section 3)? An element or
/// attribute name has no escape, so a name from data that is not one cannot be written.
pub(crate) fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(is_start)
        && chars.all(|c| {
            is_start(c)
                || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
        })
}

/// Is `c` a `NameStartChar` of XML 1.0 section 2.3 other than `:`?
const fn is_start(c: char) -> bool {
    matches!(c,
        'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}
