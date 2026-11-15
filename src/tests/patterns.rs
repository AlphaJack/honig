//! A `pattern` that backtracks exponentially gets a warning unless `maxLength` bounds the value it runs on.

use super::common;

use serde_json::{Value, json};

fn warnings(test: &str, schema: &Value) -> Vec<String> {
    let doc = common::spec("3.1.0", &json!({}), &json!({"schemas": {"Handle": schema}}));
    common::build(test, &doc, &[]).1
}

#[test]
fn a_repeated_group_that_repeats_without_max_length_is_a_warning() {
    let w = warnings("pattern-nested", &json!({"type": "string", "pattern": "^(a+)+$"}));
    assert!(w.iter().any(|m| m.contains("exponential time") && m.contains("maxLength")), "{w:#?}");
    let bounded = warnings("pattern-bounded", &json!({"type": "string", "pattern": "^(a+)+$", "maxLength": 20}));
    assert!(!bounded.iter().any(|m| m.contains("exponential")), "{bounded:#?}");
    let flat = warnings("pattern-flat", &json!({"type": "string", "pattern": "^[a-z]+$"}));
    assert!(!flat.iter().any(|m| m.contains("exponential")), "{flat:#?}");
}
