//! String formats map to the date library the target crate depends on.

use super::common;

use crate::ir::types::Format;
use crate::settings::FormatLibs;
use serde_json::json;

fn types_rs(test: &str, jiff: bool) -> String {
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {"Shop": {
            "type": "object",
            "required": ["opens"],
            "properties": {"opens": {"type": "string", "format": "time"}},
        }}}),
    );
    let opts = crate::emit::Options {
        rust: crate::emit::rust::Options { libs: FormatLibs { jiff, ..FormatLibs::default() }, ..Default::default() },
        ..Default::default()
    };
    common::file(&common::generate(test, &doc, &opts), "types.rs").to_owned()
}

/// `time` has an offset (`08:30:06Z`) that no `jiff` time type represents. It is the runtime's `FullTime`, plain
/// numbers, with or without `jiff`.
#[test]
fn time_is_a_full_time_with_any_date_library() {
    for (test, jiff) in [("time-jiff", true), ("time-none", false)] {
        let t = types_rs(test, jiff);
        assert!(t.contains("pub opens: runtime::FullTime"), "{test}: {t}");
    }
}

/// A `format: decimal` number keeps its digits (`runtime::Decimal` decoded through the raw value of `serde_json`).
/// There is nothing to warn about.
#[test]
fn a_decimal_number_keeps_its_digits() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Price": {"type": "object", "properties": {
            "amount": {"type": "number", "format": "decimal"},
            "total": {"type": "number", "format": "decimal128"},
            "label": {"type": "string", "format": "decimal"}
        }}}}),
    );
    let (_, diags) = common::build("decimal-number", &doc, &[]);
    assert!(diags.is_empty(), "{diags:#?}");
    let t = types_rs_of("decimal-number-types", &doc);
    assert!(t.contains("#[optional]\n        pub amount: runtime::Decimal"), "{t}");
    // `decimal128` keeps its digits too, as a checked type bound to the limits of IEEE 754 decimal128.
    assert!(
        t.contains("#[optional]\n        pub total: PriceTotal") && t.contains("runtime::check_decimal128("),
        "{t}"
    );
    assert!(t.contains("#[optional]\n        pub label: runtime::DecimalStr"), "{t}");
}

fn types_rs_of(test: &str, doc: &serde_json::Value) -> String {
    common::file(&common::generate(test, doc, &crate::emit::Options::default()), "types.rs").to_owned()
}

/// `{type: string, format: decimal}` is checked text that keeps every digit, as `decimal128` is. The 28 significant
/// digits of `rust_decimal` would refuse longer values that the format allows.
#[test]
fn a_string_decimal_is_checked_text() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Price": {"type": "object", "required": ["amount", "wide"], "properties": {
            "amount": {"type": "string", "format": "decimal"},
            "wide": {"type": "string", "format": "decimal128"}
        }}}}),
    );
    let t = common::file(&common::generate("string-decimal", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    assert!(t.contains("pub amount: runtime::DecimalStr"), "{t}");
    assert!(t.contains("pub wide: runtime::Decimal128Str"), "{t}");
}

/// `date-time-local` and `time-local` (RFC 3339 without an offset) get the date library's civil types.
#[test]
fn local_date_time_formats_use_the_date_library() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Shift": {"type": "object", "required": ["starts", "ends"], "properties": {
            "starts": {"type": "string", "format": "date-time-local"},
            "ends": {"type": "string", "format": "time-local"}
        }}}}),
    );
    let with = |jiff: bool, test: &str| {
        let opts = crate::emit::Options {
            rust: crate::emit::rust::Options {
                libs: FormatLibs { jiff, ..FormatLibs::default() },
                ..Default::default()
            },
            ..Default::default()
        };
        common::file(&common::generate(test, &doc, &opts), "types.rs").to_owned()
    };
    let jiff = with(true, "local-jiff");
    assert!(
        jiff.contains("pub starts: runtime::jiff::civil::DateTime")
            && jiff.contains("pub ends: runtime::jiff::civil::Time")
    );
    assert!(with(false, "local-none").contains("pub starts: runtime::DateTimeLocalStr"));
}

/// A `pattern` beside a format that has a type of its own is not checked: the type decides the value. Generation says
/// so, once, at the pattern. A pattern on a plain string is checked and needs no word.
#[test]
fn a_pattern_beside_a_typed_format_is_reported_unchecked() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Order": {"type": "object", "properties": {
            "id": {"type": "string", "format": "uuid", "pattern": "^0"},
            "code": {"type": "string", "pattern": "^[A-Z]+$"}
        }}}}),
    );
    let (_, w) = common::build("pattern-format", &doc, &[]);
    let at: Vec<&String> = w.iter().filter(|m| m.contains("is not checked")).collect();
    assert_eq!(at.len(), 1, "{w:#?}");
    assert!(at[0].starts_with("/components/schemas/Order/properties/id/pattern:"), "{at:?}");
    assert!(at[0].contains("format: uuid"), "{at:?}");
}

/// A type that the `formats` table sets replaces a library for its format. Its text is checked first, as a
/// library's is: the type may accept more than the format allows. It has no `Default` that honig could know of.
#[test]
fn a_named_type_holds_its_format_with_the_text_checked() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Order": {"type": "object", "required": ["placed", "day"], "properties": {
            "placed": {"type": "string", "format": "date-time"},
            "day": {"type": "string", "format": "date"},
            "contact": {"type": "string", "format": "email"}
        }}}}),
    );
    let named = |text: &str| crate::settings::NamedType::new(text).unwrap();
    let mut libs = FormatLibs { jiff: true, ..FormatLibs::default() };
    libs.named.insert(Format::DateTime, named("crate::Stamp"));
    libs.named.insert(Format::Email, named("crate::Mail"));
    let opts =
        crate::emit::Options { rust: crate::emit::rust::Options { libs, ..Default::default() }, ..Default::default() };
    let t = common::file(&common::generate("named-format", &doc, &opts), "types.rs").to_owned();
    assert!(t.contains("pub placed: crate::Stamp"), "{t}");
    assert!(t.contains("pub day: runtime::jiff::civil::Date"), "the other date formats keep the library: {t}");
    assert!(t.contains("pub contact: Option<crate::Mail>"), "{t}");
    assert!(t.contains("format::DateTime") && t.contains("format::Email"), "each text is checked: {t}");
}
