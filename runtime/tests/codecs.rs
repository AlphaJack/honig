//! Edge cases of the codecs that the runtime takes from crates (quick-xml, multer, percent-encoding,
//! `form_urlencoded` and cookie). Also edge cases of the paths that were quadratic. Each test pins behaviour that a
//! swap or a rewrite must keep.

#![expect(clippy::unwrap_used, reason = "test code fails loudly")]

use futures_util::StreamExt;
use honig::__private::{
    Framing, ItemStream, XmlField, XmlNode, XmlShape, block_on, check_unique, cookie_text, form_decode,
    form_decode_with, form_encode, jsonl_encode, mixed_decode, multipart_decode, multipart_encode_with, parse_cookies,
    parse_query, pct, pct_decode, set_items, sse_decode, xml_decode, xml_encode,
};
use honig::{ByteStream, InstancePath, Violation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Any element. There are no declared fields. So children and attributes land in an object by name.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(transparent)]
struct Any(Value);

static ANY: XmlNode = XmlNode { name: "a", namespace: "", prefix: "", fields: &[] };

impl XmlShape for Any {
    fn xml() -> &'static XmlNode {
        &ANY
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct Note {
    body: String,
}

static NOTE: XmlNode = XmlNode {
    name: "note",
    namespace: "",
    prefix: "",
    fields: &[XmlField {
        wire: "body",
        name: "body",
        prefix: "",
        attribute: false,
        text: false,
        list: false,
        wrapped: false,
        required: true,
        item: "body",
        child: None,
        cdata: true,
        inline: false,
        nullable: false,
        object: false,
    }],
};

impl XmlShape for Note {
    fn xml() -> &'static XmlNode {
        &NOTE
    }
}

/// An element whose text is an optional field.
#[derive(Debug, Deserialize, Serialize, PartialEq)]
struct Label {
    #[serde(default)]
    value: Option<String>,
}

static LABEL: XmlNode = XmlNode {
    name: "label",
    namespace: "",
    prefix: "",
    fields: &[XmlField {
        wire: "value",
        name: "value",
        prefix: "",
        attribute: false,
        text: true,
        list: false,
        wrapped: false,
        required: false,
        item: "value",
        child: None,
        cdata: false,
        inline: false,
        nullable: false,
        object: false,
    }],
};

impl XmlShape for Label {
    fn xml() -> &'static XmlNode {
        &LABEL
    }
}

#[test]
fn xml_text_fields_and_empty_elements_keep_their_text() {
    assert_eq!(xml_decode::<Label>(None, b"<label>hi</label>").unwrap(), Label { value: Some("hi".into()) });
    assert_eq!(xml_decode::<Label>(None, b"<label/>").unwrap(), Label { value: None }, "optional: absent");
    assert_eq!(xml_decode::<Note>(None, b"<note><body></body></note>").unwrap(), Note { body: String::new() });
}

fn xml(text: &str) -> Result<Any, honig::ClientError> {
    xml_decode::<Any>(None, text.as_bytes())
}

#[test]
fn xml_nested_past_the_limit_is_an_error_not_a_stack_overflow() {
    let depth = 100_000;
    let body = format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
    assert!(xml(&body).is_err());
}

#[test]
fn xml_end_tags_must_match() {
    assert!(xml("<a><b></a></b>").is_err());
    assert!(xml("<a><b></b>").is_err(), "unclosed");
}

#[test]
fn xml_entities_and_character_references_resolve() {
    let got = xml("<a><t>&lt;&amp;&#65;&#x42;&unknown;</t></a>").unwrap();
    assert_eq!(got, Any(json!({"t": "<&AB&unknown;"})));
}

