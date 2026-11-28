//! The client page of the book on the catalog: the response media type, a conditional request, links, callbacks,
//! event streams and `into_result`. Each runs against a server in memory that answers as the catalog would.

use std::time::Duration;

use assert_matches::assert_matches;
use futures_util::StreamExt;

use super::{api, lamp, ops, types};

/// A client whose requests `answer` handles in memory.
fn catalog<F, Fut>(answer: F) -> api::Client<impl api::Transport>
where
    F: Fn(api::Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = api::Response> + Send + 'static,
{
    api::Client::new(crate::support::loopback(answer)).with_api_key("secret")
}

/// The value of header `name` in `req`.
fn header<'r>(req: &'r api::Request, name: &str) -> Option<&'r str> {
    req.headers().get(name)?.to_str().ok()
}

/// A response with `status` and the JSON `body`.
fn json(status: u16, body: &serde_json::Value) -> api::Response {
    honig::__private::body_response(status, "application/json", body.to_string().into_bytes())
}

// ANCHOR: media
/// The catalog as CSV from an operation that answers in JSON unless asked otherwise.
async fn export_as_csv(
    client: &api::Client<impl api::Transport>,
) -> Result<ops::export_products::Response, api::ClientError> {
    let params = ops::export_products::Params { media: ops::export_products::Media::Csv };
    client.export_products(params).await
}
// ANCHOR_END: media

#[tokio::test]
async fn the_client_asks_for_the_media_type_it_picks() {
    let client = catalog(|req| {
        assert_eq!(header(&req, "Accept"), Some("text/csv, application/json;q=0.9"));
        std::future::ready(honig::__private::body_response(200, "text/csv", b"kind,name\n".to_vec()))
    });
    let csv = ops::export_products::OkBody::Csv("kind,name\n".to_owned());
    assert_eq!(export_as_csv(&client).await.unwrap(), ops::export_products::Response::Ok(csv));
}

// ANCHOR: not-modified
/// The product or `None` when the copy tagged `etag` is still current.
async fn product_if_changed(
    client: &api::Client<impl api::Transport>,
    etag: &str,
) -> Result<Option<ops::get_product::Response>, api::ClientError> {
    let params = ops::get_product::Params { if_none_match: Some(etag.to_owned()), ..ops::get_product::Params::new(7) };
    match client.get_product(params).await {
        Err(api::ClientError::NotModified { .. }) => Ok(None),
        other => other.map(Some),
    }
}
// ANCHOR_END: not-modified

