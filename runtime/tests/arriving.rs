//! Sequential request bodies that the server reads as they arrive. They are decoded an item at a time and each item is
//! checked.

#![expect(clippy::unwrap_used, reason = "test code fails loudly")]

use assert_matches::assert_matches;

use futures_util::StreamExt;
use honig::__private::{Framing, block_on};
use honig::{Blob, ByteStream, Error, InstancePath, Items, Violation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Event {
    a: i64,
}

/// A body that the adapter left unread. It arrives in `chunks`.
fn arriving(chunks: &[&[u8]]) -> Items<Event> {
    let chunks: Vec<Vec<u8>> = chunks.iter().map(|c| c.to_vec()).collect();
    let body = Blob::stream(futures_util::stream::iter(chunks)).into_body();
    Items::received(&[], body.chunks(), Framing::JsonLines).unwrap()
}

/// Every result that the stream gives up to the end.
fn read_all(mut items: Items<Event>) -> Vec<Result<Event, String>> {
    let mut out = vec![];
    while let Some(item) = block_on(items.next()) {
        out.push(item.map_err(|e| e.to_string()));
    }
    out
}

#[test]
fn items_split_across_chunks_arrive_whole() {
    let items = arriving(&[b"{\"a\":", b"1}\n{\"a\":2}\n{", b"\"a\":3}"]);
    assert_matches!(items, Items::Stream(_));
    assert_eq!(block_on(items.collect()).unwrap(), vec![Event { a: 1 }, Event { a: 2 }, Event { a: 3 }]);
}

#[test]
fn a_body_read_whole_is_a_list() {
    let items: Items<Event> =
        Items::received(b"{\"a\":1}\n{\"a\":2}\n", &ByteStream::default(), Framing::JsonLines).unwrap();
    assert_eq!(items, Items::List(vec![Event { a: 1 }, Event { a: 2 }]));
}

/// `a` must not be negative as a generated check would say.
fn non_negative(e: &Event, path: &InstancePath<'_>, out: &mut Vec<Violation>) {
    if e.a < 0 {
        out.push(Violation { path: path.to_string(), message: "must be at least 0".to_owned() });
    }
}

#[test]
fn an_item_that_breaks_a_constraint_is_invalid_and_the_next_one_still_reads() {
    let items = arriving(&[b"{\"a\":1}\n{\"a\":-1}\n{\"a\":2}\n"]);
    let mut found = vec![];
    items.check_each(&InstancePath::Key(&InstancePath::Root, "body"), &mut found, non_negative);
    assert_eq!(found, vec![], "nothing is checked before the items arrive");
    assert_eq!(
        read_all(items),
        vec![
            Ok(Event { a: 1 }),
            Err("invalid request item; /body/1: must be at least 0".to_owned()),
            Ok(Event { a: 2 })
        ]
    );
}

#[test]
fn an_invalid_item_carries_its_violations() {
    let mut items = arriving(&[b"{\"a\":-1}\n"]);
    items.check_each(&InstancePath::Key(&InstancePath::Root, "body"), &mut vec![], non_negative);
    let violations = match block_on(items.next()) {
        Some(Err(Error::Invalid(v))) => Some(v),
        _ => None,
    };
    assert_eq!(
        violations,
        Some(vec![Violation { path: "/body/0".to_owned(), message: "must be at least 0".to_owned() }])
    );
}

#[test]
fn a_list_is_checked_at_once() {
    let items = Items::List(vec![Event { a: 1 }, Event { a: -1 }]);
    let mut found = vec![];
    items.check_each(&InstancePath::Key(&InstancePath::Root, "body"), &mut found, non_negative);
    assert_eq!(found, vec![Violation { path: "/body/1".to_owned(), message: "must be at least 0".to_owned() }]);
}

#[test]
fn an_item_that_does_not_decode_is_an_error_and_the_next_one_still_reads() {
    let results = read_all(arriving(&[b"{\"a\":1}\n{\"a\":\"x\"}\n{\"a\":2}\n"]));
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], Ok(Event { a: 1 }));
    assert!(results[1].as_ref().is_err_and(|e| e.starts_with("cannot decode request")), "{:?}", results[1]);
    assert_eq!(results[2], Ok(Event { a: 2 }));
}

#[test]
fn an_arriving_body_passes_on_what_the_handler_has_not_read() {
    let mut items = arriving(&[b"{\"a\":1}\n{\"a\"", b":2}\n{\"a\":-3}\n"]);
    assert_eq!(block_on(items.next()).map(Result::unwrap), Some(Event { a: 1 }));
    let body = items.into_body(Framing::JsonLines).unwrap();
    assert_eq!(body.bytes(), b"");
    assert_eq!(block_on(body.collect()).unwrap(), b"{\"a\":2}\n{\"a\":-3}\n".as_slice());
}
