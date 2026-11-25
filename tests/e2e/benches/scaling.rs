//! How the cost of decoding into generated types grows with the body: a body of `n` bytes must not cost more than
//! `n^1.3`. honig's scaling bench uses hand-written types only. Here the types are the ones the generator
//! writes, so a union that tries each branch on every level of a recursive value shows. Counted by callgrind: see
//! `tests/bench-support/src/callgrind.rs`.

use bench_support::{Path, Work, callgrind};
use e2e::v32_validation::api::types;
use serde_json::{Value, json};

/// Body sizes in bytes, each 4 times the one before.
const SIZES: [usize; 3] = [16 << 10, 64 << 10, 256 << 10];
/// About how many bytes of body one node of a tree needs.
const NODE_BYTES: usize = 20;

/// A node of `validation.yaml`'s `Tree`: its children and an `x` that tells the two branches apart. Only the type of
/// `x` does, after the children: a union that decoded the children again for each branch doubles per level.
fn node(children: Vec<Value>, i: usize) -> Value {
    if i.is_multiple_of(2) {
        json!({"items": children, "x": i})
    } else {
        json!({"items": children, "x": i.to_string()})
    }
}

/// A balanced binary tree of about `count` nodes: its depth grows as the logarithm of its size.
fn binary(count: usize, at: usize) -> Value {
    if count <= 1 {
        return node(vec![], at);
    }
    let half = (count - 1) / 2;
    node(vec![binary(half, 2 * at + 1), binary(count - 1 - half, 2 * at + 2)], at)
}

/// A root with about `count` leaves.
fn wide(count: usize) -> Value {
    node((0..count).map(|i| node(vec![], i)).collect(), 0)
}

/// Decoding `body` into a `Tree` as a server decodes a request.
fn decode(body: &Value) -> (usize, Work) {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    let len = bytes.len();
    let work: Work = Box::new(move || {
        std::hint::black_box(honig::__private::json_decode::<types::Tree>(Some(200), &bytes).ok());
    });
    (len, work)
}

const PATHS: &[Path] = &[
    Path { name: "tree_binary", prepare: |size| decode(&binary(size / NODE_BYTES, 0)) },
    Path { name: "tree_wide", prepare: |size| decode(&wide(size / NODE_BYTES)) },
];

fn main() -> std::process::ExitCode {
    callgrind::main("scaling-e2e", PATHS, &SIZES)
}
