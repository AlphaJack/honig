//! Client recipes: the response media type, a conditional request, the OAuth device flow, event streams and callback
//! URLs. Each runs against a server in memory that answers as the shop would.

use assert_matches::assert_matches;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use e2e::v32_shop::api::{self, operations as ops, types};
use futures_util::StreamExt;

/// A client whose requests `answer` handles in memory.
fn shop<F, Fut>(answer: F) -> api::Client<impl api::Transport>
where
    F: Fn(api::Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = api::Response> + Send + 'static,
{
    api::Client::new(crate::support::loopback(answer)).with_bearer_auth("t")
}

/// The value of header `name` in `req`.
fn header<'r>(req: &'r api::Request, name: &str) -> Option<&'r str> {
    req.headers().get(name)?.to_str().ok()
}

/// One product as XML from an operation that answers in JSON unless asked otherwise.
async fn product_as_xml(
    client: &api::Client<impl api::Transport>,
) -> Result<ops::get_product::Response, api::ClientError> {
    let params = ops::get_product::Params { media: ops::get_product::Media::Xml, ..ops::get_product::Params::new(7) };
    client.get_product(params).await
}

#[tokio::test]
async fn the_client_asks_for_the_media_type_it_picks() {
    let client = shop(|req| {
        assert_eq!(header(&req, "Accept"), Some("application/xml, application/json;q=0.9"));
        std::future::ready(honig::__private::body_response(500, "text/plain", b"down".to_vec()))
    });
    assert_matches!(product_as_xml(&client).await, Err(api::ClientError::UnexpectedStatus { .. }));
}

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

