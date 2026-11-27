//! Requests that break the description. They go through the tower `Service` that a framework calls. The server
//! answers each one with a status below 500 and a body that streams to the end. A panic fails the test too.
//!
//! A fixture's test file lists its routes and the words its handlers know. Then it runs [`check`] on each [`case`]
//! in a `proptest!` block. A failure shrinks to the smallest request that fails.

use std::convert::Infallible;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use proptest::prelude::*;
use proptest::sample::select;
use proptest::test_runner::{RngSeed, TestCaseError};
use tower::Service;

/// The same cases on every run, so a failure comes from a change. `PROPTEST_CASES` runs more.
pub fn config() -> ProptestConfig {
    ProptestConfig { rng_seed: RngSeed::Fixed(0), cases: 2048, ..ProptestConfig::default() }
}

/// What a fixture's server knows.
pub struct Fixture {
    /// Each operation as (method, path template).
    pub routes: &'static [(&'static str, &'static str)],
    /// Path segments its handlers answer with a body, such as an id they store.
    pub segments: &'static [&'static str],
    pub query_names: &'static [&'static str],
    pub header_names: &'static [&'static str],
}

/// Values that sit on an edge of a type, a format or an encoding.
const EDGES: &[&str] = &[
    "",
    "0",
    "-1",
    "7",
    "9223372036854775807",
    "9223372036854775808",
    "-9223372036854775809",
    "1e400",
    "0x10",
    "NaN",
    "true",
    "null",
    "%",
    "%zz",
    "%ff%fe",
    "%00",
    "%2F",
    "%E2%82%AC",
    "..",
    "a,b",
    "a|b",
    "a b",
    "[]",
    "{}",
    r#"{"term":"lamp","page":2}"#,
    "00000000-0000-0000-0000-000000000000",
    "not-a-uuid",
];

/// Headers that honig parses for every operation.
const HEADER_NAMES: &[&str] = &[
    "Accept",
    "Authorization",
    "Cookie",
    "Range",
    "If-Range",
    "If-Match",
    "If-None-Match",
    "If-Modified-Since",
    "If-Unmodified-Since",
    "Content-Encoding",
    "Accept-Encoding",
    "traceparent",
    "tracestate",
];

const HEADER_VALUES: &[&str] = &[
    "*/*",
    "application/json",
    "application/jsonl",
    "text/event-stream",
    "application/json;q=0",
    "text/html",
    "Bearer t",
    "Bearer ",
    "Basic !!!",
    "Basic",
    "session=abc",
    "session=; session=x",
    "bytes=0-",
    "bytes=-0",
    "bytes=-99",
    "bytes=0-0,1-1",
    "items=0-1",
    "\"v0\"",
    "W/\"v0\", *",
    "*",
    "Sun, 06 Nov 2026 08:49:37 GMT",
    "not a date",
    "gzip",
    "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01",
    "00-zz-zz-zz",
];

const CONTENT_TYPES: &[&str] = &[
    "application/json",
    "application/json; charset=utf-16",
    "application/jsonl",
    "application/x-www-form-urlencoded",
    "multipart/form-data; boundary=X",
    "multipart/form-data",
    "multipart/mixed; boundary=X",
    "application/octet-stream",
    "text/plain",
    "application/xml",
    "*/*",
];

const BODIES: &[&str] = &[
    "",
    "{}",
    "[]",
    "null",
    r#"["a.txt","b.txt"]"#,
    r#"{"name":"Lamp","kind":"gadget"}"#,
    r#"{"name":"Lamp","kind":"gadget","callbackUrl":"not a url"}"#,
    r#"{"name":"","kind":"x"}"#,
    r#"{"name":1e400,"kind":[]}"#,
    r#"{"name":"\ud800","kind":"book"}"#,
    r#"{"kind":"book","kind":"book"}"#,
    "{\"a\":1}\n{\"b\":\n",
    "tags=a|b&priority=9&filter[a]=x&blob=!!",
    "tags=&priority=&filter[a]",
    "priority=2&priority=3",
    "%%%&&==",
    "--X\r\nContent-Disposition: form-data; name=\"tags\"\r\n\r\na\r\n--X--\r\n",
    "--X\r\nContent-Disposition: form-data; name=\"tags\"\r\n\r\na",
    "--X\r\n\r\n--X--",
    "--Y\r\n\r\nx\r\n--Y--\r\n",
    "<product><name>Lamp</name>",
];

/// The largest byte position in a generated `Range`. Small, so that a range lands inside a short body.
const RANGE_END: u8 = 6;

/// A path segment or a query value.
fn piece(known: &'static [&'static str]) -> impl Strategy<Value = String> {
    prop_oneof![
        2 => select(EDGES).prop_map(str::to_owned),
        1 => select([known, EDGES].concat()).prop_map(str::to_owned),
        1 => "[ -~]{0,12}",
        1 => proptest::collection::vec(any::<u8>(), 0..8)
            .prop_map(|b| b.iter().map(|x| format!("%{x:02X}")).collect()),
    ]
}

