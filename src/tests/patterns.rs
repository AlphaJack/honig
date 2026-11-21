//! A `pattern` gets no warning about how long matching lasts: whoever writes it chooses its shape.

use super::common;

use serde_json::json;

#[test]
fn a_pattern_gets_no_warning_about_matching_time() {
    for (test, pattern) in [("pattern-nested", "^(a+)+$"), ("pattern-list", "^-?\\w+(,-?\\w+)*$")] {
        let doc =
            common::spec("3.1.0", &json!({}), &json!({"schemas": {"Handle": {"type": "string", "pattern": pattern}}}));
        let w = common::build(test, &doc, &[]).1;
        assert!(!w.iter().any(|m| m.contains(pattern)), "{pattern}: {w:#?}");
    }
}
