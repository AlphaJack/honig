//! How the generator's cost grows with the description: a description of `n` bytes, as schemas and the operations
//! that return them, must not cost more than `n^1.3`. Counted by callgrind: see
//! `tests/bench-support/src/callgrind.rs`.

#![expect(clippy::disallowed_methods, reason = "writes the description the generator reads")]

// The library under one name, whatever the package is called.
extern crate cargo_honig as generator;

use serde_json::{Map, Value, json};

use bench_support::{Path, Work, callgrind};

/// Description sizes in bytes, each 4 times the one before. A fourth size of 1 MiB doubles the run under callgrind.
const SIZES: [usize; 3] = [16 << 10, 64 << 10, 256 << 10];
/// About how many bytes of description one schema and its operation take.
const SCHEMA_BYTES: usize = 400;

/// A description of `count` products, each a schema with a reference to the one before and an operation that returns
/// it.
fn description(count: usize) -> Value {
    let mut schemas = Map::new();
    let mut paths = Map::new();
    for i in 0..count {
        let mut properties = json!({"id": {"type": "integer", "format": "int64"}, "name": {"type": "string",
            "maxLength": 80}, "tags": {"type": "array", "items": {"type": "string"}}});
        if let Some(before) = i.checked_sub(1) {
            properties["related"] = json!({"$ref": format!("#/components/schemas/Product{before}")});
        }
        schemas.insert(format!("Product{i}"), json!({"type": "object", "required": ["id"], "properties": properties}));
        let ok = json!({"description": "The product", "content": {"application/json": {"schema":
            {"$ref": format!("#/components/schemas/Product{i}")}}}});
        let get = json!({"operationId": format!("getProduct{i}"), "responses": {"200": ok}});
        paths.insert(
            format!("/products{i}/{{id}}"),
            json!({"get": get, "parameters": [{"name": "id", "in": "path",
            "required": true, "schema": {"type": "integer"}}]}),
        );
    }
    json!({"openapi": "3.2.0", "info": {"title": "Shop", "version": "1"}, "paths": paths,
        "components": {"schemas": schemas}})
}

/// The generator on a description of about `size` bytes, written to a file of its own.
fn generate(size: usize) -> (usize, Work) {
    let text = description(size.div_ceil(SCHEMA_BYTES)).to_string();
    let file = std::env::temp_dir().join(format!("scaling-{size}-{}.json", std::process::id()));
    std::fs::write(&file, &text).ok();
    let work: Work = Box::new(move || {
        std::hint::black_box(generator::__private::generate(&file, &generator::__private::Options::default()).ok());
    });
    (text.len(), work)
}

const PATHS: &[Path] = &[Path { name: "generate", prepare: generate }];

fn main() -> std::process::ExitCode {
    callgrind::main(PATHS, &SIZES)
}
