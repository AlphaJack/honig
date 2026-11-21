//! WebSockets. OpenAPI cannot describe them. So honig generates nothing for them. Serve one as a plain axum route next
//! to the generated routes. Share state through your own `Arc`.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use e2e::v32_catalog::api;
use futures_util::{SinkExt, StreamExt};

use super::Catalog;

// ANCHOR: websocket
/// The API of the catalog and a WebSocket route that the description does not declare. It answers each message with
/// how many products the catalog contains.
fn with_live_updates() -> axum::Router {
    let catalog = Arc::new(Catalog::default());
    let live = {
        let catalog = Arc::clone(&catalog);
        move |ws: WebSocketUpgrade| async move { ws.on_upgrade(move |socket| count_products(socket, catalog)) }
    };
    api::Server::new(catalog).into_axum().route("/v1/live", axum::routing::get(live))
}

async fn count_products(mut socket: WebSocket, catalog: Arc<Catalog>) {
    while let Some(Ok(Message::Text(_))) = socket.recv().await {
        let count = catalog.products().map_or(0, |products| products.len());
        if socket.send(Message::Text(count.to_string().into())).await.is_err() {
            break;
        }
    }
}
// ANCHOR_END: websocket

#[tokio::test]
async fn the_websocket_route_lives_beside_the_api() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, with_live_updates()).await });
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/v1/live")).await.unwrap();
    socket.send(tokio_tungstenite::tungstenite::Message::text("count")).await.unwrap();
    let answer = socket.next().await.unwrap().unwrap();
    assert_eq!(answer.into_text().unwrap().as_str(), "0");
}
