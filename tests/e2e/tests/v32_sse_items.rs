//! A non-array `schema` on SSE or JSON Lines types each item. SSE comment
//! blocks (heartbeats) carry no item.

use e2e::v32_sse_items::api::{self, operations as ops, types};
use futures_util::StreamExt;

/// Bytes in the pieces a server flushes them in.
fn pieces(pieces: Vec<&'static [u8]>) -> api::ByteStream {
    let pieces = pieces.into_iter().map(|p| Ok(api::Bytes::from_static(p)));
    api::ByteStream::chunks(futures_util::stream::iter(pieces), None)
}

fn tile(tile: &str, rows: i64) -> types::TileEvent {
    types::TileEvent { tile: tile.to_owned(), rows }
}

#[test]
fn a_non_array_schema_types_each_item() {
    // Compiles only when the bodies are `Items<TileEvent>` and not `Items<Value>`.
    let _ = ops::tile_stream::Response::Ok(api::Items::List(vec![tile("a", 1)]));
    let _ = ops::tile_lines::Response::Ok(api::Items::List(vec![tile("a", 1)]));
}

#[test]
fn heartbeats_are_skipped_while_streaming() {
    let chunks: Vec<&'static [u8]> = vec![
        b":heartbeat\n\n",
        b"data: {\"tile\":\"a\",\"rows\":1}\n\n: keep-alive\n",
        b"\n",
        b"data: {\"tile\":\"b\",\"rows\":2}\n\n:heartbeat\n\n",
    ];
    let mut stream = api::ItemStream::<types::TileEvent>::new(200, pieces(chunks), api::Framing::Sse);
    let mut got = vec![];
    while let Some(item) = api::block_on(stream.next()) {
        got.push(item.expect("every block left is an event"));
    }
    assert_eq!(got, vec![tile("a", 1), tile("b", 2)]);
}

#[test]
fn heartbeats_are_skipped_in_a_buffered_body() {
    let body = b":heartbeat\n\ndata: {\"tile\":\"a\",\"rows\":1}\n\n:heartbeat\n\n";
    let got: Vec<types::TileEvent> = api::runtime::sse_decode(200, body).expect("decodes");
    assert_eq!(got, vec![tile("a", 1)]);
}