#[test]
fn xml_attributes_unescape_and_nil_is_null() {
    // An element no schema describes keeps its attributes as members.
    let got =
        xml(r#"<a xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><c k="x&quot;y"/><n xsi:nil="true"/></a>"#)
            .unwrap();
    assert_eq!(got, Any(json!({"c": {"k": "x\"y"}, "n": null})));
}

#[test]
fn xml_cdata_roundtrips_its_terminator() {
    let note = Note { body: "a ]]> b <c> & d".into() };
    let bytes = xml_encode(&note).unwrap();
    assert_eq!(xml_decode::<Note>(None, &bytes).unwrap(), note);
}

const CT: &str = "multipart/form-data; boundary=b";

#[test]
fn multipart_without_a_closing_boundary_is_an_error() {
    let body = "--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\n1\r\n";
    assert!(multipart_decode::<Value>(None, CT, body.as_bytes()).is_err());
}

#[test]
fn multipart_keeps_line_endings_and_dashes_inside_a_part() {
    let body = "--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\nline\r\n\r\n-- b --\r\nend\r\n--b--\r\n";
    let got: Value = multipart_decode(None, CT, body.as_bytes()).unwrap();
    assert_eq!(got, json!({"x": "line\r\n\r\n-- b --\r\nend"}));
}

#[test]
fn multipart_repeated_names_become_a_list() {
    let part = |v: &str| format!("--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\n{v}\r\n");
    let body = format!("{}{}--b--\r\n", part("one"), part("two"));
    let got: Value = multipart_decode(None, CT, body.as_bytes()).unwrap();
    assert_eq!(got, json!({"x": ["one", "two"]}));
}

#[test]
fn mixed_parts_without_headers_are_text() {
    let body = "--b\r\n\r\nfirst\r\n--b\r\nContent-Type: application/json\r\n\r\n{\"n\":1}\r\n--b--\r\n";
    let got: Value = mixed_decode(None, "multipart/mixed; boundary=b", body.as_bytes(), None).unwrap();
    assert_eq!(got, json!(["first", {"n": 1}]));
}

#[test]
fn percent_encoding_keeps_unreserved_and_reserved_only_when_asked() {
    assert_eq!(pct("a-._~ b/ü", false), "a-._~%20b%2F%C3%BC");
    assert_eq!(pct("a/b?c", true), "a/b?c");
}

#[test]
fn percent_decoding_leaves_invalid_escapes_as_they_are() {
    assert_eq!(pct_decode("%+1%zz%4"), "%+1%zz%4");
    assert_eq!(pct_decode("%C3%BC"), "ü");
    assert_eq!(pct_decode("%FF"), "\u{fffd}", "invalid UTF-8 is replaced");
}

#[test]
fn form_decoding_reads_plus_as_space_and_bare_keys_as_empty() {
    let pairs: Vec<(String, String)> =
        parse_query("a+b=c%20d&e&&f=").into_iter().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    assert_eq!(pairs, [("a b", "c d"), ("e", ""), ("f", "")].map(|(k, v)| (k.to_owned(), v.to_owned())));
    let got: Value = form_decode(None, b"x=one&x=two").unwrap();
    assert_eq!(got, json!({"x": ["one", "two"]}));
}

#[test]
fn form_encoding_writes_space_as_plus() {
    let body = form_encode(&json!({"q": "a b&c"})).unwrap();
    assert_eq!(String::from_utf8(body).unwrap(), "q=a+b%26c");
}

#[test]
fn cookies_trim_spaces_skip_bare_names_and_percent_decode_by_name() {
    let header = " a = 1 ;bare; b=%20x ";
    assert_eq!(parse_cookies(header), [("a".to_owned(), "1".to_owned()), ("b".to_owned(), "%20x".to_owned())]);
    assert_eq!(cookie_text(Some(header), "b").as_deref(), Some(" x"));
}

fn unique(items: &[Value]) -> bool {
    let mut out: Vec<Violation> = vec![];
    check_unique(&mut out, &InstancePath::Root, items);
    out.is_empty()
}

#[test]
fn unique_items_compare_as_json_values() {
    assert!(!unique(&[json!(1), json!(1.0)]), "1 and 1.0 are one number");
    assert!(!unique(&[json!(0), json!(-0.0)]));
    assert!(!unique(&[json!({"a": 1, "b": 2}), json!({"b": 2, "a": 1})]), "key order does not matter");
    assert!(unique(&[json!("1"), json!(1)]));
    assert!(unique(&[json!([1, 2]), json!([2, 1])]), "array order matters");
}

/// A body that arrives one byte at a time: every split point a network can produce.
fn byte_by_byte(body: &[u8]) -> ByteStream {
    let bytes: Vec<_> = body.iter().map(|b| Ok(honig::Bytes::from(vec![*b]))).collect();
    ByteStream::chunks(futures_util::stream::iter(bytes), None)
}

fn streamed(body: &[u8], framing: Framing) -> Vec<Value> {
    let mut items = ItemStream::<Value>::new(Some(200), byte_by_byte(body), framing);
    let mut out = vec![];
    while let Some(item) = block_on(items.next()) {
        out.push(item.unwrap());
    }
    out
}

#[test]
fn sse_split_at_every_byte_matches_the_whole_body() {
    let body = b"\xEF\xBB\xBFdata: 1\r\n\r\n: heartbeat\n\ndata: 2\r\rdata: 3\n\r\ndata: 4";
    assert_eq!(streamed(body, Framing::Sse), sse_decode::<Value>(Some(200), body).unwrap());
    // The body ends inside the last event. That event is dropped.
    assert_eq!(streamed(body, Framing::Sse), [1, 2, 3].map(|n| json!({"data": n})));
}

/// The streaming multipart examples of 3.2. A part at a time gives the same items as the whole body whatever the split.
/// The preamble and the epilogue carry none.
#[test]
fn multipart_split_at_every_byte_matches_the_whole_body() {
    let ct = "multipart/mixed; boundary=b0und";
    let body = b"preamble\r\n--b0und\r\nContent-Type: application/json\r\n\r\n{\"n\":1}\r\n--b0und\r\n\
                 Content-Type: text/plain\r\n\r\ntwo\r\n--b0und\r\nContent-Type: application/json\r\n\r\n[3]\r\n\
                 --b0und--\r\nepilogue";
    let mut items = ItemStream::<Value>::multipart(Some(200), byte_by_byte(body), ct);
    let mut streamed = vec![];
    while let Some(item) = block_on(items.next()) {
        streamed.push(item.unwrap());
    }
    assert_eq!(streamed, mixed_decode::<Vec<Value>>(Some(200), ct, body, None).unwrap());
    assert_eq!(streamed, [json!({"n": 1}), json!("two"), json!([3])]);
}

#[test]
fn json_lines_split_at_every_byte_match_the_whole_body() {
    let body = b"1\n\n{\"a\":2}\r\n[3]";
    assert_eq!(streamed(body, Framing::JsonLines), [json!(1), json!({"a": 2}), json!([3])]);
}

/// Every piece of a streamed response body in order.
fn pieces(resp: &honig::Response) -> Vec<honig::Bytes> {
    let mut chunks = resp.body().chunks().clone();
    let mut out = vec![];
    while let Some(piece) = block_on(chunks.next()) {
        out.push(piece.unwrap());
    }
    out
}

#[test]
fn a_streamed_iterator_goes_out_in_few_pieces_with_the_same_bytes() {
    let items: Vec<u32> = (0..10_000).collect();
    let mut resp = honig::__private::new_response(200);
    let source = honig::Items::stream(futures_util::stream::iter(items.clone()));
    set_items(&mut resp, "application/jsonl", source, Framing::JsonLines).unwrap();
    let got = pieces(&resp);
    assert!(got.len() < items.len() / 100, "{} pieces for {} items", got.len(), items.len());
    assert_eq!(got.concat(), jsonl_encode(&items).unwrap());
}

/// Pending on its first poll and ready on the next. That is an item that a feed is still waiting for.
struct Later(bool);

impl Future for Later {
    type Output = ();
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    }
}

