//! Observability: one wide event per request with fields that your handlers and layers add.

use std::sync::{Arc, Mutex};

use e2e::v32_catalog::api::{self, operations as ops};

use super::Catalog;

// ANCHOR: events
/// The wide event of each request as one JSON line to wherever your logs go. A layer outside the server reads it from
/// the extensions of the response. Here the lines go to a list that a test inspects.
fn logged(lines: Arc<Mutex<Vec<String>>>) -> api::Client<impl api::Transport> {
    let service = tower::ServiceBuilder::new()
        .map_response(move |resp: api::Response| {
            if let (Some(event), Ok(mut lines)) = (resp.extensions().get::<api::RequestEvent>(), lines.lock()) {
                lines.push(event.to_json());
            }
            resp
        })
        .service(api::Server::new(Catalog::default()).into_service());
    api::Client::new(service).with_api_key("secret")
}
// ANCHOR_END: events

#[tokio::test]
async fn every_request_logs_one_event() {
    let lines = Arc::new(Mutex::new(vec![]));
    let client = logged(Arc::clone(&lines));
    client.get_product(ops::get_product::Params::new(7)).await.unwrap();
    let lines = lines.lock().unwrap();
    assert_eq!(lines.len(), 1);
    let event: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(event["operation"], "get_product");
    assert_eq!(event["status"], 404);
}

// ANCHOR: trace-context
/// Spans exported through OpenTelemetry. The span of a request continues the trace that its `traceparent` names. The
/// requests of the client carry the ones of the current span.
fn exported() -> impl tracing::Subscriber {
    use opentelemetry::trace::TracerProvider as _;
    use tracing_subscriber::layer::SubscriberExt as _;
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(provider.tracer("shop")))
}
// ANCHOR_END: trace-context

fn trace_id(span: &tracing::Span) -> opentelemetry::trace::TraceId {
    use opentelemetry::trace::TraceContextExt as _;
    use tracing_opentelemetry::OpenTelemetrySpanExt as _;
    span.context().span().span_context().trace_id()
}

#[test]
fn a_request_continues_the_callers_trace() {
    use tracing::Instrument as _;
    // Process-wide and not this thread's: with a thread-local subscriber, a test on another thread that reaches the
    // runtime's request span first can leave it cached as disabled. Only this test inspects spans.
    static EXPORTED: std::sync::Once = std::sync::Once::new();
    EXPORTED.call_once(|| tracing::subscriber::set_global_default(exported()).unwrap());
    let seen = Arc::new(Mutex::new(None));
    let record = Arc::clone(&seen);
    let service = api::Server::new(Catalog::default())
        .operation_layer(tower::filter::AsyncFilterLayer::new(move |req: api::Request| {
            if let Ok(mut seen) = record.lock() {
                *seen = Some(trace_id(&tracing::Span::current()));
            }
            std::future::ready(Ok::<_, api::ApiError>(req))
        }))
        .into_service();
    let client = api::Client::new(crate::support::loopback(move |req: api::Request| {
        assert!(req.headers().get("traceparent").and_then(|v| v.to_str().ok()).is_some_and(|p| p.starts_with("00-")));
        // Under a root span of its own as a server on another host is. Only the header links the two.
        service.handle(req).instrument(tracing::info_span!(parent: None, "elsewhere"))
    }))
    .with_api_key("secret");
    let checkout = tracing::info_span!("checkout");
    let caller = trace_id(&checkout);
    api::block_on(client.get_product(ops::get_product::Params::new(7)).instrument(checkout)).unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(caller));
}

// ANCHOR: axum-fallback
/// An axum router whose undeclared paths reach the server too. They then get its 404 in the error schema and a wide
/// event. Leave the fallback out to give those paths to routes of your own.
fn with_fallback(lines: Arc<Mutex<Vec<String>>>) -> axum::Router {
    let server = || api::Server::new(Catalog::default());
    server().into_axum().fallback_service(server().into_service()).layer(tower::util::MapResponseLayer::new(
        move |resp: axum::response::Response| {
            if let (Some(event), Ok(mut lines)) = (resp.extensions().get::<api::RequestEvent>(), lines.lock()) {
                lines.push(event.to_json());
            }
            resp
        },
    ))
}
// ANCHOR_END: axum-fallback

#[test]
fn an_undeclared_path_under_axum_logs_an_event_through_the_fallback() {
    use tower::ServiceExt as _;
    let lines = Arc::new(Mutex::new(vec![]));
    let req = axum::http::Request::get("/nowhere").body(axum::body::Body::empty()).unwrap();
    let resp = api::block_on(with_fallback(Arc::clone(&lines)).oneshot(req)).unwrap();
    assert_eq!(resp.status(), 404);
    let lines = lines.lock().unwrap();
    let event: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(
        (lines.len(), &event["error"], &event["status"]),
        (1, &serde_json::json!("not_found"), &serde_json::json!(404))
    );
}

/// The event records why the request was refused. A layer's 401 is `unauthorized`.
#[tokio::test]
async fn the_event_holds_the_reason_of_a_refusal() {
    let seen = Arc::new(Mutex::new(None));
    let record = Arc::clone(&seen);
    let server = api::Server::new(Catalog::default())
        .operation_layer(tower::filter::AsyncFilterLayer::new(|_: api::Request| {
            std::future::ready(Err::<api::Request, _>(api::ApiError::new(
                api::http::StatusCode::UNAUTHORIZED,
                "expired session",
            )))
        }))
        .into_service();
    let service = tower::ServiceBuilder::new()
        .map_response(move |resp: api::Response| {
            *record.lock().unwrap() = resp.extensions().get::<api::RequestEvent>().cloned();
            resp
        })
        .service(server);
    let client = api::Client::new(service).with_api_key("secret");
    client.get_product(ops::get_product::Params::new(7)).await.unwrap_err();
    let event = seen.lock().unwrap().take().unwrap();
    let error = event.snapshot().error;
    assert!(matches!(&error, Some(api::Reason::Denied(e)) if e.message == "expired session"), "{error:?}");
    assert_eq!(event.fields()["error"], "unauthorized");
}

/// A `TraceLayer` of tower-http reads the wide event from the response: the operation, the route and why the server
/// refused the request.
#[tokio::test]
async fn a_trace_layer_reads_the_event_of_a_refusal() {
    use tower::ServiceExt as _;
    let seen = Arc::new(Mutex::new(vec![]));
    let record = Arc::clone(&seen);
    let trace = tower_http::trace::TraceLayer::new_for_http().on_response(
        move |resp: &axum::response::Response, _: std::time::Duration, _: &tracing::Span| {
            let event = resp.extensions().get::<api::RequestEvent>().map(api::RequestEvent::fields);
            record
                .lock()
                .unwrap()
                .push(event.map(|f| (f["operation"].clone(), f["route"].clone(), f["error"].clone())));
        },
    );
    let server = || api::Server::new(Catalog::default());
    let app = server().into_axum().fallback_service(server().into_service()).layer(trace);
    for path in ["/v1/products/7", "/nowhere"] {
        let req = axum::http::Request::get(path).body(axum::body::Body::empty()).unwrap();
        app.clone().oneshot(req).await.unwrap();
    }
    let json = serde_json::Value::from;
    assert_eq!(
        *seen.lock().unwrap(),
        [
            Some((json("get_product"), json("/products/{productId}"), json("unauthorized"))),
            Some((json(""), json(""), json("not_found"))),
        ]
    );
}
