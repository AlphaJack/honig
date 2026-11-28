//! Every path that honig follows through a body or a header. Each is a function from a size to the work it times:
//! JSON, XML, multipart, form and percent encoding, cookies, streams, queries and `uniqueItems`. `codecs` times them
//! and `scaling` counts how their cost grows.

use std::collections::BTreeMap;
use std::hint::black_box;

use honig::__private as runtime;
use runtime::{XmlField, XmlNode, XmlShape};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use bench_support::{Path, Work};

const BOUNDARY: &str = "boundary-7d1a2c";
/// Text with every class that a codec treats apart: unreserved, reserved, space, markup and non-ASCII.
const MIXED: &str = "fragile & <keep> \"upright\"; 50% off=yes/no \u{fc}n\u{ef}c\u{f6}d\u{e9} ";
/// How a streamed body arrives: 1 KiB at a time as a network delivers it.
const PIECE: usize = 1 << 10;

/// A path's input could not be built: the bench's own data is wrong.
#[expect(clippy::panic, reason = "a bench fails loudly on input it built itself")]
fn built<T, E: std::fmt::Display>(r: Result<T, E>) -> T {
    r.unwrap_or_else(|e| panic!("bench input: {e}"))
}

fn raw(size: usize) -> Vec<u8> {
    (0..size).map(|i| u8::try_from(i * 31 % 251).unwrap_or(0)).collect::<Vec<_>>()
}

/// `MIXED` repeated to about `size` bytes.
fn mixed(size: usize) -> String {
    MIXED.repeat(size.div_ceil(MIXED.len()))
}

const fn field(wire: &'static str, list: bool, child: Option<fn() -> &'static XmlNode>) -> XmlField<XmlNode> {
    XmlField {
        wire,
        name: wire,
        prefix: "",
        attribute: false,
        text: false,
        list,
        wrapped: false,
        required: true,
        item: wire,
        child,
        cdata: false,
        inline: false,
        nullable: false,
        object: false,
    }
}

#[derive(Serialize, Deserialize)]
struct Line {
    sku: String,
    qty: u32,
    note: String,
}

#[derive(Serialize, Deserialize)]
struct Order {
    id: u64,
    customer: String,
    line: Vec<Line>,
}

static LINE: XmlNode = XmlNode {
    name: "line",
    namespace: "",
    prefix: "",
    fields: &[
        XmlField { attribute: true, ..field("sku", false, None) },
        field("qty", false, None),
        field("note", false, None),
    ],
};

static ORDER: XmlNode = XmlNode {
    name: "order",
    namespace: "",
    prefix: "",
    fields: &[field("id", false, None), field("customer", false, None), field("line", true, Some(|| &LINE))],
};

impl XmlShape for Order {
    fn xml() -> &'static XmlNode {
        &ORDER
    }
}

/// The bytes of one unknown element `<u00000000/>`.
const UNKNOWN_BYTES: usize = 13;

fn order(size: usize) -> Order {
    let line = |i: usize| Line { sku: format!("SKU-{i:08}"), qty: 3, note: MIXED.to_owned() };
    let per_line =
        runtime::xml_encode(&Order { id: 7, customer: "ACME".into(), line: vec![line(0)] }).map_or(1, |b| b.len());
    Order { id: 7, customer: "ACME & sons".into(), line: (0..size / per_line).map(line).collect() }
}

#[derive(Serialize, Deserialize)]
struct Upload {
    file: runtime::Binary,
    note: String,
}

fn upload(size: usize) -> Upload {
    Upload {
        file: runtime::Binary {
            bytes: raw(size).into(),
            filename: Some("f.bin".into()),
            content_type: Some("application/octet-stream".into()),
        },
        note: MIXED.to_owned(),
    }
}

