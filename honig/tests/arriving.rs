//! Sequential request bodies that the server consumes as they arrive. They are decoded an item at a time. An item type
//! that checks its keywords checks each item as it decodes.

#![expect(clippy::unwrap_used, reason = "test code fails loudly")]

use assert_matches::assert_matches;

use futures_util::StreamExt;
use honig::__private::Limits;
use honig::__private::{Framing, block_on};
use honig::{Blob, ByteStream, Items};
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

/// An item whose `a` must not be negative. Decoding checks it, as a generated checked type does.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "Event")]
struct NonNegative(i64);

impl TryFrom<Event> for NonNegative {
    type Error = String;

    fn try_from(e: Event) -> Result<Self, String> {
        if e.a < 0 { Err(format!("{} is less than the minimum 0", e.a)) } else { Ok(Self(e.a)) }
    }
}

#[test]
fn an_item_that_breaks_a_constraint_does_not_decode_and_the_next_one_still_reads() {
    let body = Blob::stream(futures_util::stream::iter([b"{\"a\":1}\n{\"a\":-1}\n{\"a\":2}\n".to_vec()])).into_body();
    let mut items: Items<NonNegative> = Items::received(&[], body.chunks(), Framing::JsonLines).unwrap();
    let mut read = vec![];
    while let Some(item) = block_on(items.next()) {
        read.push(item.map_err(|e| e.to_string()));
    }
    assert_eq!(read.len(), 3);
    assert_eq!(read[0], Ok(NonNegative(1)));
    assert!(read[1].as_ref().is_err_and(|e| e.contains("-1 is less than the minimum 0")), "{:?}", read[1]);
    assert_eq!(read[2], Ok(NonNegative(2)));
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

/// `minItems`, `maxItems` and `uniqueItems` apply to a body as it arrives: the item past `maxItems` and an item met
/// before are an `Err`, and so is the end of a body short of `minItems`. A body received whole is checked at once.
#[test]
fn an_arriving_body_is_held_to_its_item_limits() {
    let limited = |chunks: &[&[u8]], limits: Limits| read_all(arriving(chunks).limited(limits).unwrap());
    let unique = Limits { unique: true, ..Limits::default() };
    let read = limited(&[b"{\"a\":1}\n{\"a\":1}\n{\"a\":2}\n"], unique);
    assert_eq!(read[0], Ok(Event { a: 1 }));
    assert!(read[1].as_ref().is_err_and(|e| e.contains("items must be unique")), "{read:?}");
    assert_eq!(read[2], Ok(Event { a: 2 }));
    let read = limited(&[b"{\"a\":1}\n{\"a\":2}\n{\"a\":3}\n"], Limits { max: Some(2), ..Limits::default() });
    assert_eq!(read.len(), 3);
    assert!(read[2].as_ref().is_err_and(|e| e.contains("at most 2 items allowed")), "{read:?}");
    let read = limited(&[b"{\"a\":1}\n"], Limits { min: Some(3), ..Limits::default() });
    assert_eq!(read.len(), 2, "the item, then the end short of `minItems`: {read:?}");
    assert!(read[1].as_ref().is_err_and(|e| e.contains("at least 3 items required, found 1")), "{read:?}");
    let whole: Items<Event> = Items::List(vec![Event { a: 1 }, Event { a: 1 }]);
    assert!(whole.limited(unique).is_err(), "a list is checked at once");
}
