//! A struct with more fields than `object!` expands under the default `recursion_limit`: the crate root raises it.

use e2e::v32_wide::api::types;

#[test]
fn a_struct_of_130_fields_decodes() {
    let s: types::Subscription = serde_json::from_str(r#"{"feature_130":"BOOK-1"}"#).unwrap();
    assert_eq!(s.feature_130.as_deref(), Some("BOOK-1"));
    assert_eq!(serde_json::to_string(&s).unwrap(), r#"{"feature_130":"BOOK-1"}"#);
}