#[test]
fn a_streamed_item_goes_out_without_waiting_for_the_next() {
    let source = futures_util::stream::unfold(0_u32, |n| async move {
        if n > 0 {
            Later(false).await;
        }
        (n < 3).then_some((n, n + 1))
    });
    let mut resp = honig::__private::new_response(200);
    set_items(&mut resp, "application/jsonl", honig::Items::stream(source), Framing::JsonLines).unwrap();
    assert_eq!(pieces(&resp), [b"0\n".to_vec(), b"1\n".to_vec(), b"2\n".to_vec()]);
}

#[test]
fn a_required_array_parameter_that_is_not_sent_is_missing() {
    use honig::__private::from_param;
    assert_eq!(
        from_param::<Vec<i64>>(None, "/query/ids").unwrap_err(),
        honig::Reason::invalid("/query/ids", "is required")
    );
    assert_eq!(from_param::<Option<Vec<i64>>>(None, "/query/ids").unwrap(), None);
    assert_eq!(from_param::<Vec<i64>>(Some(json!(["1", "2"])), "/query/ids").unwrap(), [1, 2]);
}

#[test]
fn a_body_of_a_media_range_may_come_as_octet_stream() {
    use honig::__private::check_media;
    // What a client sends when the description gives only `image/*`.
    assert!(check_media("application/octet-stream", &["image/*"]).is_ok());
    assert!(check_media("image/png", &["image/*"]).is_ok());
    assert!(check_media("application/octet-stream", &["application/json"]).is_err(), "only for a range");
}

