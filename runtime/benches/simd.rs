//! The paths that the `simd` feature speeds up from 256 B to 16 MiB. Run it with and without `--features simd`.
//! Throughput is per input byte.

#![expect(clippy::expect_used, reason = "a bench fails loudly on input it built itself")]

use divan::Bencher;
use divan::counter::BytesCount;
use honig::__private as runtime;

const SIZES: [usize; 5] = [256, 8 << 10, 128 << 10, 2 << 20, 16 << 20];
const BOUNDARY: &str = "boundary-7d1a2c";

#[derive(serde::Deserialize, serde::Serialize)]
struct Order {
    id: u64,
    customer: String,
    lines: Vec<Line>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct Line {
    sku: String,
    qty: u32,
    price: f64,
    note: String,
}

fn main() {
    // A time cap per bench and size. A whole run lasts seconds. A slow path shows as few samples and not as a wait.
    // Options on the command line (`--max-time 30` to profile one) override these.
    divan::Divan::default().sample_count(20).max_time(std::time::Duration::from_millis(200)).config_with_args().main();
}

/// An order of about `size` bytes of JSON with non-ASCII text for the UTF-8 check.
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

fn raw(size: usize) -> Vec<u8> {
    (0..size).map(|i| u8::try_from(i * 31 % 251).unwrap_or(0)).collect::<Vec<_>>()
}

#[divan::bench(args = SIZES)]
fn json_decode(b: Bencher<'_, '_>, size: usize) {
    let body = order(size);
    b.counter(BytesCount::of_slice(&body)).bench(|| runtime::json_decode::<Order>(None, divan::black_box(&body)));
}

#[divan::bench(args = SIZES)]
fn json_encode(b: Bencher<'_, '_>, size: usize) {
    let body = order(size);
    let order: Order = serde_json::from_slice(&body).expect("an order");
    b.counter(BytesCount::of_slice(&body)).bench(|| runtime::json_encode(divan::black_box(&order)));
}

/// A text body: the UTF-8 check and a copy into a `String`.
#[divan::bench(args = SIZES)]
fn text_decode(b: Bencher<'_, '_>, size: usize) {
    let body = order(size);
    b.counter(BytesCount::of_slice(&body)).bench(|| runtime::text_decode(divan::black_box(&body)));
}

#[divan::bench(args = SIZES)]
fn base64_encode(b: Bencher<'_, '_>, size: usize) {
    let data = raw(size);
    b.counter(BytesCount::of_slice(&data)).bench(|| runtime::base64(divan::black_box(&data)));
}

#[divan::bench(args = SIZES)]
fn base64_decode(b: Bencher<'_, '_>, size: usize) {
    let text = runtime::base64(&raw(size));
    b.counter(BytesCount::of_str(&text)).bench(|| runtime::unbase64(divan::black_box(&text)));
}

#[derive(serde::Deserialize)]
struct Upload {
    #[expect(dead_code, reason = "decoded to be measured")]
    file: runtime::Binary,
}

/// A form with one file part: the boundary and header searches and then the part kept as bytes.
#[divan::bench(args = SIZES)]
fn multipart_decode(b: Bencher<'_, '_>, size: usize) {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.bin\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend(raw(size));
    body.extend(format!("\r\n--{BOUNDARY}--\r\n").into_bytes());
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    b.counter(BytesCount::of_slice(&body))
        .bench(|| runtime::multipart_decode::<Upload>(None, &content_type, divan::black_box(&body)).expect("a form"));
}

#[derive(serde::Deserialize)]
struct Note {
    #[expect(dead_code, reason = "decoded to be measured")]
    note: String,
}

/// A form with one text part: the UTF-8 check and a copy into a `String`.
#[divan::bench(args = SIZES)]
fn multipart_text_decode(b: Bencher<'_, '_>, size: usize) {
    let mut body =
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"note\"\r\nContent-Type: text/plain\r\n\r\n")
            .into_bytes();
    body.extend(order(size));
    body.extend(format!("\r\n--{BOUNDARY}--\r\n").into_bytes());
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    b.counter(BytesCount::of_slice(&body))
        .bench(|| runtime::multipart_decode::<Note>(None, &content_type, divan::black_box(&body)).expect("a form"));
}
