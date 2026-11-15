//! Example Objects reach the generated `documented_examples_decode` test in
//! every form: `value`, `dataValue`, `serializedValue` and `externalValue`.

use super::common;

use serde_json::{Value, json};

/// A `POST /products` JSON body with these `examples`.
fn body_spec(examples: &Value) -> Value {
    common::spec(
        "3.2.0",
        &json!({"/products": {"post": {
            "operationId": "add",
            "requestBody": {"content": {"application/json": {
                "schema": {"type": "object", "properties": {"name": {"type": "string"}}},
                "examples": examples,
            }}},
            "responses": {"204": {"description": "ok"}},
        }}}),
        &json!({}),
    )
}

/// A `GET /products` with one pipe-delimited `tags` query parameter.
fn param_spec(examples: &Value) -> Value {
    common::spec(
        "3.2.0",
        &json!({"/products": {"get": {
            "parameters": [{
                "name": "tags", "in": "query", "style": "pipeDelimited", "explode": false,
                "schema": {"type": "array", "items": {"type": "string"}},
                "examples": examples,
            }],
            "responses": {"204": {"description": "ok"}},
        }}}),
        &json!({}),
    )
}

/// Examples of the request body and the diagnostics.
fn body_examples(test: &str, examples: &Value, files: &[(&str, &Value)]) -> (Vec<(String, Value)>, Vec<String>) {
    let (api, diags) = common::build(test, &body_spec(examples), files);
    let body = api.http.operations[0].body.as_ref().expect("body");
    (body.contents[0].examples.clone(), diags)
}

#[test]
fn external_value_is_read_relative_to_the_description() {
    let lamp = json!({"name": "Lamp"});
    let (ex, diags) =
        body_examples("ex-external", &json!({"lamp": {"externalValue": "lamp.json"}}), &[("lamp.json", &lamp)]);
    assert_eq!(ex.len(), 1, "{diags:#?}");
    assert!(ex[0].0.ends_with("/examples/lamp/externalValue"), "{ex:?}");
    assert_eq!(ex[0].1, lamp);
}

#[test]
fn missing_external_value_is_a_warning() {
    let (ex, diags) = body_examples("ex-missing", &json!({"gone": {"externalValue": "gone.json"}}), &[]);
    assert_eq!(ex, vec![]);
    assert!(diags.iter().any(|d| d.contains("/examples/gone/externalValue") && d.contains("gone.json")), "{diags:#?}");
}

#[test]
fn serialized_value_of_a_json_body_is_parsed() {
    let mug = json!({"name": "Mug"});
    let (ex, diags) = body_examples("ex-serialized", &json!({"mug": {"serializedValue": mug.to_string()}}), &[]);
    assert_eq!(ex.len(), 1, "{diags:#?}");
    assert_eq!(ex[0].1, mug);
}

#[test]
fn serialized_parameter_examples_keep_their_wire_form() {
    let examples = json!({
        "two": {"dataValue": ["a", "b"], "serializedValue": "tags=a|b"},
        "plain": {"value": ["c"]},
    });
    let (api, _) = common::build("ex-param", &param_spec(&examples), &[]);
    let p = &api.http.operations[0].params[0];
    assert_eq!(p.serialized.len(), 1, "{:?}", p.serialized);
    let (at, value, wire) = &p.serialized[0];
    assert!(at.ends_with("/examples/two/serializedValue"), "{at}");
    assert_eq!(value, &json!(["a", "b"]));
    assert_eq!(wire, "tags=a|b");
}

/// The generated examples test compares every documented `serializedValue`.
#[test]
fn serialized_parameter_examples_are_checked_by_the_generated_test() {
    let examples = json!({"two": {"dataValue": ["a", "b"], "serializedValue": "tags=a,b"}});
    let opts = crate::emit::Options { tests: true, ..crate::emit::Options::default() };
    let files = common::generate("ex-wrong", &param_spec(&examples), &opts);
    let tests = common::file(&files, "documented_examples.rs");
    assert!(tests.contains("fn wire("), "no wire check emitted");
    assert!(tests.contains("\"tags=a,b\""), "documented form not checked");
}