fn header_value() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => select(HEADER_VALUES).prop_map(str::to_owned),
        1 => (0..RANGE_END, 0..RANGE_END).prop_map(|(first, last)| format!("bytes={first}-{last}")),
        1 => proptest::collection::vec(any::<u8>(), 0..12)
            .prop_map(|b| format!("Basic {}", honig::__private::base64(&b))),
        1 => "[ -~]{0,20}",
    ]
}

/// Escapes what `http::Uri` refuses and keeps `%` as it is, so that broken escapes reach the server.
fn uri_safe(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~%!$'()*+,;=:@".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A path template with each `{name}` replaced by a segment.
fn fill(template: &str, segments: &[String]) -> String {
    let mut out = String::new();
    let mut segments = segments.iter();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        out.push_str(&uri_safe(segments.next().map_or("", String::as_str)));
        rest = rest[open..].find('}').map_or("", |close| &rest[open + close + 1..]);
    }
    out.push_str(rest);
    out
}

#[derive(Debug)]
pub struct Case {
    method: String,
    target: String,
    headers: Vec<(&'static str, String)>,
    content_type: Option<&'static str>,
    body: Vec<u8>,
}

pub fn case(fixture: &'static Fixture) -> impl Strategy<Value = Case> {
    let method = prop_oneof![
        4 => Just(None),
        1 => select(&["GET", "HEAD", "POST", "PUT", "QUERY", "COPY", "FOO", "get"][..]).prop_map(Some),
    ];
    let segments = proptest::collection::vec(piece(fixture.segments), 1..3);
    let suffix = prop_oneof![8 => Just(""), 1 => select(&["/", "//", "/x", "/.."][..])];
    let query = proptest::collection::vec(
        prop_oneof![
            3 => (select([fixture.query_names, &["x"]].concat()), piece(fixture.segments))
                .prop_map(|(k, v)| format!("{}={}", uri_safe(k), uri_safe(&v))),
            1 => piece(fixture.segments).prop_map(|v| uri_safe(&v)),
        ],
        0..5,
    );
    let header_name = select([HEADER_NAMES, fixture.header_names].concat());
    let headers = proptest::collection::vec((header_name, header_value()), 0..5);
    let content_type = proptest::option::of(select(CONTENT_TYPES));
    let body = prop_oneof![
        3 => select(BODIES).prop_map(|b| b.as_bytes().to_vec()),
        1 => (select(BODIES), any::<prop::sample::Index>(), any::<u8>()).prop_map(|(b, at, x)| {
            let mut b = b.as_bytes().to_vec();
            if !b.is_empty() {
                let i = at.index(b.len());
                b[i] = x;
            }
            b
        }),
        1 => proptest::collection::vec(any::<u8>(), 0..64),
    ];
    (select(fixture.routes), method, segments, suffix, query, headers, content_type, body).prop_map(
        |((route_method, template), method, segments, suffix, query, headers, content_type, body)| {
            let mut target = fill(template, &segments);
            target.push_str(suffix);
            if !query.is_empty() {
                target.push('?');
                target.push_str(&query.join("&"));
            }
            Case { method: method.unwrap_or(route_method).to_owned(), target, headers, content_type, body }
        },
    )
}

/// `service` answers `case` with a status below 500 and a body that streams to the end. A case that no HTTP stack
/// would deliver passes.
pub fn check<S, B>(mut service: S, case: &Case) -> Result<(), TestCaseError>
where
    S: Service<http::Request<Full<Bytes>>, Response = http::Response<B>, Error = Infallible>,
    B: http_body::Body,
    B::Error: std::fmt::Display,
{
    let mut builder = http::Request::builder().method(case.method.as_str()).uri(case.target.as_str());
    for (name, value) in &case.headers {
        if http::HeaderValue::from_str(value).is_err() {
            return Ok(());
        }
        builder = builder.header(*name, value.as_str());
    }
    if let Some(ct) = case.content_type {
        builder = builder.header("Content-Type", ct);
    }
    let Ok(req) = builder.body(Full::new(Bytes::from(case.body.clone()))) else { return Ok(()) };
    let resp = match honig::block_on(service.call(req)) {
        Ok(resp) => resp,
        Err(never) => match never {},
    };
    let status = resp.status().as_u16();
    let body = honig::block_on(resp.into_body().collect())
        .map_err(|e| TestCaseError::fail(format!("{status}: the body failed: {e}")))?
        .to_bytes();
    prop_assert!(status < 500, "{status}: {}", String::from_utf8_lossy(&body));
    Ok(())
}