#[test]
fn a_body_that_says_it_is_not_utf8_is_refused() {
    use honig::__private::check_media;
    assert!(check_media("application/json; charset=utf-16", &["application/json"]).is_err());
    assert!(check_media("text/plain; charset=ISO-8859-1", &["text/plain"]).is_err());
    assert!(check_media("application/json; charset=\"UTF-8\"", &["application/json"]).is_ok());
    assert!(check_media("text/plain; charset=us-ascii", &["text/plain"]).is_ok());
    assert!(check_media("application/json", &["application/json"]).is_ok());
}

/// The stack a tokio worker thread gets.
const WORKER_STACK: usize = 2 << 20;

#[test]
fn deeply_nested_json_is_an_error_on_a_worker_sized_stack() {
    #[derive(Debug, Deserialize)]
    struct Holder {
        #[expect(dead_code, reason = "decoded to be refused")]
        a: Value,
    }
    let depth = 10_000;
    let body = format!("{{\"a\":{}{}}}", "[".repeat(depth), "]".repeat(depth));
    let decoded = std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn(move || honig::__private::json_decode::<Holder>(None, body.as_bytes()).is_err())
        .unwrap()
        .join();
    assert!(decoded.unwrap(), "decoded, or overflowed the stack");
}

fn format_ok(value: &str, format: &str) -> bool {
    let mut out: Vec<Violation> = vec![];
    honig::__private::check_format(&mut out, &InstancePath::Root, value, format);
    out.is_empty()
}

#[test]
fn formats_without_a_library_are_checked_as_their_library_would_parse_them() {
    assert!(format_ok("-12.50", "decimal") && !format_ok("1e3", "decimal") && !format_ok("1.", "decimal"));
    assert!(format_ok("anything", "x-unknown"), "a format honig does not know passes");
}

#[test]
fn a_uuid_is_only_its_hyphenated_form() {
    assert!(format_ok("01234567-89AB-4def-8123-456789abcdef", "uuid"));
    for other in [
        "0123456789ab",
        "0123456789ab4def8123456789abcdef",
        "{01234567-89ab-4def-8123-456789abcdef}",
        "urn:uuid:01234567-89ab-4def-8123-456789abcdef",
        "0123456-789ab-4def-8123-456789abcdef",
        "01234567-89ab-4def-8123-456789abcdeg",
    ] {
        assert!(!format_ok(other, "uuid"), "{other}");
    }
}

/// RFC 3339 by its grammar: `T` (or `t`) between date and time.
#[test]
fn dates_and_times_are_checked_by_rfc_3339() {
    let ok = |format: &str, values: &[&str]| values.iter().all(|v| format_ok(v, format));
    let refused = |format: &str, values: &[&str]| values.iter().all(|v| !format_ok(v, format));
    assert!(ok("date", &["2024-02-29"]) && refused("date", &["2023-02-29", "2024-13-45", "2024-1-5", "20240105"]));
    assert!(ok("date-time", &["2024-06-01T12:30:00Z", "2024-06-01t12:30:00.5+02:00", "2016-12-31T23:59:60Z"]));
    let bad_date_times = [
        "2024-06-01T12:30:00",
        "2024-06-01T12:30Z",
        "2024-06-01T12:30:00+0200",
        "2024-06-01T12:59:60Z",
        "2024-06-01 12:30:00Z",
    ];
    assert!(refused("date-time", &bad_date_times), "an offset, seconds, and a leap second only where one falls");
    assert!(ok("time", &["12:30:00Z", "12:30:00.5Z", "12:30:00-05:00", "23:59:60Z"]));
    assert!(
        refused("time", &["12:30:00", "24:00:00Z", "12:30Z", "22:59:60Z"]),
        "an offset, and a leap second only at 23:59 UTC"
    );
    assert!(ok("time-local", &["12:30:00"]) && refused("time-local", &["12:30:00Z", "12:30:00+02:00"]));
    assert!(ok("date-time-local", &["2024-06-01T12:30:00.5", "2024-06-01t12:30:00"]));
    assert!(refused("date-time-local", &["2024-06-01T12:30:00Z", "2024-06-01T12:30:00+02:00", "2024-06-01"]));
}

