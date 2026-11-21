//! A type that the `formats` table sets represents its format. `named-formats.toml` sets `String` for `date-time`.
//! `String` accepts any text: honig checks the format before it does.

use e2e::v32_named_formats::api::types;

#[test]
fn a_named_type_reads_only_text_of_its_format() {
    let stamp: types::Stamp = serde_json::from_str(r#"{"at":"2024-06-19T08:30:00Z"}"#).expect("a date-time");
    let at: String = stamp.at;
    assert_eq!(at, "2024-06-19T08:30:00Z");
    // RFC 3339 needs `T` and seconds. A `String` alone would accept this.
    let loose = serde_json::from_str::<types::Stamp>(r#"{"at":"2024-06-19 08:30Z"}"#);
    assert!(loose.is_err(), "{loose:?}");
}
