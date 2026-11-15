//! A `oneOf` union decoded in two ways from 256 B to 16 MiB. Throughput is per input byte.
//!
//! - `content`: a union without a `Decimal`. The value is buffered once in `runtime::Content`. Then each branch is
//!   tried on the buffer. Numbers become `f64` there.
//! - `raw`: a union holding a `Decimal` (`Content::exact`). The JSON text of the value is borrowed once. Then each branch is
//!   parsed from it. So the decimal keeps its digits.
//!
//! The matching branch is tried `first` or `last`. The two branches before it fail only at their missing id after
//! they read every line.

#![expect(clippy::expect_used, reason = "a bench fails loudly on input it built itself")]

use std::assert_matches;

use divan::Bencher;
use divan::counter::BytesCount;
use honig::__private::Content;
use serde::Deserialize;
use serde_json::value::RawValue;

const SIZES: [usize; 5] = [256, 8 << 10, 128 << 10, 2 << 20, 16 << 20];

fn main() {
    divan::Divan::default().sample_count(20).max_time(std::time::Duration::from_millis(200)).config_with_args().main();
}

#[derive(Debug, Deserialize)]
struct Line {
    sku: String,
    qty: u32,
    price: f64,
    note: String,
}

#[derive(Debug, Deserialize)]
struct Refund {
    refund_id: u64,
    lines: Vec<Line>,
}

#[derive(Debug, Deserialize)]
struct Quote {
    quote_id: u64,
    lines: Vec<Line>,
}

#[derive(Debug, Deserialize)]
struct Order {
    id: u64,
    customer: String,
    lines: Vec<Line>,
}

/// The branches of the union as generated code holds them.
#[derive(Debug)]
enum Union {
    Refund(Refund),
    Quote(Quote),
    Order(Order),
}

impl Union {
    /// The fields that each branch keeps. So the optimizer cannot drop the decode.
    fn weight(&self) -> u64 {
        let total = |lines: &[Line]| {
            lines
                .iter()
                .map(|l| u64::from(l.qty) + l.price.to_bits() + (l.sku.len() + l.note.len()) as u64)
                .sum::<u64>()
        };
        match self {
            Self::Refund(r) => r.refund_id + total(&r.lines),
            Self::Quote(q) => q.quote_id + total(&q.lines),
            Self::Order(o) => o.id + o.customer.len() as u64 + total(&o.lines),
        }
    }
}

/// An order of about `size` bytes of JSON.
fn order(size: usize) -> Vec<u8> {
    let line = |i: usize| {
        format!(
            "{{\"sku\":\"SKU-{i:08}\",\"qty\":{},\"price\":19.99,\"note\":\"fragile, \u{fc}n\u{ef}c\u{f6}d\u{e9}\"}}",
            i % 7 + 1
        )
    };
    let lines: Vec<String> = (0..size / line(0).len()).map(line).collect();
    format!(r#"{{"id":7,"customer":"ACME","lines":[{}]}}"#, lines.join(",")).into_bytes()
}

fn content(body: &[u8], order_first: bool) -> Union {
    let v: Content<'_> = serde_json::from_slice(body).expect("JSON");
    if order_first && let Ok(o) = v.decode_ref::<Order>() {
        return Union::Order(o);
    }
    if let Ok(r) = v.decode_ref::<Refund>() {
        return Union::Refund(r);
    }
    if let Ok(q) = v.decode_ref::<Quote>() {
        return Union::Quote(q);
    }
    Union::Order(v.decode_ref::<Order>().expect("an order"))
}

fn raw(body: &[u8], order_first: bool) -> Union {
    let v: &RawValue = serde_json::from_slice(body).expect("JSON");
    let text = v.get();
    if order_first && let Ok(o) = serde_json::from_str::<Order>(text) {
        return Union::Order(o);
    }
    if let Ok(r) = serde_json::from_str::<Refund>(text) {
        return Union::Refund(r);
    }
    if let Ok(q) = serde_json::from_str::<Quote>(text) {
        return Union::Quote(q);
    }
    Union::Order(serde_json::from_str::<Order>(text).expect("an order"))
}

fn run(b: Bencher<'_, '_>, size: usize, decode: fn(&[u8], bool) -> Union, order_first: bool) {
    let body = order(size);
    assert_matches!(decode(&body, order_first), Union::Order(_), "the order branch matches");
    b.counter(BytesCount::of_slice(&body)).bench(|| decode(divan::black_box(&body), order_first).weight());
}

#[divan::bench(args = SIZES)]
fn content_first(b: Bencher<'_, '_>, size: usize) {
    run(b, size, content, true);
}

#[divan::bench(args = SIZES)]
fn raw_first(b: Bencher<'_, '_>, size: usize) {
    run(b, size, raw, true);
}

#[divan::bench(args = SIZES)]
fn content_last(b: Bencher<'_, '_>, size: usize) {
    run(b, size, content, false);
}

#[divan::bench(args = SIZES)]
fn raw_last(b: Bencher<'_, '_>, size: usize) {
    run(b, size, raw, false);
}
