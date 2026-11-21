//! Names in Rust: `PascalCase` for types and variants and `snake_case` for the rest. Keywords are escaped with a
//! trailing `_`. A [`Name`] never ends in `_`. So an escaped keyword cannot meet another name.

use crate::ir::names::{Name, Reserved, capitalize};
use crate::text::KEYWORDS;

/// Prelude names generated code uses unqualified. A shared response type avoids them: one named `string` would
/// otherwise declare a `String` that shadows the standard one in its module.
const PRELUDE: &[&str] = &[
    "Option",
    "Some",
    "None",
    "Result",
    "Ok",
    "Err",
    "String",
    "Vec",
    "Box",
    "From",
    "Into",
    "Default",
    "Send",
    "Sync",
    "Sized",
    "Copy",
    "Clone",
    "Eq",
    "PartialEq",
    "Ord",
    "PartialOrd",
    "Iterator",
    "ToString",
    "ToOwned",
    "Drop",
    "Fn",
    "FnMut",
    "FnOnce",
    "Future",
    "Debug",
];

/// What generated Rust keeps for itself. No type shadows the prelude. No module of an operation gets the name of
/// one of the own modules of the generated module.
pub(crate) const RESERVED: Reserved<'static> = Reserved {
    types: PRELUDE,
    operations: &["runtime", "types", "operations", "client", "server", "documented_examples", "responses", "tags"],
};

fn escape(s: String) -> String {
    if KEYWORDS.contains(&s.as_str()) { s + "_" } else { s }
}

/// Converts a name to keyword-safe `snake_case` for fields, parameters, functions and modules.
pub(crate) fn snake(n: &Name) -> String {
    escape(n.words().join("_"))
}

/// Returns `PascalCase` and keyword-safe for types and variants.
pub(crate) fn pascal(n: &Name) -> String {
    escape(n.words().iter().map(|w| capitalize(w)).collect())
}

/// Returns [`pascal`] with `suffix` after a [`PRELUDE`] name.
pub(crate) fn off_prelude(n: &Name, suffix: &str) -> String {
    let name = pascal(n);
    if PRELUDE.contains(&name.as_str()) { name + suffix } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases() {
        let (s, p) = (|x: &str| snake(&Name::new(x)), |x: &str| pascal(&Name::new(x)));
        assert_eq!(s("orderId"), "order_id");
        assert_eq!(s("X-Rate-Limit"), "x_rate_limit");
        assert_eq!(s("HTTPServer"), "http_server");
        assert_eq!(s("type"), "type_");
        assert_eq!(s("123abc"), "n123abc");
        assert_eq!(s("+1"), "plus1");
        assert_eq!(s("$ref"), "ref_");
        assert_eq!(s("s3Bucket"), "s3_bucket");
        assert_eq!(p("order_line"), "OrderLine");
        assert_eq!(p("KindVariant2Variant2"), "KindVariant2Variant2");
        assert_eq!(p("self"), "Self_");
        assert_eq!(p("2fa"), "N2fa");
        assert_eq!(p(""), "Empty");
        assert_eq!(off_prelude(&Name::new("string"), "Response"), "StringResponse");
    }
}