/// 3.2: `contentEncoding` on a multipart field means a `Content-Transfer-Encoding` header. A part sent base64 or
/// quoted-printable arrives as its bytes. A field declared base64 goes out encoded.
#[test]
fn a_transfer_encoded_part_is_decoded_and_a_declared_one_encoded() {
    #[derive(Debug, PartialEq, Deserialize, Serialize)]
    struct Upload {
        note: String,
        data: String,
    }
    let ct = "multipart/form-data; boundary=xyz";
    let body = b"--xyz\r\nContent-Disposition: form-data; name=\"note\"\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n\
                 caf=C3=A9 =\r\nau lait\r\n--xyz\r\nContent-Disposition: form-data; name=\"data\"\r\n\
                 Content-Transfer-Encoding: base64\r\n\r\naGVs\r\nbG8=\r\n--xyz--\r\n";
    let got: Upload = multipart_decode(Some(200), ct, body).unwrap();
    assert_eq!(got, Upload { note: "café au lait".into(), data: "hello".into() });
    let headers = [("data", "Content-Transfer-Encoding", "base64".to_owned())];
    let (sent_ct, sent) = multipart_encode_with(&got, &[], &headers).unwrap();
    let text = String::from_utf8_lossy(&sent);
    assert!(text.contains("Content-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n"), "{text}");
    assert_eq!(multipart_decode::<Upload>(Some(200), &sent_ct, &sent).unwrap(), got);
}

#[test]
fn a_one_value_query_parameter_sent_twice_is_refused() {
    use honig::__private::{Shape, Style, decode_query, query_text};
    use honig::Reason;
    let pairs = parse_query("a=1&a=2&tags=x&tags=y&f%5Bk%5D=1&f%5Bk%5D=2&d=x%20y&d=z");
    // Servers disagree on which value counts (first or last). So none is picked.
    let twice = Reason::invalid("/query/a", "is sent more than once, but it holds one value");
    assert_eq!(query_text(&pairs, "a", "/query/a").unwrap_err(), twice);
    assert_eq!(decode_query(&pairs, "a", "/query/a", Style::Form, Shape::Prim, true).unwrap_err(), twice);
    assert!(decode_query(&pairs, "d", "/query/d", Style::SpaceDelimited, Shape::Array, false).is_err());
    // An exploded array and a deepObject key that holds a list are many values by design.
    assert_eq!(
        decode_query(&pairs, "tags", "/query/tags", Style::Form, Shape::Array, true).unwrap(),
        Some(serde_json::json!(["x", "y"]))
    );
    assert!(decode_query(&pairs, "f", "/query/f", Style::DeepObject, Shape::Object, true).unwrap().is_some());
    let once = parse_query("a=1");
    assert_eq!(query_text(&once, "a", "/query/a").unwrap().as_deref(), Some("1"));
    assert_eq!(query_text(&once, "b", "/query/b").unwrap(), None);
}

#[test]
fn a_response_of_an_undeclared_media_type_is_refused() {
    use honig::__private::check_response_media;
    use honig::ClientError;
    let json = ["application/json"];
    assert!(check_response_media(200, "application/json; charset=utf-8", Some(b"{}"), &json).is_ok());
    // A plain-text 404 under an operation that declares JSON.
    let refused = check_response_media(404, "text/plain", Some(b"404 page not found"), &json).unwrap_err();
    assert!(
        matches!(&refused, ClientError::UnexpectedMedia { status: 404, content_type, declared, body }
            if content_type == "text/plain" && declared == "application/json" && body == b"404 page not found".as_slice()),
        "{refused:?}"
    );
    assert_eq!(refused.to_string(), "unexpected media type in 404 response: text/plain, declared application/json");
    // A body without a type is the one type declared (the `OK` of a health check). With two there is no telling
    // which. An empty body needs no type.
    assert!(check_response_media(200, "", Some(b"OK"), &["text/plain"]).is_ok());
    assert!(check_response_media(200, "", Some(b"{}"), &json).is_ok());
    assert!(check_response_media(200, "", Some(b"{}"), &["application/json", "application/xml"]).is_err());
    assert!(check_response_media(200, "", Some(b""), &["application/json", "application/xml"]).is_ok());
    // A stream's body is unread. It is the one type declared or it must name its type.
    assert!(check_response_media(200, "", None, &["text/event-stream"]).is_ok());
    assert!(check_response_media(200, "", None, &["text/event-stream", "application/jsonl"]).is_err());
    assert!(check_response_media(200, "image/png", None, &["image/*"]).is_ok());
    // Read as UTF-8. So another charset is refused.
    assert!(check_response_media(200, "text/plain; charset=latin1", Some(b"x"), &["text/plain"]).is_err());
}

