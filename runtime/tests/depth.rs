//! Every decoder that recurses as its input nests, fed an input nested far past its limit on a stack of 1 MiB: half of
//! the 2 MiB that an async runtime gives a worker thread. Each one returns: the input sets no depth of the stack.

#![cfg(all(feature = "multipart", feature = "xml"))]
#![expect(clippy::unwrap_used, reason = "test code fails loudly")]

use honig::__private as runtime;
use runtime::{XmlField, XmlNode, XmlShape};
use serde::Deserialize;
use serde_json::Value;

/// Far past every limit: a decoder that recursed once per level would overflow any stack.
const DEEP: usize = 100_000;

/// Returns `f()` run on a thread with a stack of 1 MiB.
fn small_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new().stack_size(1 << 20).spawn(f).unwrap().join().unwrap()
}

/// `open` `DEEP` times, `inner`, then `close` as often.
fn nested(open: &str, inner: &str, close: &str) -> String {
    [open.repeat(DEEP), inner.to_owned(), close.repeat(DEEP)].concat()
}

#[derive(Debug, Deserialize)]
struct Order {
    #[expect(dead_code, reason = "the shape needs a field")]
    id: Option<String>,
}

static ORDER: XmlNode = XmlNode {
    name: "order",
    namespace: "",
    prefix: "",
    fields: &[XmlField {
        wire: "id",
        name: "id",
        prefix: "",
        attribute: false,
        text: false,
        list: false,
        wrapped: false,
        required: false,
        item: "id",
        child: None,
        cdata: false,
        inline: false,
        nullable: false,
        object: false,
    }],
};

impl XmlShape for Order {
    fn xml() -> &'static XmlNode {
        &ORDER
    }
}

#[test]
fn a_json_body_nested_too_deep_is_an_error() {
    for body in [nested("[", "", "]"), nested("{\"a\":", "1", "}")] {
        assert!(small_stack(move || runtime::json_decode::<Value>(None, body.as_bytes()).is_err()));
    }
    let text = nested("[", "", "]");
    assert!(small_stack(move || serde_json::from_str::<runtime::Content<'_>>(&text).is_err()));
}

#[test]
fn an_xml_body_nested_too_deep_is_an_error() {
    let body = ["<order>", &nested("<u>", "", "</u>"), "</order>"].concat();
    assert!(small_stack(move || runtime::xml_decode::<Order>(None, body.as_bytes()).is_err()));
}

#[test]
fn a_deep_object_nested_too_deep_is_an_error() {
    let query = ["a", &"[b]".repeat(DEEP), "=1"].concat();
    assert!(small_stack(move || {
        let pairs = runtime::parse_query(&query);
        runtime::decode_query(&pairs, "a", "/a", runtime::Style::DeepObject, runtime::Shape::Object, true).is_err()
    }));
}

#[test]
fn a_multipart_body_nested_too_deep_returns() {
    let mut body = String::from("x");
    for _ in 0..DEEP / 100 {
        body = format!("--b\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n{body}\r\n--b--\r\n");
    }
    small_stack(move || {
        runtime::mixed_decode::<Value>(None, "multipart/mixed; boundary=b", body.as_bytes(), None).ok()
    });
}