/// A form as another client sends it with a boundary of its own, a file part and a text part.
fn foreign_form(size: usize) -> (String, Vec<u8>) {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"f.bin\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend(raw(size));
    body.extend(format!("\r\n--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\n{MIXED}").bytes());
    body.extend(format!("\r\n--{BOUNDARY}--\r\n").into_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

/// Form fields of about `size` bytes in all.
fn fields(size: usize) -> BTreeMap<String, String> {
    (0..size.div_ceil(MIXED.len() + 8)).map(|i| (format!("f{i:06}"), MIXED.to_owned())).collect()
}

/// A `Cookie` header of about `size` bytes and the name of its last cookie.
fn cookies(size: usize) -> (String, String) {
    let count = size.div_ceil(24);
    let pairs: Vec<String> = (0..count).map(|i| format!("c{i:06}=v%20{i:08}")).collect();
    (pairs.join("; "), format!("c{:06}", count - 1))
}

/// About `size` bytes of small JSON items.
fn items(size: usize) -> Vec<Value> {
    let item = serde_json::json!({"id": 1, "note": MIXED});
    let count = size.div_ceil(item.to_string().len());
    vec![item; count]
}

/// A `Link` header's model of about `size` bytes: one context with many `next` links.
fn links(size: usize) -> Value {
    let next: Vec<Value> =
        (0..size.div_ceil(32)).map(|i| serde_json::json!({"href": format!("/products?page={i}")})).collect::<Vec<_>>();
    serde_json::json!({"linkset": [{"anchor": "https://shop.example.com/products", "next": next}]})
}

/// About how many bytes of path one route adds: `/r00000123`.
const ROUTE_BYTES: usize = 10;

/// A trie of `count` literal routes from `/r00000000` up and the path of its last one.
fn routes(count: usize) -> (Vec<runtime::Node>, String) {
    let names: Vec<&'static str> =
        (0..count).map(|i| &*Box::leak(format!("r{i:08}").into_boxed_str())).collect::<Vec<_>>();
    let children: Vec<(&'static str, u32)> =
        names.iter().enumerate().map(|(i, n)| (*n, u32::try_from(i + 1).unwrap_or(u32::MAX))).collect();
    let none = runtime::Node::NONE;
    let mut nodes = vec![runtime::node(Box::leak(children.into_boxed_slice()), &[], none, none)];
    nodes.extend((0..count).map(|i| runtime::node(&[], &[], none, u32::try_from(i).unwrap_or(none))));
    (nodes, format!("/{}", names.last().copied().unwrap_or_default()))
}

/// The item of an event as generated code declares it. It is a struct decoded straight from its JSON text.
#[derive(Serialize, Deserialize)]
struct Item {
    id: u32,
    note: String,
}

/// Reads every item of `body` as a stream.
fn stream(body: &[u8], framing: runtime::Framing) -> usize {
    let pieces: Vec<_> = body.chunks(PIECE).map(|p| Ok(honig::Bytes::copy_from_slice(p))).collect();
    let pieces = honig::ByteStream::chunks(futures_util::stream::iter(pieces), None);
    let mut items = runtime::ItemStream::<Value>::new(None, pieces, framing);
    let mut n = 0;
    while let Some(Ok(_)) = runtime::block_on(futures_util::StreamExt::next(&mut items)) {
        n += 1;
    }
    n
}

/// Runs `f` on `input` each time where the optimizer cannot see it.
fn on<I: 'static, R>(input: I, f: impl Fn(&I) -> R + 'static) -> Work {
    Box::new(move || {
        black_box(f(black_box(&input)));
    })
}

/// About `size` bytes of `X-Tag` lines, all of one name: a client may send any number of them.
fn tag_lines(size: usize) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    for i in 0..size.div_ceil(16) {
        headers.append("x-tag", built(http::HeaderValue::from_str(&format!("t{i:08},a"))));
    }
    headers
}

/// A media type with about `size` bytes of parameters.
fn long_media(size: usize) -> String {
    let params: String = (0..size.div_ceil(12)).map(|i| format!("; p{i:06}=v")).collect();
    format!("application/json{params}")
}

/// A `{}` response whose `Content-Type` has about `size` bytes of parameters.
fn long_media_response(size: usize) -> honig::Response {
    let mut resp = runtime::new_response(200);
    let media = built(http::HeaderValue::from_str(&long_media(size)));
    resp.headers_mut().insert(http::header::CONTENT_TYPE, media);
    *resp.body_mut() = honig::Body::from(b"{}".to_vec());
    resp
}

/// The longest target that `http::Uri` parses, with room for the path before the query.
const URI_MAX: usize = (1 << 16) - (1 << 10);

/// About `size` bytes of `name=value` pairs.
fn long_query(size: usize) -> String {
    (0..size.div_ceil(16)).map(|i| format!("k{i:08}=v{i:04}")).collect::<Vec<_>>().join("&")
}

/// About `size` bytes of comma-separated items.
fn long_list(size: usize) -> String {
    (0..size.div_ceil(10)).map(|i| format!("i{i:08}")).collect::<Vec<_>>().join(",")
}

/// About `size` bytes of digits with a point: a decimal as a client may send it.
fn long_decimal(size: usize) -> String {
    format!("1{}.{}", "0".repeat(size / 2), "5".repeat(size / 2))
}

/// A JSON object of about `size` bytes, its names `x-00000000` and up.
fn named_object(size: usize) -> String {
    let members: Vec<String> = (0..size.div_ceil(18)).map(|i| format!("\"x-{i:08}\":\"v\"")).collect();
    format!("{{{}}}", members.join(","))
}

/// The violations of a check, where generated code collects them.
fn violations() -> Vec<honig::Violation> {
    Vec::new()
}

#[cfg(feature = "regress")]
static LETTERS: runtime::Pattern = runtime::Pattern::new("^a+$");

/// An API key in the query: what `check_auth` looks for among the pairs.
static KEY_IN_QUERY: &[&[runtime::Requirement]] =
    &[&[(runtime::Scheme::ApiKey("key", runtime::KeyIn::Query, "key"), &[])]];

pub(crate) const PATHS: &[Path] = &[
    Path {
        name: "json_decode",
        prepare: |size| {
            let body = built(runtime::json_encode(&order(size)));
            (body.len(), on(body, |b| runtime::json_decode::<Order>(None, b)))
        },
    },
    Path {
        name: "json_encode_extra",
        prepare: |size| {
            // The extra map of a generated struct: each key is checked against the keys written before it.
            let extra: BTreeMap<String, u32> = (0..size / UNKNOWN_BYTES).map(|i| (format!("k{i:08}"), 1)).collect();
            let encode = |m: &BTreeMap<String, u32>| {
                let mut out = Vec::new();
                m.serialize(runtime::UniqueKeys(&mut serde_json::Serializer::new(&mut out))).ok();
                out
            };
            (size, on(extra, encode))
        },
    },
    Path {
        name: "json_encode",
        prepare: |size| {
            let order = order(size);
            (built(runtime::json_encode(&order)).len(), on(order, runtime::json_encode))
        },
    },
    Path {
        name: "xml_decode",
        prepare: |size| {
            let body = built(runtime::xml_encode(&order(size)));
            (body.len(), on(body, |b| runtime::xml_decode::<Order>(None, b)))
        },
    },
    Path {
        name: "xml_decode_repeated",
        prepare: |size| {
            // Unknown elements, then the field `id` again and again: a decoder that looks for an earlier `id` among
            // the children so far is quadratic.
            let half = size / UNKNOWN_BYTES / 2;
            let body = ["<order>", &"<u/>".repeat(half), &"<id>1</id>".repeat(half), "</order>"].concat().into_bytes();
            (body.len(), on(body, |b| runtime::xml_decode::<Order>(None, b)))
        },
    },
    Path {
        name: "xml_decode_unknown",
        prepare: |size| {
            // Elements that the shape does not list, each of its own name: a decoder that compares each with the
            // ones before it is quadratic.
            let order = String::from_utf8(built(runtime::xml_encode(&order(0)))).unwrap_or_default();
            let unknown: String = (0..size / UNKNOWN_BYTES).map(|i| format!("<u{i:08}/>")).collect();
            let body = order.replacen("</order>", &format!("{unknown}</order>"), 1).into_bytes();
            (body.len(), on(body, |b| runtime::xml_decode::<Order>(None, b)))
        },
    },
    Path {
        name: "xml_encode",
        prepare: |size| {
            let order = order(size);
            (built(runtime::xml_encode(&order)).len(), on(order, runtime::xml_encode))
        },
    },
    Path {
        name: "multipart_decode",
        prepare: |size| {
            let (content_type, body) = built(runtime::multipart_encode(&upload(size), &[]));
            (body.len(), on((content_type, body), |(ct, b)| runtime::multipart_decode::<Upload>(None, ct, b)))
        },
    },
    Path {
        name: "multipart_decode_foreign",
        prepare: |size| {
            let (content_type, body) = foreign_form(size);
            (body.len(), on((content_type, body), |(ct, b)| runtime::multipart_decode::<Upload>(None, ct, b)))
        },
    },
    Path { name: "multipart_encode", prepare: |size| (size, on(upload(size), |u| runtime::multipart_encode(u, &[]))) },
    Path {
        name: "form_decode",
        prepare: |size| {
            let body = built(runtime::form_encode(&fields(size)));
            (body.len(), on(body, |b| runtime::form_decode::<Value>(None, b)))
        },
    },
    Path { name: "form_encode", prepare: |size| (size, on(fields(size), runtime::form_encode)) },
    Path {
        name: "pct_encode",
        prepare: |size| {
            let text = mixed(size);
            (text.len(), on(text, |t| runtime::pct(t, false)))
        },
    },
    Path {
        name: "pct_decode",
        prepare: |size| {
            let text = runtime::pct(&mixed(size), false);
            (text.len(), on(text, |t| runtime::pct_decode(t)))
        },
    },
    Path {
        name: "cookie_parse",
        prepare: |size| {
            let (header, _) = cookies(size);
            (header.len(), on(header, |h| runtime::parse_cookies(h)))
        },
    },
    Path {
        name: "cookie_lookup",
        prepare: |size| {
            let (header, last) = cookies(size);
            (
                header.len(),
                on((header, last), |(h, last)| {
                    runtime::cookie_text(Some(h.as_str()), last, "/cookie/c", runtime::Style::Form)
                        .map(|c| c.map(|c| c.len()))
                }),
            )
        },
    },
    Path {
        name: "jsonl_decode",
        prepare: |size| {
            let body = built(runtime::jsonl_encode(&items(size)));
            (body.len(), on(body, |b| runtime::jsonl_decode::<Value>(None, b)))
        },
    },
    Path {
        name: "jsonl_decode_typed",
        prepare: |size| {
            let body = built(runtime::jsonl_encode(&items(size)));
            (body.len(), on(body, |b| runtime::jsonl_decode::<Item>(None, b)))
        },
    },
    Path { name: "sse_encode", prepare: |size| (size, on(items(size), |i| runtime::sse_encode(i))) },
    Path {
        name: "sse_decode",
        prepare: |size| {
            let body = built(runtime::sse_encode(&items(size)));
            (body.len(), on(body, |b| runtime::sse_decode::<Value>(None, b)))
        },
    },
    Path {
        name: "sse_decode_typed",
        prepare: |size| {
            let body = built(runtime::sse_encode(&items(size)));
            (body.len(), on(body, |b| runtime::sse_decode::<Item>(None, b)))
        },
    },
    Path {
        name: "jsonl_stream",
        prepare: |size| {
            let body = built(runtime::jsonl_encode(&items(size)));
            (body.len(), on(body, |b| stream(b, runtime::Framing::JsonLines)))
        },
    },
    Path {
        name: "multipart_stream",
        prepare: |size| {
            // One part that arrives a piece at a time: a decoder that searches it again for each piece is quadratic.
            let body = ["--b\r\nContent-Type: text/plain\r\n\r\n", &"x".repeat(size), "\r\n--b--\r\n"].concat();
            let decode = |b: &String| {
                let pieces: Vec<_> = b.as_bytes().chunks(PIECE).map(|p| Ok(honig::Bytes::copy_from_slice(p))).collect();
                let pieces = honig::ByteStream::chunks(futures_util::stream::iter(pieces), None);
                let mut parts = runtime::ItemStream::<Value>::multipart(None, pieces, "multipart/mixed; boundary=b");
                runtime::block_on(futures_util::StreamExt::next(&mut parts)).is_some()
            };
            (body.len(), on(body, decode))
        },
    },
    Path {
        name: "sse_stream",
        prepare: |size| {
            let body = built(runtime::sse_encode(&items(size)));
            (body.len(), on(body, |b| stream(b, runtime::Framing::Sse)))
        },
    },
    Path {
        name: "route_mixed",
        prepare: |size| {
            // `/{a}-{b}.git` on a segment of dashes: every split of it ends in a dash.
            static NODES: [runtime::Node; 2] = [
                runtime::node(
                    &[],
                    &[(
                        &[runtime::Part::Var, runtime::Part::Lit("-"), runtime::Part::Var, runtime::Part::Lit(".git")],
                        1,
                    )],
                    runtime::Node::NONE,
                    runtime::Node::NONE,
                ),
                runtime::node(&[], &[], runtime::Node::NONE, 0),
            ];
            let path = ["/", &"-".repeat(size / 16)].concat();
            (path.len(), on(path, |p| runtime::route(&NODES, p).is_some()))
        },
    },
    Path {
        name: "unique_items",
        prepare: |size| {
            let items: Vec<Value> =
                (0..size.div_ceil(MIXED.len())).map(|i| serde_json::json!({"i": i, "note": MIXED})).collect::<Vec<_>>();
            let check = |v: &Vec<Value>| {
                let mut out = vec![];
                runtime::check_unique(&mut out, &runtime::InstanceLocation::Root, v);
                out
            };
            (size, on(items, check))
        },
    },
    Path {
        name: "query_decode_array",
        prepare: |size| {
            let values = vec![MIXED.to_owned(); size.div_ceil(MIXED.len())];
            let mut out = vec![];
            built(runtime::encode_query(&mut out, "q", &values, runtime::Enc::FORM));
            let query = out.join("&");
            let decode = |q: &String| {
                let pairs = runtime::parse_query(q);
                runtime::decode_query(&pairs, "q", "/query/q", runtime::Style::Form, runtime::Shape::Array, true)
                    .ok()
                    .flatten()
            };
            (query.len(), on(query, decode))
        },
    },
    Path {
        name: "mixed_decode",
        prepare: |size| {
            let (content_type, body) = built(runtime::mixed_encode(&items(size), "multipart/mixed", &[]));
            (body.len(), on((content_type, body), |(ct, b)| runtime::mixed_decode::<Value>(None, ct, b, None)))
        },
    },
    Path {
        name: "json_seq_decode",
        prepare: |size| {
            let body = built(runtime::json_seq_encode(&items(size)));
            (body.len(), on(body, |b| runtime::json_seq_decode::<Value>(None, b)))
        },
    },
    Path { name: "json_seq_encode", prepare: |size| (size, on(items(size), |i| runtime::json_seq_encode(i))) },
    Path {
        name: "text_decode",
        prepare: |size| {
            let body = mixed(size).into_bytes();
            (body.len(), on(body, |b| runtime::text_decode(None, b)))
        },
    },
    Path {
        name: "char_count",
        prepare: |size| {
            let text = mixed(size);
            (text.len(), on(text, |t| runtime::char_count(t)))
        },
    },
    Path {
        name: "text_encode",
        prepare: |size| {
            let text = mixed(size);
            (text.len(), on(text, |t| runtime::text_encode(t)))
        },
    },
    Path {
        name: "linkset_decode",
        prepare: |size| {
            let header = built(runtime::linkset_encode(&links(size)));
            (header.len(), on(header, |h| runtime::linkset_decode::<Value>(None, h)))
        },
    },
    Path { name: "linkset_encode", prepare: |size| (size, on(links(size), runtime::linkset_encode)) },
    Path {
        name: "path_encode",
        prepare: |size| {
            let values = vec![MIXED.to_owned(); size.div_ceil(MIXED.len())];
            (size, on(values, |v| runtime::encode_path("ids", v, runtime::Enc::SIMPLE)))
        },
    },
    Path {
        name: "query_encode",
        prepare: |size| {
            let values = vec![MIXED.to_owned(); size.div_ceil(MIXED.len())];
            let encode = |v: &Vec<String>| {
                let mut out = vec![];
                runtime::encode_query(&mut out, "q", v, runtime::Enc::FORM).map(|()| out)
            };
            (size, on(values, encode))
        },
    },
    Path {
        name: "header_encode",
        prepare: |size| {
            let values = vec![MIXED.to_owned(); size.div_ceil(MIXED.len())];
            (size, on(values, |v| runtime::encode_header(v, runtime::Enc::SIMPLE)))
        },
    },
    Path {
        name: "cookie_encode",
        prepare: |size| {
            let values = vec![MIXED.to_owned(); size.div_ceil(MIXED.len())];
            let encode = |v: &Vec<String>| {
                let mut out = vec![];
                runtime::encode_cookie(&mut out, "c", v, runtime::Enc::FORM).map(|()| out)
            };
            (size, on(values, encode))
        },
    },
    Path { name: "base64_encode", prepare: |size| (size, on(raw(size), |b| runtime::base64(b))) },
    Path {
        name: "base64_decode",
        prepare: |size| {
            let text = runtime::base64(&raw(size));
            (text.len(), on(text, |t| runtime::unbase64(t)))
        },
    },
    Path { name: "base64url_encode", prepare: |size| (size, on(runtime::Base64Url(raw(size)), serde_json::to_vec)) },
    Path {
        name: "base64url_decode",
        prepare: |size| {
            let body = built(serde_json::to_vec(&runtime::Base64Url(raw(size))));
            (body.len(), on(body, |b| serde_json::from_slice::<runtime::Base64Url>(b)))
        },
    },
    Path {
        name: "route",
        prepare: |size| {
            let (nodes, path) = routes(size.div_ceil(ROUTE_BYTES));
            (size, on((nodes, path), |(n, p)| runtime::route(n, p).map(|(leaf, _)| leaf)))
        },
    },
    Path {
        name: "first_header",
        prepare: |size| (size, on(tag_lines(size), |h| runtime::first_header(h, "x-tag").map(|v| v.len()))),
    },
    Path {
        name: "joined_header",
        prepare: |size| (size, on(tag_lines(size), |h| runtime::joined_header(h, "x-tag").map(|v| v.len()))),
    },
    Path {
        name: "header_lines",
        prepare: |size| (size, on(tag_lines(size), |h| runtime::header_lines(h, "x-tag", true))),
    },
    Path {
        name: "check_body_media",
        prepare: |size| {
            (size, on(long_media_response(size), |r| runtime::check_body_media(r, &["application/json"]).is_ok()))
        },
    },
    Path {
        name: "check_media_of",
        prepare: |size| {
            let (head, body) = long_media_response(size).into_parts();
            (size, on((head, body), |(h, b)| runtime::check_media_of(h, b.bytes(), &["application/json"]).is_ok()))
        },
    },
    Path {
        name: "check_response_media",
        prepare: |size| {
            let media = long_media(size);
            (size, on(media, |m| runtime::check_response_media(200, m, Some(b"{}"), &["application/json"]).is_ok()))
        },
    },
    Path {
        name: "media_matches",
        prepare: |size| (size, on(long_media(size), |m| runtime::media_matches(m, "application/json"))),
    },
    Path {
        name: "check_media",
        prepare: |size| (size, on(long_media(size), |m| runtime::check_media(m, &["application/json"]).is_ok())),
    },
    Path {
        name: "negotiate",
        prepare: |size| {
            let accept: String = (0..size.div_ceil(24)).map(|i| format!("text/x{i:08};q=0.5, ")).collect();
            (size, on(accept, |a| runtime::negotiate(Some(a), &["application/json", "application/xml"])))
        },
    },
    Path {
        name: "target",
        prepare: |size| {
            // `http::Uri` refuses a longer target, so no request carries one.
            let uri: http::Uri = built(format!("/r?{}", long_query(size.min(URI_MAX))).parse());
            (size.min(URI_MAX), on(uri, |u| runtime::target("https://shop.example.com/v1", u)))
        },
    },
    Path {
        name: "normalize_path",
        prepare: |size| {
            let path = "/a/./b/../c//".repeat(size.div_ceil(13));
            (path.len(), on(path, |p| runtime::normalize_path(p).len()))
        },
    },
    Path {
        name: "split_target",
        prepare: |size| {
            let target = format!("/products?{}", long_query(size));
            (target.len(), on(target, |t| runtime::split_target(t).1.len()))
        },
    },
    Path { name: "check_query", prepare: |size| (size, on(long_query(size), |q| runtime::check_query(q).is_ok())) },
    Path {
        name: "find_method",
        prepare: |size| {
            // The method of a request is any token a client sends.
            let method = "X".repeat(size);
            let methods: &[(&str, u8)] = &[("GET", 0), ("PUT", 1), ("DELETE", 2)];
            (size, on(method, move |m| runtime::find_method(methods, m).is_some()))
        },
    },
    Path {
        name: "check_auth",
        prepare: |size| {
            let query = format!("{}&key=secret", long_query(size));
            let auth = |q: &String| {
                let pairs = runtime::parse_query(q);
                runtime::check_auth(&http::HeaderMap::new(), &pairs, KEY_IN_QUERY).is_some()
            };
            (query.len(), on(query, auth))
        },
    },
    Path {
        name: "query_text",
        prepare: |size| {
            let query = format!("{}&last=1", long_query(size));
            let text = |q: &String| {
                let pairs = runtime::parse_query(q);
                runtime::query_text(&pairs, "last", "/query/last").ok().flatten().map(|v| v.len())
            };
            (query.len(), on(query, text))
        },
    },
    Path {
        name: "decode_querystring",
        prepare: |size| (size, on(long_query(size), |q| runtime::decode_querystring(q, "/query", true).is_ok())),
    },
    Path {
        name: "decode_path",
        prepare: |size| {
            let raw = format!(".{}", long_list(size));
            let decode =
                |r: &String| runtime::decode_path(r, "/path/p", runtime::Style::Label, runtime::Shape::Array, false);
            (raw.len(), on(raw, move |r| decode(r).is_ok()))
        },
    },
    Path {
        name: "path_text",
        prepare: |size| {
            let raw = "%C3%BC".repeat(size.div_ceil(6));
            (raw.len(), on(raw, |r| runtime::path_text(r, runtime::Style::Simple).map(|v| v.len())))
        },
    },
    Path {
        name: "path_param",
        prepare: |size| {
            let raw = long_list(size);
            let param = |r: &String| {
                runtime::path_param::<Vec<String>>(&[r.as_str()], 0, runtime::Style::Simple, "/path/p").map(|v| v.len())
            };
            (raw.len(), on(raw, param))
        },
    },
    Path {
        name: "decode_header",
        prepare: |size| {
            let raw = long_list(size);
            let decode = |r: &String| {
                runtime::decode_header(Some(r), "/header/x", runtime::Style::Simple, runtime::Shape::Array, false)
                    .is_ok()
            };
            (raw.len(), on(raw, decode))
        },
    },
    Path { name: "json_header", prepare: |size| (size, on(named_object(size), |r| runtime::json_header(r))) },
    Path {
        name: "decode_cookie",
        prepare: |size| {
            let (header, _) = cookies(size);
            let decode = |h: &String| {
                let (style, shape) = (runtime::Style::Form, runtime::Shape::Object);
                runtime::decode_cookie(Some(h), "c", "/cookie/c", style, shape, true).is_ok()
            };
            (header.len(), on(header, decode))
        },
    },
    Path {
        name: "without_keys",
        prepare: |size| {
            let object: Value = built(serde_json::from_str(&named_object(size)));
            (size, on(object, |o| runtime::without_keys(Some(o.clone()), &["page", "limit"]).is_some()))
        },
    },
    Path {
        name: "from_text",
        prepare: |size| {
            let text = mixed(size);
            (text.len(), on(text, |t| runtime::from_text::<String>(Some(t.as_str().into()), "/query/q").is_ok()))
        },
    },
    Path {
        name: "from_param",
        prepare: |size| {
            let items: Value = long_list(size).split(',').map(|i| Value::String(i.to_owned())).collect();
            (size, on(items, |v| runtime::from_param::<Vec<String>>(Some(v.clone()), "/query/q").is_ok()))
        },
    },
    Path {
        name: "from_json_param",
        prepare: |size| {
            let text = Value::String(named_object(size));
            (size, on(text, |v| runtime::from_json_param::<Value>(Some(v.clone()), "/header/x").is_ok()))
        },
    },
    Path {
        name: "text_decode_as",
        prepare: |size| (size, on(mixed(size).into_bytes(), |b| runtime::text_decode_as::<String>(None, b).is_ok())),
    },
    Path {
        name: "utf8_text",
        prepare: |size| (size, on(mixed(size).into_bytes(), |b| runtime::utf8_text(None, b).map(str::len).ok())),
    },
    Path { name: "header_text", prepare: |size| (size, on(raw(size), |b| runtime::header_text(b).len())) },
    Path {
        name: "browser_refuses",
        prepare: |size| {
            let value = built(http::HeaderValue::from_str(&"v".repeat(size)));
            (size, on(value, |v| runtime::browser_refuses("x-tag", v).is_some()))
        },
    },
    Path {
        name: "linkset_header",
        prepare: |size| {
            let link: String = (0..size.div_ceil(32))
                .map(|i| format!("</products?page={i}>; rel=\"next\""))
                .collect::<Vec<_>>()
                .join(", ");
            (link.len(), on(link, |l| runtime::linkset_header(Some(l)).is_some()))
        },
    },
    Path {
        name: "part_header",
        prepare: |size| {
            let (content_type, body) = foreign_form(size);
            let header = |(ct, b): &(String, Vec<u8>)| runtime::part_header(ct, b, "note", "content-disposition");
            (body.len(), on((content_type, body), header))
        },
    },
    Path {
        name: "multipart_encode_with",
        prepare: |size| {
            let headers = [("note", "x-note", "1".to_owned())];
            (size, on(upload(size), move |u| runtime::multipart_encode_with(u, &[], &headers).is_ok()))
        },
    },
    Path {
        name: "form_encode_styled",
        prepare: |size| {
            let styles = [("f000000", runtime::Enc::FORM)];
            (size, on(fields(size), move |f| runtime::form_encode_styled(f, &styles).is_ok()))
        },
    },
    Path {
        name: "form_encode_with",
        prepare: |size| {
            let styles = [("f000000", runtime::Enc::FORM)];
            (
                size,
                on(fields(size), move |f| {
                    runtime::form_encode_with(f, &styles, &[("f000001", "application/json")]).is_ok()
                }),
            )
        },
    },
    Path {
        name: "form_decode_styled",
        prepare: |size| {
            let body = built(runtime::form_encode(&fields(size)));
            let styles = [("f000000", runtime::Style::Form, runtime::Shape::Primitive, true)];
            (body.len(), on(body, move |b| runtime::form_decode_styled::<Value>(None, b, &styles).is_ok()))
        },
    },
    Path {
        name: "form_decode_with",
        prepare: |size| {
            let body = built(runtime::form_encode(&fields(size)));
            let styles = [("f000000", runtime::Style::Form, runtime::Shape::Primitive, true)];
            (body.len(), on(body, move |b| runtime::form_decode_with::<Value>(None, b, &styles, &[]).is_ok()))
        },
    },
    Path {
        name: "json_text",
        prepare: |size| (size, on(named_object(size), |t| runtime::json_text::<Value>(t).is_ok())),
    },
    Path {
        name: "set_blob_in",
        prepare: |size| {
            let blob = |b: &Vec<u8>| {
                let mut resp = runtime::new_response(200);
                let blob = honig::Blob::from(honig::Bytes::copy_from_slice(b)).with_media("image/png");
                runtime::set_blob_in(&mut resp, "image/*", blob);
                resp
            };
            (size, on(raw(size), blob))
        },
    },
    Path {
        name: "serialize_tagged",
        prepare: |size| {
            let tagged = |o: &Order| {
                let mut out = Vec::new();
                runtime::serialize_tagged(o, "kind", "order", &mut serde_json::Serializer::new(&mut out)).ok();
                out
            };
            (size, on(order(size), tagged))
        },
    },
    Path {
        name: "not_one_of",
        prepare: |size| (size, on(mixed(size), |v| runtime::not_one_of(v, &["red", "green"]).to_string().len())),
    },
    Path {
        name: "check_format",
        prepare: |size| {
            let check = |v: &String| {
                let mut out = violations();
                runtime::check_format(&mut out, &honig::InstanceLocation::Root, v, "date-time");
                out
            };
            (size, on(mixed(size), check))
        },
    },
    Path {
        name: "check_decimal128",
        prepare: |size| {
            let check = |v: &String| {
                let mut out = violations();
                runtime::check_decimal128(&mut out, &honig::InstanceLocation::Root, v);
                out
            };
            (size, on(long_decimal(size), check))
        },
    },
    Path {
        name: "check_decimal_range",
        prepare: |size| {
            let check = |v: &String| {
                let mut out = violations();
                let bounds = runtime::Bounds { max: Some("100.5"), ..runtime::Bounds::NONE };
                runtime::check_decimal_range(&mut out, &honig::InstanceLocation::Root, v, bounds);
                out
            };
            (size, on(long_decimal(size), check))
        },
    },
    Path {
        name: "check_multiple_text",
        prepare: |size| {
            let check = |v: &String| {
                let mut out = violations();
                runtime::check_multiple_text(&mut out, &honig::InstanceLocation::Root, v, "0.25");
                out
            };
            (size, on(long_decimal(size), check))
        },
    },
    Path { name: "is_multiple", prepare: |size| (size, on(long_decimal(size), |v| runtime::is_multiple(v, "0.25"))) },
    Path {
        name: "check_multiple",
        prepare: |size| {
            // One number, whatever the size of the body: its text is in exponent form.
            let check = |v: &f64| {
                let mut out = violations();
                runtime::check_multiple(&mut out, &honig::InstanceLocation::Root, *v, "0.25");
                out
            };
            (size, on(f64::from(u32::try_from(size).unwrap_or(u32::MAX)), check))
        },
    },
    Path {
        name: "check_len",
        prepare: |size| {
            let check = |v: &String| {
                let mut out = violations();
                runtime::check_len(&mut out, &honig::InstanceLocation::Root, v.len(), Some(1), Some(10), "characters");
                out
            };
            (size, on(mixed(size), check))
        },
    },
    Path {
        name: "check_required",
        prepare: |size| {
            let object: serde_json::Map<String, Value> = built(serde_json::from_str(&named_object(size)));
            let check = |o: &serde_json::Map<String, Value>| {
                let mut out = violations();
                runtime::check_required(
                    &mut out,
                    &honig::InstanceLocation::Root,
                    |n| o.contains_key(n),
                    &["id", "name"],
                );
                out
            };
            (size, on(object, check))
        },
    },
    Path {
        name: "fits",
        prepare: |size| {
            let members = (0..size.div_ceil(18)).map(|i| (format!("x-{i:08}").into(), runtime::Content::U64(1)));
            let content = runtime::Content::Map(members.collect());
            let fields: &[(&str, bool, &[runtime::Kind])] = &[("id", false, &[runtime::Kind::Integer])];
            (size, on(content, move |c| runtime::fits(c, fields)))
        },
    },
    Path {
        name: "kept",
        prepare: |size| {
            let members = (0..size.div_ceil(18)).map(|i| (format!("x-{i:08}").into(), runtime::Content::U64(1)));
            let content = runtime::Content::Map(members.collect());
            (size, on(content, |c| runtime::kept(c, &["id", "name"], false)))
        },
    },
    #[cfg(feature = "regress")]
    Path {
        name: "pattern_matches",
        prepare: |size| (size, on("a".repeat(size), |s| runtime::pattern_matches(&LETTERS, s))),
    },
    #[cfg(feature = "regress")]
    Path {
        name: "pattern_matches_dyn",
        prepare: |size| (size, on("a".repeat(size), |s| runtime::pattern_matches_dyn("^a+$", s))),
    },
    #[cfg(feature = "regress")]
    Path {
        name: "check_pattern",
        prepare: |size| {
            let check = |s: &String| {
                let mut out = violations();
                runtime::check_pattern(&mut out, &honig::InstanceLocation::Root, &LETTERS, s);
                out
            };
            (size, on("a".repeat(size), check))
        },
    },
    #[cfg(feature = "regress")]
    Path {
        name: "pattern_entries",
        prepare: |size| {
            let read = |t: &String| {
                let mut d = serde_json::Deserializer::from_str(t);
                runtime::pattern_entries::<_, String>(&mut d, &["^x-", "^y-"], 0).map(|m| m.len()).ok()
            };
            (size, on(named_object(size), read))
        },
    },
    #[cfg(feature = "regress")]
    Path {
        name: "other_entries",
        prepare: |size| {
            let read = |t: &String| {
                let mut d = serde_json::Deserializer::from_str(t);
                runtime::other_entries::<_, String>(&mut d, &["^y-"]).map(|m| m.len()).ok()
            };
            (size, on(named_object(size), read))
        },
    },
    #[cfg(feature = "regress")]
    Path {
        name: "no_other_entries",
        prepare: |size| {
            let read = |t: &String| {
                let mut d = serde_json::Deserializer::from_str(t);
                runtime::no_other_entries(&mut d, &["^x-"]).is_ok()
            };
            (size, on(named_object(size), read))
        },
    },
];