#[test]
fn an_event_whose_data_is_not_json_says_so() {
    // A chat chunk written whole as JSON in `data`. An `id` alone would read as a field of the event itself.
    #[derive(Debug, serde::Deserialize)]
    #[expect(dead_code, reason = "decoded, not read")]
    struct Chunk {
        id: String,
        object: String,
    }
    // An end marker that no schema describes.
    let body = b"data: {\"id\":\"a\",\"object\":\"chunk\"}\n\ndata: [DONE]\n\n";
    let e = sse_decode::<Chunk>(Some(200), body).unwrap_err();
    assert!(e.to_string().contains("the event's data is not JSON: `[DONE]`"), "{e}");
}

/// A `Decimal` keeps its digits in a form field and a multipart part as in a JSON body.
#[test]
fn a_decimal_keeps_its_digits_in_a_form_and_a_multipart_part() {
    #[derive(Deserialize)]
    struct Holder {
        d: honig::Decimal,
    }
    const DIGITS: &str = "12345678901234567.89";
    let form: Holder = form_decode(Some(200), format!("d={DIGITS}").as_bytes()).unwrap();
    assert_eq!(form.d.as_str(), DIGITS);
    let body = format!("--b\r\nContent-Disposition: form-data; name=\"d\"\r\n\r\n{DIGITS}\r\n--b--\r\n");
    let part: Holder = multipart_decode(Some(200), "multipart/form-data; boundary=b", body.as_bytes()).unwrap();
    assert_eq!(part.d.as_str(), DIGITS);
}

/// Body pieces and then, when `open`, a connection that stays open.
fn arriving(pieces: Vec<String>, open: bool) -> ByteStream {
    let pieces = futures_util::stream::iter(pieces.into_iter().map(|p| Ok(honig::Bytes::from(p))));
    if open {
        ByteStream::chunks(pieces.chain(futures_util::stream::pending()), None)
    } else {
        ByteStream::chunks(pieces, None)
    }
}

const MIXED: &str = "--b\r\nContent-Type: application/json\r\n\r\n{\"a\":1}\r\n--b\r\nContent-Type: application/json\r\n\r\n{\"a\":2}\r\n--b--\r\n";

#[test]
fn multipart_items_split_byte_by_byte_arrive_whole() {
    let bytes: Vec<String> = MIXED.chars().map(String::from).collect();
    let items: ItemStream<Value> =
        ItemStream::multipart(Some(200), arriving(bytes, false), "multipart/mixed; boundary=b");
    let got: Vec<Value> = block_on(items.map(Result::unwrap).collect());
    assert_eq!(got, [json!({"a": 1}), json!({"a": 2})]);
}

#[test]
fn a_multipart_item_arrives_before_the_body_ends() {
    use futures_util::FutureExt;
    let (first, _) = MIXED.split_at(MIXED.find("{\"a\":2}").unwrap());
    let mut items: ItemStream<Value> =
        ItemStream::multipart(Some(200), arriving(vec![first.to_owned()], true), "multipart/mixed; boundary=b");
    assert_eq!(items.next().now_or_never().map(|i| i.map(Result::unwrap)), Some(Some(json!({"a": 1}))));
    assert!(items.next().now_or_never().is_none(), "the second part has not arrived");
}

/// A form field that its type refuses is named by its pointer, as in a JSON body. Only the failing body is read again
/// to find it.
#[test]
fn a_form_field_that_does_not_decode_is_named() {
    #[derive(Debug, Deserialize)]
    struct Member {
        #[expect(dead_code, reason = "decoded only to fail on `age`")]
        name: String,
        #[expect(dead_code, reason = "decoded only to fail on `age`")]
        age: u8,
    }
    let pointer = |e: honig::ClientError| {
        let honig::ClientError::Decode { pointer, .. } = e else { panic!("not a decode error: {e}") };
        pointer
    };
    let err = form_decode::<Member>(None, b"name=Ann&age=300").expect_err("300 is past u8");
    assert_eq!(pointer(err), "/age");
    let err = form_decode_with::<Member>(None, b"name=Ann&age=300", &[], &[]).expect_err("300 is past u8");
    assert_eq!(pointer(err), "/age");
}
