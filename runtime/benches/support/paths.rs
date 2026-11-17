//! Every path that the runtime takes through a body or a header. Each is a function from a size to the work it times:
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

/// An order of about `size` bytes of XML.
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
    let mut items = runtime::ItemStream::<Value>::new(0, pieces, framing);
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

pub(crate) const PATHS: &[Path] = &[
    Path {
        name: "json_decode",
        prepare: |size| {
            let body = built(runtime::json_encode(&order(size)));
            (body.len(), on(body, |b| runtime::json_decode::<Order>(0, b)))
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
            (body.len(), on(body, |b| runtime::xml_decode::<Order>(0, b)))
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
            (body.len(), on(body, |b| runtime::xml_decode::<Order>(0, b)))
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
            (body.len(), on((content_type, body), |(ct, b)| runtime::multipart_decode::<Upload>(0, ct, b)))
        },
    },
    Path {
        name: "multipart_decode_foreign",
        prepare: |size| {
            let (content_type, body) = foreign_form(size);
            (body.len(), on((content_type, body), |(ct, b)| runtime::multipart_decode::<Upload>(0, ct, b)))
        },
    },
    Path { name: "multipart_encode", prepare: |size| (size, on(upload(size), |u| runtime::multipart_encode(u, &[]))) },
    Path {
        name: "form_decode",
        prepare: |size| {
            let body = built(runtime::form_encode(&fields(size)));
            (body.len(), on(body, |b| runtime::form_decode::<Value>(0, b)))
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
                on((header, last), |(h, last)| runtime::cookie_text(Some(h.as_str()), last).map(|c| c.len())),
            )
        },
    },
    Path {
        name: "jsonl_decode",
        prepare: |size| {
            let body = built(runtime::jsonl_encode(&items(size)));
            (body.len(), on(body, |b| runtime::jsonl_decode::<Value>(0, b)))
        },
    },
    Path {
        name: "jsonl_decode_typed",
        prepare: |size| {
            let body = built(runtime::jsonl_encode(&items(size)));
            (body.len(), on(body, |b| runtime::jsonl_decode::<Item>(0, b)))
        },
    },
    Path { name: "sse_encode", prepare: |size| (size, on(items(size), |i| runtime::sse_encode(i))) },
    Path {
        name: "sse_decode",
        prepare: |size| {
            let body = built(runtime::sse_encode(&items(size)));
            (body.len(), on(body, |b| runtime::sse_decode::<Value>(0, b)))
        },
    },
    Path {
        name: "sse_decode_typed",
        prepare: |size| {
            let body = built(runtime::sse_encode(&items(size)));
            (body.len(), on(body, |b| runtime::sse_decode::<Item>(0, b)))
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
        name: "sse_stream",
        prepare: |size| {
            let body = built(runtime::sse_encode(&items(size)));
            (body.len(), on(body, |b| stream(b, runtime::Framing::Sse)))
        },
    },
    Path {
        name: "unique_items",
        prepare: |size| {
            let items: Vec<Value> =
                (0..size.div_ceil(MIXED.len())).map(|i| serde_json::json!({"i": i, "note": MIXED})).collect::<Vec<_>>();
            let check = |v: &Vec<Value>| {
                let mut out = vec![];
                runtime::check_unique(&mut out, &runtime::InstancePath::Root, v);
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
            (body.len(), on((content_type, body), |(ct, b)| runtime::mixed_decode::<Value>(0, ct, b, None)))
        },
    },
    Path {
        name: "json_seq_decode",
        prepare: |size| {
            let body = built(runtime::json_seq_encode(&items(size)));
            (body.len(), on(body, |b| runtime::json_seq_decode::<Value>(0, b)))
        },
    },
    Path { name: "json_seq_encode", prepare: |size| (size, on(items(size), |i| runtime::json_seq_encode(i))) },
    Path {
        name: "text_decode",
        prepare: |size| {
            let body = mixed(size).into_bytes();
            (body.len(), on(body, |b| runtime::text_decode(b)))
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
            (header.len(), on(header, |h| runtime::linkset_decode::<Value>(0, h)))
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
            (size, on(values, |v| runtime::encode_header(v, false)))
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
        name: "validate_untyped",
        prepare: |size| {
            let schema = serde_json::json!({"type": "array", "uniqueItems": false, "items": {"type": "object",
                "required": ["id"], "properties": {"id": {"type": "integer", "minimum": 0},
                "note": {"type": "string", "maxLength": 200}}}});
            let check = |(value, schema): &(Value, Value)| {
                let mut out = vec![];
                runtime::check_schema(&mut out, &runtime::InstancePath::Root, value, schema);
                out
            };
            (size, on((Value::Array(items(size)), schema), check))
        },
    },
];
