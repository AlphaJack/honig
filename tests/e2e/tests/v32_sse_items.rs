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

/// Answers the tile operations; the others are not reached.
struct Tiles;

impl api::Api for Tiles {
    type Principal = ();

    async fn tile_stream(
        &self,
        _: ops::tile_stream::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::tile_stream::Response, api::ApiError> {
        Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not reached"))
    }
    async fn tile_lines(
        &self,
        _: ops::tile_lines::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::tile_lines::Response, api::ApiError> {
        Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not reached"))
    }
    async fn tile_updates(
        &self,
        _: ops::tile_updates::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::tile_updates::Response, api::ApiError> {
        Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not reached"))
    }
    async fn chat_stream(
        &self,
        _: ops::chat_stream::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::chat_stream::Response, api::ApiError> {
        Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not reached"))
    }
    async fn tile_batch(
        &self,
        p: ops::tile_batch::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::tile_batch::Response, api::ApiError> {
        // Each item is checked as it is read: a body past its limits ends in an `Err`.
        p.body.collect().await.map_err(|e| api::ApiError::new(api::http::StatusCode::BAD_REQUEST, e.to_string()))?;
        Ok(ops::tile_batch::Response::NoContent)
    }
    async fn tile_top(
        &self,
        _: ops::tile_top::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::tile_top::Response, api::ApiError> {
        // Three tiles where the description allows two: the server sends what its code produces.
        Ok(ops::tile_top::Response::Ok(api::Items::List(vec![tile("a", 1), tile("b", 2), tile("c", 3)])))
    }
}

/// `minItems`, `maxItems` and `uniqueItems` of a sequence hold on both sides: the server refuses a request body past
/// them, and a client reading a response as it arrives gets an `Err` at the item past `maxItems`.
#[test]
fn a_sequence_is_held_to_its_item_limits_on_both_sides() {
    let client = api::Client::new(api::Server::new(Tiles).into_service());
    let batch = |tiles: Vec<types::TileEvent>| {
        api::block_on(client.tile_batch(ops::tile_batch::Params { body: api::Items::List(tiles) }))
    };
    assert!(matches!(batch(vec![tile("a", 1), tile("b", 2)]), Ok(ops::tile_batch::Response::NoContent)));
    for (tiles, why) in [
        (vec![tile("a", 1)], "at least 2 items required"),
        (vec![tile("a", 1), tile("b", 2), tile("c", 3), tile("d", 4)], "at most 3 items allowed"),
        (vec![tile("a", 1), tile("a", 1)], "items must be unique"),
    ] {
        let refused = batch(tiles).expect_err(why).to_string();
        assert!(refused.contains("400") && refused.contains(why), "{refused}");
    }
    let read = api::block_on(async {
        let Ok(api::Streamed::Ok(mut items)) = client.tile_top_stream(ops::tile_top::Params {}).await else {
            panic!("the stream")
        };
        let mut read = vec![];
        while let Some(item) = items.next().await {
            read.push(item.map_err(|e| e.to_string()));
        }
        read
    });
    assert_eq!(read.len(), 3, "{read:?}");
    assert_eq!(read[1], Ok(tile("b", 2)));
    assert!(read[2].as_ref().is_err_and(|e| e.contains("at most 2 items allowed")), "{read:?}");
    let whole = api::block_on(client.tile_top(ops::tile_top::Params {})).expect_err("read whole, three is past two");
    assert!(whole.to_string().contains("at most 2 items allowed"), "{whole}");
}