#[tokio::test]
async fn a_304_says_the_held_copy_is_current() {
    let client = catalog(|req| {
        assert_eq!(header(&req, "If-None-Match"), Some(r#""v1""#));
        let mut not_modified = honig::__private::new_response(304);
        not_modified.headers_mut().append("etag", honig::http::HeaderValue::from_static(r#""v1""#));
        std::future::ready(not_modified)
    });
    assert_eq!(product_if_changed(&client, r#""v1""#).await.unwrap(), None);
}

// ANCHOR: links
/// The `deleteProduct` call that the `DeleteProduct` link of a `getProduct` response fills: the ID of the request.
fn delete_link(request: &ops::get_product::Params, body: &serde_json::Value) -> Option<ops::delete_product::Params> {
    let link = ops::get_product::links::DeleteProduct::new(request, body);
    Some(ops::delete_product::Params::new(link.product_id?))
}
// ANCHOR_END: links

#[test]
fn the_link_carries_the_id_of_the_request() {
    let request = ops::get_product::Params::new(7);
    let body = serde_json::json!({"kind": "physical", "name": "Desk lamp"});
    assert_eq!(delete_link(&request, &body), Some(ops::delete_product::Params::new(7)));
}

// ANCHOR: callback
/// Where the `productCreated` callback goes: the URL in the `notify` parameter of the request.
fn created_callback(params: &ops::create_product::Params) -> Option<String> {
    ops::create_product::callbacks::product_created_url(params, &api::Exchange::default())
}
// ANCHOR_END: callback

#[test]
fn the_callback_goes_where_the_request_says() {
    let notify = "https://hooks.example.com/a".parse().ok();
    let params = ops::create_product::Params { notify, ..ops::create_product::Params::new(lamp()) };
    assert_eq!(created_callback(&params).as_deref(), Some("https://hooks.example.com/a"));
}

// ANCHOR: feed
/// The feed after the event that a reconnecting client received last. Each event carries its `id`. The client sends it
/// back as `Last-Event-ID`. `retry` tells clients how long to wait before they reconnect. On a quiet minute a
/// comment goes out. So proxies keep the connection open.
fn feed(
    events: &[(String, types::ProductEvent)],
    last_event_id: Option<&str>,
    retry: Duration,
) -> api::Items<types::ProductEvent> {
    let after = last_event_id.and_then(|id| events.iter().position(|(e, _)| e == id)).map_or(0, |i| i + 1);
    let events = events.iter().skip(after);
    let records = std::iter::once(honig::Sse::Retry(retry)).chain(events.map(|(id, item)| honig::Sse::Event {
        id: Some(id.clone()),
        event: None,
        item: item.clone(),
    }));
    api::Items::events(futures_util::stream::iter(records.collect::<Vec<_>>())).heartbeat(Duration::from_secs(60))
}
// ANCHOR_END: feed

/// A `200` with the feed of `events` after `last_event_id`, which asks to reconnect at once.
fn events_response(events: &[(String, types::ProductEvent)], last_event_id: Option<&str>) -> api::Response {
    let mut resp = honig::__private::new_response(200);
    let items = feed(events, last_event_id, Duration::ZERO);
    honig::__private::set_items(&mut resp, "text/event-stream", items, api::Framing::Sse).unwrap();
    resp
}

/// The event that the catalog sends when it adds `name`.
fn created(name: &str) -> types::ProductEvent {
    let product = types::Product::Physical(types::Physical::new(name.to_owned()));
    types::ProductEvent::new(types::ProductEventEvent::Created, product)
}

// ANCHOR: stream
/// The product events of one connection, decoded as they arrive.
async fn product_events(
    client: &api::Client<impl api::Transport>,
) -> Result<Vec<types::ProductEvent>, api::ClientError> {
    let params = ops::stream_product_events::Params::default();
    let api::Streamed::Ok(stream) = client.stream_product_events_stream(params).await? else {
        return Ok(vec![]); // another declared status, decoded whole
    };
    let mut stream = stream.no_reconnect();
    let mut events = vec![];
    while let Some(event) = stream.next().await {
        events.push(event?);
    }
    Ok(events)
}
// ANCHOR_END: stream

#[tokio::test]
async fn the_feed_is_read_to_its_end() {
    let client = catalog(|_| std::future::ready(events_response(&[("e1".to_owned(), created("Desk lamp"))], None)));
    assert_eq!(product_events(&client).await.unwrap(), [created("Desk lamp")]);
}

// ANCHOR: reconnect
/// Follows the feed. When the connection ends or breaks off the stream waits for the `retry` of the server. Then it
/// asks again with `Last-Event-ID`. So no event is lost or received twice. It ends when the server answers `204` or
/// after five failed reconnections in a row.
async fn follow(
    client: &api::Client<impl api::Transport>,
    mut on_event: impl FnMut(types::ProductEvent),
) -> Result<(), api::ClientError> {
    let params = ops::stream_product_events::Params::default();
    let api::Streamed::Ok(stream) = client.stream_product_events_stream(params).await? else {
        return Ok(()); // another declared status: the feed is gone
    };
    let mut stream = stream.max_retries(5);
    while let Some(event) = stream.next().await {
        on_event(event?);
    }
    Ok(())
}
// ANCHOR_END: reconnect

#[tokio::test]
async fn the_feed_resumes_after_the_last_event_read() {
    let events = [("e1".to_owned(), created("Desk lamp")), ("e2".to_owned(), created("Office chair"))];
    let asked = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen = std::sync::Arc::clone(&asked);
    // The first connection sends one event and closes. The second sends the next. The third finds the feed ended.
    let client = catalog(move |req| {
        let last = header(&req, "Last-Event-ID").map(str::to_owned);
        seen.lock().unwrap().push(last.clone());
        std::future::ready(match last.as_deref() {
            None => events_response(&events[..1], None),
            Some("e1") => events_response(&events, Some("e1")),
            Some(_) => honig::__private::new_response(204),
        })
    });
    let mut events = vec![];
    follow(&client, |e| events.push(e)).await.unwrap();
    assert_eq!(events, [created("Desk lamp"), created("Office chair")]);
    assert_eq!(*asked.lock().unwrap(), [None, Some("e1".to_owned()), Some("e2".to_owned())]);
}

/// `into_result` gives the success and any other response as an `Err` that `?` converts into any error type.
#[tokio::test]
async fn into_result_gives_the_success_and_errs_with_the_rest() -> Result<(), Box<dyn std::error::Error>> {
    let gone = catalog(|_| std::future::ready(honig::__private::new_response(204)));
    // ANCHOR: into-ok
    gone.delete_product(ops::delete_product::Params::new(7)).await?.into_result()?;
    // ANCHOR_END: into-ok
    let missing = catalog(|_| std::future::ready(json(404, &serde_json::json!({"message": "gone"}))));
    let err = missing.delete_product(ops::delete_product::Params::new(7)).await?.into_result().unwrap_err();
    assert_matches!(*err, ops::delete_product::Response::NotFound(_));
    assert_eq!(err.to_string(), "status 404 (NotFound)");
    Ok(())
}