#[tokio::test]
async fn a_304_says_the_held_copy_is_current() {
    let client = shop(|req| {
        assert_eq!(header(&req, "If-None-Match"), Some(r#""v1""#));
        let mut not_modified = honig::__private::new_response(304);
        not_modified.headers_mut().append("etag", honig::http::HeaderValue::try_from(r#""v1""#).unwrap());
        std::future::ready(not_modified)
    });
    assert_eq!(product_if_changed(&client, r#""v1""#).await.unwrap(), None);
}

// ANCHOR: device-flow
/// Signs in on another device. The user opens a page and types the code. Meanwhile this polls until they did.
async fn sign_in(client: &mut api::Client<impl api::Transport>) -> Result<(), api::ClientError> {
    let code = client.device_start_o_auth2("shop-cli", &[ops::OAuth2Scope::ReadProducts]).await?;
    println!("Open {} and enter {}", code.verification_uri, code.user_code);
    loop {
        match client.device_poll_o_auth2("shop-cli", &code).await? {
            api::DevicePoll::Ready(_) => return Ok(()), // the token is now the client's credential
            api::DevicePoll::Pending | api::DevicePoll::SlowDown => {
                tokio::time::sleep(Duration::from_secs(code.interval)).await;
            }
        }
    }
}
// ANCHOR_END: device-flow

#[tokio::test]
async fn the_device_flow_polls_until_the_token_is_ready() {
    let polls = AtomicU32::new(0);
    let json = |status, body: serde_json::Value| {
        honig::__private::body_response(status, "application/json", body.to_string().into_bytes())
    };
    let mut client = shop(move |req| {
        std::future::ready(if req.uri().to_string().ends_with("/oauth/device") {
            json(
                200,
                serde_json::json!({"device_code": "d", "user_code": "WDJB-MJHT", "interval": 0,
                "verification_uri": "https://shop.example.com/device", "expires_in": 600}),
            )
        } else if polls.fetch_add(1, Ordering::Relaxed) == 0 {
            json(400, serde_json::json!({"error": "authorization_pending"}))
        } else {
            json(200, serde_json::json!({"access_token": "t", "token_type": "Bearer"}))
        })
    });
    sign_in(&mut client).await.unwrap();
}

/// Every product event of the feed decoded as they arrive. The feed ends with `data: [DONE]`. That is not an event.
async fn product_events(
    client: &api::Client<impl api::Transport>,
) -> Result<Vec<types::ProductEvent>, api::ClientError> {
    let params = ops::stream_product_events::Params::default();
    let api::Streamed::Ok(mut stream) = client.stream_product_events_stream(params).await? else {
        return Ok(vec![]); // another declared status, decoded whole
    };
    let mut events = vec![];
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => events.push(event),
            Err(api::ClientError::Decode { body, .. }) if body.ends_with(b"[DONE]") => break,
            Err(error) => return Err(error),
        }
    }
    Ok(events)
}

#[tokio::test]
async fn the_feed_is_read_until_done() {
    let client = shop(|_| {
        let event = serde_json::json!({"event": "created", "product": {"id": 1, "name": "Desk lamp"}});
        let body = format!("data: {event}\n\ndata: [DONE]\n\n");
        std::future::ready(honig::__private::body_response(200, "text/event-stream", body.into_bytes()))
    });
    let events = product_events(&client).await.unwrap();
    assert_eq!(events.len(), 1);
}

/// Where the `productCreated` callback goes: the URL the request's body names.
fn created_callback(request: &api::Request) -> Option<String> {
    let exchange = api::Exchange { request: Some(request), method: "POST", ..Default::default() };
    ops::create_product::callbacks::product_created_url(&ops::create_product::Params::default(), &exchange)
}

#[test]
fn the_callback_goes_where_the_request_says() {
    let mut request = honig::__private::new_request("POST", "/products".to_owned(), vec![]).unwrap();
    request
        .headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *request.body_mut() =
        honig::Body::from(br#"{"name": "Desk lamp", "successCallbackUrl": "https://hooks.example.com/a"}"#.to_vec());
    assert_eq!(created_callback(&request).as_deref(), Some("https://hooks.example.com/a"));
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

#[tokio::test]
async fn the_feed_resumes_after_the_last_event_read() {
    let event = |name: &str| {
        let product = serde_json::from_value(serde_json::json!({"id": 1, "name": name})).unwrap();
        types::ProductEvent::new(types::ProductEventEvent::Created, product)
    };
    let events = [("e1".to_owned(), event("Desk lamp")), ("e2".to_owned(), event("Office chair"))];
    let asked = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen = std::sync::Arc::clone(&asked);
    // The first connection closes after one event. The second closes after the next. The third finds nothing new.
    let client = shop(move |req| {
        let last = header(&req, "Last-Event-ID").map(str::to_owned);
        seen.lock().unwrap().push(last.clone());
        let new = match last.as_deref() {
            None => &events[..1],
            Some("e1") => &events[..2],
            Some(_) => &[],
        };
        let mut resp = honig::__private::new_response(204);
        if !new.is_empty() {
            resp = honig::__private::new_response(200);
            let items = feed(new, last.as_deref(), Duration::ZERO);
            honig::__private::set_items(&mut resp, "text/event-stream", items, api::Framing::Sse).unwrap();
        }
        std::future::ready(resp)
    });
    let mut names = vec![];
    follow(&client, |e| names.push(e.product.name)).await.unwrap();
    assert_eq!(names, ["Desk lamp", "Office chair"]);
    assert_eq!(*asked.lock().unwrap(), [None, Some("e1".to_owned()), Some("e2".to_owned())]);
}

/// `into_result` gives the success and any other response as an `Err` that `?` converts into any error type.
#[tokio::test]
async fn into_result_gives_the_success_and_errs_with_the_rest() -> Result<(), Box<dyn std::error::Error>> {
    let gone = shop(|_| std::future::ready(honig::__private::new_response(204)));
    gone.delete_product(ops::delete_product::Params { product_id: 7 }).await?.into_result()?;
    let missing = shop(|_| {
        let mut r = honig::__private::new_response(404);
        r.headers_mut().insert("content-type", honig::http::HeaderValue::from_static("application/json"));
        *r.body_mut() = honig::Body::from(b"{}".to_vec());
        std::future::ready(r)
    });
    let err = missing.delete_product(ops::delete_product::Params { product_id: 7 }).await?.into_result().unwrap_err();
    assert_matches!(*err, ops::delete_product::Response::NotFound(_));
    assert_eq!(err.to_string(), "status 404 (NotFound)");
    Ok(())
}
