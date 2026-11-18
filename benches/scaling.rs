//! How the generator's cost grows with the description: a description of `n` bytes, as schemas and the operations
//! that return them, must not cost more than `n^1.3` in any phase. One run per size counts each phase (load, ir, emit,
//! print) apart, so a phase that grows too fast shows by name. Counted by callgrind: see
//! `tests/bench-support/src/callgrind.rs`.

#![expect(clippy::disallowed_methods, reason = "writes the description the generator reads")]

// The library under one name, whatever the package is called.
extern crate cargo_honig as generator;

use serde_json::{Map, Value, json};

use bench_support::{Phased, Work, callgrind, phase};

/// Description sizes in bytes, each 4 times the one before. A fourth size of 1 MiB doubles the run under callgrind.
const SIZES: [usize; 3] = [16 << 10, 64 << 10, 256 << 10];
/// About how many bytes of description one schema and its operation take.
const SCHEMA_BYTES: usize = 400;
/// The same for [`any_of_chain`].
const ANY_OF_SCHEMA_BYTES: usize = 460;

/// A description of `count` products, each a schema with a reference to the one before and an operation that returns
/// it.
fn ref_chain(count: usize) -> Value {
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

/// A description of `count` products, each a union of the two before it and an object that refers to them, and an
/// operation that returns it. Two branches of one type: the union is checked whole at run time. So a schema is reached
/// by many paths, and a check reaches every schema before it, as in large real descriptions.
fn any_of_chain(count: usize) -> Value {
    let mut schemas = Map::new();
    let mut paths = Map::new();
    for i in 0..count {
        let refs: Vec<Value> = [i.checked_sub(1), i.checked_sub(2)]
            .into_iter()
            .flatten()
            .map(|j| json!({"$ref": format!("#/components/schemas/Product{j}")}))
            .collect();
        let mut properties = json!({"id": {"type": "integer"}, "name": {"type": "string", "maxLength": 80}});
        for (k, r) in refs.iter().enumerate() {
            properties[format!("r{k}")] = r.clone();
        }
        let own = json!({"type": "object", "properties": properties});
        let schema = if refs.is_empty() {
            own
        } else {
            let branches: Vec<Value> = refs.into_iter().chain([own]).collect();
            json!({"anyOf": branches})
        };
        schemas.insert(format!("Product{i}"), schema);
        let ok = json!({"description": "The product", "content": {"application/json": {"schema":
            {"$ref": format!("#/components/schemas/Product{i}")}}}});
        paths.insert(
            format!("/products{i}"),
            json!({"get": {"operationId": format!("getProduct{i}"), "responses": {"200": ok}}}),
        );
    }
    json!({"openapi": "3.2.0", "info": {"title": "Shop", "version": "1"}, "paths": paths,
        "components": {"schemas": schemas}})
}

/// The phases of the generator in the order they run, each counted alone.
const PHASES: &[&str] = &["load", "ir", "emit", "print"];

/// Every phase of the generator on the description that `shape` builds for `size`, written to a file of its own.
fn run(shape: fn(usize) -> Value, size: usize) -> (usize, Work) {
    use generator::__private::phases;
    let text = shape(size).to_string();
    let file = std::env::temp_dir().join(format!("scaling-{}-{}.json", text.len(), std::process::id()));
    std::fs::write(&file, &text).ok();
    let opts = generator::__private::Options::default();
    let specs = vec![file];
    let work: Work = Box::new(move || {
        let loaded = built(phase(|| phases::load(&specs, &opts)));
        let ir = phase(|| phases::ir(&loaded, &opts));
        let trees = phase(|| phases::emit(&ir, &opts));
        built(phase(|| phases::print(&loaded, ir, trees, &opts)));
    });
    (text.len(), work)
}

/// The bench builds its own description: one that does not generate is a bug of the bench, said as such.
#[expect(clippy::panic, reason = "a bench fails loudly on input it built itself")]
fn built<T, E: std::fmt::Debug>(r: Result<T, E>) -> T {
    r.unwrap_or_else(|e| panic!("bench description: {e:?}"))
}

/// [`ref_chain`] of about `size` bytes.
fn sized_ref(size: usize) -> Value {
    ref_chain(size.div_ceil(SCHEMA_BYTES))
}

/// [`any_of_chain`] of about `size` bytes.
fn sized_any_of(size: usize) -> Value {
    any_of_chain(size.div_ceil(ANY_OF_SCHEMA_BYTES))
}

const PATHS: &[Phased] = &[
    Phased { name: "ref", phases: PHASES, prepare: |size| run(sized_ref, size) },
    Phased { name: "any_of", phases: PHASES, prepare: |size| run(sized_any_of, size) },
];

fn main() -> std::process::ExitCode {
    callgrind::main_phased(PATHS, &SIZES)
}
