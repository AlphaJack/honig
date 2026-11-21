//! How much memory the decoding of an XML body takes beyond the body itself. A global allocator counts it. So the
//! number is exact and the same on every machine. It is its own test binary because the allocator is process-wide.

#![expect(clippy::unwrap_used, reason = "test code fails loudly")]
#![expect(
    unsafe_code,
    reason = "a global allocator is an unsafe trait; this one only counts sizes, then calls `System`"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use honig::__private::{XmlField, XmlNode, XmlShape, xml_decode};
use serde::Deserialize;

/// Live and peak heap bytes.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

// SAFETY: every call goes to `System`; the counters only observe sizes.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
        PEAK.fetch_max(live, Ordering::Relaxed);
        // SAFETY: the caller's contract, passed on.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        // SAFETY: the caller's contract, passed on.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

#[derive(Debug, Deserialize)]
#[expect(dead_code, reason = "decoded, not read")]
struct Order {
    id: String,
    lines: Vec<Line>,
}

#[derive(Debug, Deserialize)]
#[expect(dead_code, reason = "decoded, not read")]
struct Line {
    sku: String,
    qty: i64,
    note: String,
}

const fn field(
    wire: &'static str,
    attribute: bool,
    list: bool,
    child: Option<fn() -> &'static XmlNode>,
) -> XmlField<XmlNode> {
    XmlField {
        wire,
        name: wire,
        prefix: "",
        attribute,
        text: false,
        list,
        wrapped: false,
        required: true,
        item: if list { "line" } else { wire },
        child,
        cdata: false,
        inline: false,
        nullable: false,
        object: false,
    }
}

static LINE: XmlNode = XmlNode {
    name: "line",
    namespace: "",
    prefix: "",
    fields: &[field("sku", true, false, None), field("qty", false, false, None), field("note", false, false, None)],
};

static ORDER: XmlNode = XmlNode {
    name: "order",
    namespace: "",
    prefix: "",
    fields: &[field("id", true, false, None), field("lines", false, true, Some(|| &LINE))],
};

impl XmlShape for Order {
    fn xml() -> &'static XmlNode {
        &ORDER
    }
}

/// Peak heap past which decoding has regressed.
const MAX_OVERHEAD: f64 = 5.5;

/// Lines in the body: about 4 MiB of XML.
const LINES: usize = 50_000;

/// Peak heap during the decode beyond what was live before it divided by the size of the body.
fn overhead(body: &[u8]) -> f64 {
    let before = LIVE.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let order = xml_decode::<Order>(Some(200), body).unwrap();
    let peak = PEAK.load(Ordering::Relaxed) - before;
    drop(order);
    #[expect(clippy::cast_precision_loss, reason = "a ratio of sizes far below 2^52")]
    let ratio = peak as f64 / body.len() as f64;
    ratio
}

#[test]
fn decoding_xml_takes_a_bounded_multiple_of_the_body() {
    let lines: String = (0..LINES)
        .map(|i| format!("<line sku=\"SKU-{i:08}\"><qty>{}</qty><note>handle with care</note></line>", i % 97))
        .collect();
    let body = format!("<?xml version=\"1.0\"?><order id=\"o-1\">{lines}</order>").into_bytes();
    let ratio = overhead(&body);
    println!("xml_decode: {} bytes, peak {ratio:.2}x the body", body.len());
    assert!(ratio < MAX_OVERHEAD, "decoding takes {ratio:.2}x the body, past {MAX_OVERHEAD}x");
}
