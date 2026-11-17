//! String formats map to the date library the target crate depends on.

use super::common;

use crate::settings::{DateLib, FormatLibs};
use serde_json::json;

fn types_rs(test: &str, dates: Option<DateLib>) -> String {
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
        rust: crate::emit::rust::Options { libs: FormatLibs { dates, ..FormatLibs::default() }, ..Default::default() },
        ..Default::default()
    };
    common::file(&common::generate(test, &doc, &opts), "types.rs").to_owned()
}

/// `time` has an offset (`08:30:06Z`). `chrono::NaiveTime` has none and refuses it: the text stays a checked `String`.
#[test]
fn time_with_chrono_is_a_string() {
    let t = types_rs("time-chrono", Some(DateLib::Chrono));
    assert!(t.contains("pub opens: String"), "{t}");
}

/// With `jiff` a `time` is the runtime's `FullTime`: the clock and its offset in `jiff` types.
#[test]
fn time_with_jiff_is_a_full_time() {
    let t = types_rs("time-jiff", Some(DateLib::Jiff));
    assert!(t.contains("pub opens: runtime::FullTime"), "{t}");
}

#[test]
fn time_without_a_date_library_is_a_string() {
    assert!(types_rs("time-none", None).contains("pub opens: String"));
}

/// A `format: decimal` number keeps its digits (`runtime::Decimal` read through the raw value of `serde_json`). There
/// is nothing to warn about.
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
    assert!(
        t.contains("pub amount: Option<runtime::Decimal>") && t.contains("pub total: Option<runtime::Decimal>"),
        "{t}"
    );
    assert!(t.contains("pub label: Option<String>"), "{t}");
}

fn types_rs_of(test: &str, doc: &serde_json::Value) -> String {
    common::file(&common::generate(test, doc, &crate::emit::Options::default()), "types.rs").to_owned()
}

/// `{type: string, format: decimal}` is a `String` that keeps every digit as `decimal128` is. The 28 significant
/// digits of `rust_decimal` would refuse longer values that the format allows.
#[test]
fn a_string_decimal_is_a_string() {
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
    assert!(t.contains("pub amount: String") && t.contains("pub wide: String"), "{t}");
}

/// `date-time-local` and `time-local` (RFC 3339 without an offset) take the date library's civil types.
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
    let with = |dates: Option<DateLib>, test: &str| {
        let opts = crate::emit::Options {
            rust: crate::emit::rust::Options {
                libs: FormatLibs { dates, ..FormatLibs::default() },
                ..Default::default()
            },
            ..Default::default()
        };
        common::file(&common::generate(test, &doc, &opts), "types.rs").to_owned()
    };
    let chrono = with(Some(DateLib::Chrono), "local-chrono");
    assert!(chrono.contains("pub starts: chrono::NaiveDateTime") && chrono.contains("pub ends: chrono::NaiveTime"));
    let jiff = with(Some(DateLib::Jiff), "local-jiff");
    assert!(jiff.contains("pub starts: jiff::civil::DateTime") && jiff.contains("pub ends: jiff::civil::Time"));
    assert!(with(None, "local-none").contains("pub starts: String"));
}
