//! How the generator's cost grows with the description: a description of `n` bytes, as schemas and the operations
//! that return them, must not cost more than `n^1.3` in any phase. The shapes: chains of `$ref`, of `anyOf`, of nested
//! `oneOf`, of `allOf` and of `anyOf` with `unevaluatedProperties`, and the first for a lenient client. One run per
//! size counts each phase (load, ir, emit, print) apart, so a phase that grows too fast shows by name. Counted by
//! callgrind: see `tests/bench-support/src/callgrind.rs`.

#![expect(clippy::disallowed_methods, reason = "writes the description the generator reads")]

// The library under one name, whatever the package is called.
extern crate cargo_honig as generator;

use serde_json::{Map, Value, json};

use bench_support::{Phased, Work, callgrind, phase};

/// Description sizes in bytes, each 4 times the one before. Small: each function is judged on its own cost, which shows
/// a quadratic one long before it outweighs the rest of its phase. At these sizes the O(n^3) walk of #132 read n^2.85 as
/// a function while its phase read n^1.27.
const SIZES: [usize; 3] = [4 << 10, 16 << 10, 64 << 10];
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
/// by many paths, and a check reaches every schema before it.
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

/// A description of `count` schemas, each made by `schema` from its index, and an operation that returns each.
fn products(count: usize, schema: impl Fn(usize) -> Value) -> Value {
    let mut schemas = Map::new();
    let mut paths = Map::new();
    for i in 0..count {
        schemas.insert(format!("Product{i}"), schema(i));
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

/// The object of product `i`: a property that only it requires, so a union tells it from the others.
fn own(i: usize) -> Value {
    json!({"type": "object", "required": [format!("p{i}")], "properties": {
        format!("p{i}"): {"type": "string", "maxLength": 80}, "id": {"type": "integer"}}})
}

/// Products whose `oneOf` holds the product before and an object of their own: unions nested as deep as the
/// description is long. A union counts the branches that match, so a nested one is judged at every level.
fn one_of_chain(count: usize) -> Value {
    products(count, |i| match i.checked_sub(1) {
        Some(before) => json!({"oneOf": [{"$ref": format!("#/components/schemas/Product{before}")}, own(i)]}),
        None => own(i),
    })
}

/// How many `allOf` members a chain merges before it starts again: a struct holds the fields of every member, so an
/// endless chain would grow its output with the square of its length.
const ALL_OF_RUN: usize = 8;

/// Products that are the `allOf` of the product before and an object of their own, in runs of [`ALL_OF_RUN`]. Each
/// merges the properties of its members, some declared by both.
fn all_of_chain(count: usize) -> Value {
    products(count, |i| match i.checked_sub(1).filter(|_| i % ALL_OF_RUN != 0) {
        Some(before) => json!({"allOf": [{"$ref": format!("#/components/schemas/Product{before}")}, own(i)]}),
        None => own(i),
    })
}

/// Products whose `anyOf` holds the product before and an object of their own, with `unevaluatedProperties: false`:
/// checked whole, with the properties that any matching branch evaluates.
fn unevaluated_chain(count: usize) -> Value {
    products(count, |i| match i.checked_sub(1) {
        Some(before) => json!({"anyOf": [{"$ref": format!("#/components/schemas/Product{before}")}, own(i)],
            "unevaluatedProperties": false}),
        None => own(i),
    })
}

/// The phases of the generator in the order they run, each counted alone.
const PHASES: &[&str] = &["load", "ir", "emit", "print"];

/// Every phase of the generator on the description that `shape` builds for `size`, written to a file of its own.
fn run(shape: fn(usize) -> Value, size: usize) -> (usize, Work) {
    run_with(shape, size, true)
}

/// [`run`] for a lenient client (`client = { strict = false }`) when `strict` is false: every enum, closed struct and
/// tagged union splits into a type for requests and one for responses.
fn run_with(shape: fn(usize) -> Value, size: usize, strict: bool) -> (usize, Work) {
    use generator::__private::phases;
    let text = shape(size).to_string();
    let file = std::env::temp_dir().join(format!("scaling-{}-{}.json", text.len(), std::process::id()));
    std::fs::write(&file, &text).ok();
    let mut opts = generator::__private::Options::default();
    opts.strict_client = strict;
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

/// About how many bytes of description one product of [`products`] and its operation take.
const PRODUCT_BYTES: usize = 370;

const PATHS: &[Phased] = &[
    Phased { name: "ref", phases: PHASES, prepare: |size| run(sized_ref, size) },
    Phased { name: "any_of", phases: PHASES, prepare: |size| run(sized_any_of, size) },
    Phased { name: "one_of", phases: PHASES, prepare: |size| run(|s| one_of_chain(s.div_ceil(PRODUCT_BYTES)), size) },
    Phased { name: "all_of", phases: PHASES, prepare: |size| run(|s| all_of_chain(s.div_ceil(PRODUCT_BYTES)), size) },
    Phased { name: "lenient_ref", phases: PHASES, prepare: |size| run_with(sized_ref, size, false) },
    Phased {
        name: "unevaluated",
        phases: PHASES,
        prepare: |size| run(|s| unevaluated_chain(s.div_ceil(PRODUCT_BYTES)), size),
    },
];

fn main() -> std::process::ExitCode {
    callgrind::main_phased("scaling-generator", PATHS, &SIZES)
}
