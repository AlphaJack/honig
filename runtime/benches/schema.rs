//! The JSON Schema check of a value that no Rust type judges alone: a body that is one of two object schemas, each
//! with unions of its own (Mercurio's `POST /notify`).

#![expect(clippy::expect_used, reason = "a bench fails loudly on input it built itself")]

use honig::__private as runtime;

fn main() {
    divan::Divan::default().sample_count(50).max_time(std::time::Duration::from_millis(500)).config_with_args().main();
}

fn schema() -> serde_json::Value {
    serde_json::from_str(include_str!("schema_notify.json")).expect("the schema is JSON")
}

fn check(bencher: divan::Bencher<'_, '_>, body: &serde_json::Value) {
    let schema = schema();
    bencher.bench(|| {
        let mut out = vec![];
        runtime::check_schema(&mut out, &runtime::InstancePath::Root, divan::black_box(body), &schema);
        out
    });
}

/// A notification that the second branch takes.
#[divan::bench]
fn a_notification(bencher: divan::Bencher<'_, '_>) {
    let body = serde_json::json!({
        "urls": ["json://localhost", "mailto://a@b"], "body": "the build broke", "title": "CI",
        "type": "failure", "format": "markdown", "tag": ["ops", "ci"], "attach": "https://example.com/log.txt",
    });
    check(bencher, &body);
}

/// A body that no branch takes: `urls` is a number.
#[divan::bench]
fn a_wrong_field(bencher: divan::Bencher<'_, '_>) {
    check(bencher, &serde_json::json!({"urls": 5, "body": "x"}));
}
