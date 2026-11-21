//! Schemas are interpreted with the `OpenAPI` dialect. A description that declares
//! another one or a `$vocabulary` gets a warning instead of silence.

use super::common;

use serde_json::{Value, json};

fn warnings(test: &str, doc: &Value) -> Vec<String> {
    common::build(test, doc, &[]).1
}

fn schemas(schemas: &Value) -> Value {
    common::spec("3.1.0", &json!({}), &json!({"schemas": schemas}))
}

fn dialect_noise(w: &[String]) -> bool {
    w.iter().any(|m| m.contains("dialect") || m.contains("vocabulary"))
}

#[test]
fn unknown_root_dialect_is_reported() {
    let mut doc = schemas(&json!({}));
    doc["jsonSchemaDialect"] = json!("https://example.com/my-dialect");
    let w = warnings("dialect-root", &doc);
    assert!(w.iter().any(|m| m.starts_with("/jsonSchemaDialect:") && m.contains("my-dialect")), "{w:#?}");
}

#[test]
fn unknown_schema_dialect_and_vocabulary_are_reported() {
    let w = warnings(
        "dialect-schema",
        &schemas(&json!({
            "A": {"$schema": "http://json-schema.org/draft-04/schema#", "type": "string"},
            "B": {"$vocabulary": {"https://json-schema.org/draft/2020-12/vocab/core": true}, "type": "string"},
        })),
    );
    assert!(w.iter().any(|m| m.starts_with("/components/schemas/A/$schema:") && m.contains("draft-04")), "{w:#?}");
    assert!(w.iter().any(|m| m.starts_with("/components/schemas/B/$vocabulary:")), "{w:#?}");
}

#[test]
fn supported_dialects_are_quiet() {
    let mut doc = schemas(&json!({
        "A": {"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "string"},
        "B": {"$schema": "http://json-schema.org/draft-07/schema#", "type": "string"},
    }));
    doc["jsonSchemaDialect"] = json!("https://spec.openapis.org/oas/3.1/dialect/base");
    let w = warnings("dialect-quiet", &doc);
    assert!(!dialect_noise(&w), "{w:#?}");
}

#[test]
fn schema_examples_are_not_scanned() {
    let doc = schemas(&json!({
        "Doc": {"type": "object", "examples": [{"$schema": "https://example.com/x", "$vocabulary": {}}]},
    }));
    let w = warnings("dialect-examples", &doc);
    assert!(!dialect_noise(&w), "{w:#?}");
}
