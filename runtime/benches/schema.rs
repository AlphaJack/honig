//! The JSON Schema check of a value that no Rust type judges alone: a body that is one of two object schemas, each
//! with unions of its own.

#![expect(clippy::expect_used, reason = "a bench fails loudly on input it built itself")]

use honig::__private as runtime;
use serde_json::json;

fn main() {
    divan::Divan::default().sample_count(50).max_time(std::time::Duration::from_millis(500)).config_with_args().main();
}

/// An order or a return: each a few typed fields, string-or-list unions and enums, open to more properties.
fn schema() -> serde_json::Value {
    let names = json!({"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]});
    let file = json!({"oneOf": [
        {"type": "string"},
        {"type": "object", "properties": {"url": {"type": "string"}, "name": {"type": "string"}}},
    ]});
    let files = json!({"oneOf": [file, {"type": "array", "items": file}]});
    let shared = json!({
        "channel": {"type": "string", "enum": ["web", "store", "phone", "partner"]},
        "format": {"type": ["string", "null"], "enum": ["text", "html", "markdown", "", null]},
        "tag": names, "tags": names,
        "extra": {"oneOf": [{"type": "object", "additionalProperties": {"type": ["string", "number", "null"]}}, names]},
    });
    let mut order = shared.clone();
    order.as_object_mut().expect("an object").extend(
        json!({"note": {"type": "string"}, "title": {"type": "string"}, "attach": files, "files": files, "to": names})
            .as_object()
            .expect("an object")
            .clone(),
    );
    let mut refund = shared;
    refund.as_object_mut().expect("an object").extend(
        json!({
            "status": {"type": "string"}, "reason": {"type": "string"}, "amount": {"type": "integer"},
            "lines": {"type": "array", "items": {"type": "object", "properties": {
                "sku": {"type": "string"}, "labels": {"type": "object", "additionalProperties": {"type": "string"}},
            }, "additionalProperties": true}},
            "to": names,
        })
        .as_object()
        .expect("an object")
        .clone(),
    );
    json!({"oneOf": [
        {"type": "object", "required": ["status", "lines"], "not": {"required": ["note"]}, "properties": refund,
         "additionalProperties": true},
        {"type": "object", "not": {"required": ["status", "lines"], "not": {"required": ["note"]}}, "properties": order,
         "additionalProperties": true},
    ]})
}

fn check(bencher: divan::Bencher<'_, '_>, body: &serde_json::Value) {
    let schema = schema();
    bencher.bench(|| {
        let mut out = vec![];
        runtime::check_schema(&mut out, &runtime::InstancePath::Root, divan::black_box(body), &schema);
        out
    });
}

/// An order that the second branch takes.
#[divan::bench]
fn an_order(bencher: divan::Bencher<'_, '_>) {
    let body = json!({
        "to": ["ann@example.com", "bob@example.com"], "note": "leave at the door", "title": "Order 42",
        "channel": "web", "format": "markdown", "tag": ["gift", "express"], "attach": "https://example.com/r.pdf",
    });
    check(bencher, &body);
}

/// A body that no branch takes: `to` is a number.
#[divan::bench]
fn a_wrong_field(bencher: divan::Bencher<'_, '_>) {
    check(bencher, &json!({"to": 5, "note": "x"}));
}
