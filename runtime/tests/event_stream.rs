//! An event stream that reconnects as the WHATWG `EventSource` does. Also the records and heartbeat of a server.

#![expect(clippy::unwrap_used, reason = "test code fails loudly")]

use assert_matches::assert_matches;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use honig::__private::{Framing, set_items};
use honig::{ByteStream, Bytes, Error, ItemStream, Items, Request, Response, Sse};

/// One scripted answer. It is a status, a `Content-Type` and body pieces (`None` breaks the connection). Or it is a
/// request that fails.
enum Answer {
    Stream(u16, &'static str, Vec<Option<&'static str>>),
    Fail,
}

/// Answers each request with the next scripted answer and keeps the requests.
#[derive(Default)]
struct Script {
    answers: Mutex<VecDeque<Answer>>,
    sent: Mutex<Vec<Request>>,
}

impl Script {
    fn new(answers: impl IntoIterator<Item = Answer>) -> Arc<Self> {
        Arc::new(Self { answers: Mutex::new(answers.into_iter().collect()), sent: Mutex::default() })
    }

    /// The `Last-Event-ID` of each request sent.
    fn last_event_ids(&self) -> Vec<Option<String>> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.headers().get("Last-Event-ID").and_then(|v| v.to_str().ok()).map(str::to_owned))
            .collect()
    }
}

/// The first answer as a generated `<op>_stream` reads it.
fn open(script: &Arc<Script>) -> ItemStream<String> {
    let req = honig::__private::new_request("GET", "/events".to_owned(), vec![]).unwrap();
    let again = honig::__private::resendable(&req);
    let transport = Scripted(Arc::clone(script));
    let resp = honig::__private::block_on(honig::send_streaming(&transport, req)).unwrap();
    let status = resp.status().as_u16();
    ItemStream::new(status, resp.into_body().into_stream(), Framing::Sse).reconnecting(transport, again)
}

/// Body pieces in order. `None` breaks the connection.
fn pieces(pieces: Vec<Option<&'static str>>) -> ByteStream {
    let pieces = pieces
        .into_iter()
        .map(|p| p.map(|t| Bytes::from_static(t.as_bytes())).ok_or_else(|| Error::Transport("reset".into())));
    ByteStream::chunks(futures_util::stream::iter(pieces), None)
}

/// The script as a tower service: clones share it.
#[derive(Clone)]
struct Scripted(Arc<Script>);

impl tower::Service<Request> for Scripted {
    type Response = Response;
    type Error = Error;
    type Future = std::future::Ready<Result<Response, Error>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request) -> Self::Future {
        self.0.sent.lock().unwrap().push(req);
        let answer = self.0.answers.lock().unwrap().pop_front();
        std::future::ready(match answer.unwrap_or(Answer::Stream(204, "", vec![])) {
            Answer::Fail => Err(Error::Transport("refused".into())),
            Answer::Stream(status, ct, body) => {
                Ok(honig::__private::body_response(status, ct, honig::Body::stream(pieces(body))))
            }
        })
    }
}

/// Every item and error until the stream ends.
fn drain(stream: &mut ItemStream<String>) -> Vec<Result<String, Error>> {
    std::iter::from_fn(|| honig::__private::block_on(stream.next())).collect()
}

const SSE: &str = "text/event-stream";

#[test]
fn a_broken_stream_resumes_after_the_last_event_and_drops_the_one_it_broke_inside() {
    let script = Script::new([
        Answer::Stream(200, SSE, vec![Some("retry: 0\nid: 1\ndata: \"a\"\n\n"), Some("id: 9\ndata: \"b"), None]),
        Answer::Stream(200, SSE, vec![Some("id: 2\ndata: \"c\"\n\n: bye\n\n")]),
    ]);
    let mut stream = open(&script);
    let got: Vec<String> = drain(&mut stream).into_iter().map(Result::unwrap).collect();
    assert_eq!(got, ["a", "c"]);
    assert_eq!(script.last_event_ids(), [None, Some("1".into()), Some("2".into())], "204 ends it");
    assert_eq!(stream.last_event_id(), "2");
    let again = &script.sent.lock().unwrap()[1];
    assert_eq!(
        (
            again.headers().get("Accept").and_then(|v| v.to_str().ok()),
            again.headers().get("Cache-Control").and_then(|v| v.to_str().ok())
        ),
        (Some(SSE), Some("no-cache"))
    );
}

#[test]
fn an_id_or_retry_without_data_still_counts() {
    let script = Script::new([Answer::Stream(200, SSE, vec![Some("retry: 0\n\nid: 5\n\n")])]);
    let mut stream = open(&script);
    assert!(drain(&mut stream).is_empty());
    assert_eq!(stream.retry(), Duration::ZERO);
    assert_eq!(script.last_event_ids(), [None, Some("5".into())]);
}

#[test]
fn failed_reconnections_are_errors_until_the_budget_runs_out() {
    let script = Script::new([
        Answer::Stream(200, SSE, vec![Some("retry: 0\n\n")]),
        Answer::Fail,
        Answer::Stream(503, "text/plain", vec![Some("busy")]),
        Answer::Stream(200, SSE, vec![]),
    ]);
    let mut stream = open(&script).max_retries(2);
    let got = drain(&mut stream);
    assert_matches!(
        got[..],
        [Err(Error::Transport(_)), Err(Error::UnexpectedStatus { status: http::StatusCode::SERVICE_UNAVAILABLE, .. })],
        "{got:?}"
    );
    assert_eq!(script.sent.lock().unwrap().len(), 3, "no third attempt");
}

#[test]
fn another_status_or_media_type_ends_the_stream() {
    for (answer, media) in
        [(Answer::Stream(404, "text/plain", vec![]), false), (Answer::Stream(200, "text/plain", vec![]), true)]
    {
        let script = Script::new([Answer::Stream(200, SSE, vec![Some("retry: 0\n\n")]), answer]);
        let got = drain(&mut open(&script));
        match &got[..] {
            [Err(Error::UnexpectedMedia { .. })] => assert!(media),
            [Err(Error::UnexpectedStatus { status: http::StatusCode::NOT_FOUND, .. })] => assert!(!media),
            other => panic!("{other:?}"),
        }
        assert_eq!(script.sent.lock().unwrap().len(), 2);
    }
}

#[test]
fn a_stream_without_its_request_ends_with_its_body() {
    let script = Script::new([Answer::Stream(200, SSE, vec![Some("data: \"a\"\n\n"), None])]);
    let mut stream = open(&script).no_reconnect();
    let got = drain(&mut stream);
    assert_matches!(&got[..], [Ok(a), Err(Error::Transport(_))] if a == "a", "{got:?}");
}

/// The bytes a server sends for `items`.
fn served(items: Items<String>) -> String {
    let mut resp = honig::__private::new_response(200);
    set_items(&mut resp, SSE, items, Framing::Sse).unwrap();
    let body = honig::__private::block_on(resp.into_body().collect()).unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}

#[test]
fn a_server_writes_ids_names_retry_and_comments() {
    let records = vec![
        Sse::Retry(Duration::from_millis(1500)),
        Sse::Event { id: Some("7".into()), event: Some("price".into()), item: "a".to_owned() },
        Sse::Comment("one\rtwo".into()),
    ];
    let body = served(Items::events(futures_util::stream::iter(records)));
    assert_eq!(body, "retry: 1500\n\nid: 7\nevent: price\ndata: \"a\"\n\n: one\n: two\n\n");
    let bad = Sse::Event { id: Some("a\nb".into()), event: None, item: String::new() };
    let mut resp = honig::__private::new_response(200);
    set_items(&mut resp, SSE, Items::events(futures_util::stream::iter(vec![bad])), Framing::Sse).unwrap();
    assert_matches!(honig::__private::block_on(resp.into_body().collect()), Err(Error::Encode(_)));
}

/// Items written into one piece: an item that fails halfway sends none of its bytes. The items before it go out.
#[test]
fn an_item_that_fails_to_encode_sends_nothing_of_itself() {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Price {
        low: f64,
        high: f64,
    }
    let items = vec![Price { low: 1.0, high: 2.0 }, Price { low: 1.0, high: f64::NAN }];
    let mut resp = honig::__private::new_response(200);
    set_items(&mut resp, SSE, Items::stream(futures_util::stream::iter(items)), Framing::Sse).unwrap();
    let mut chunks = resp.into_body().into_stream();
    let first = honig::__private::block_on(chunks.next()).unwrap().unwrap();
    assert_eq!(&first[..], b"data: {\"low\":1.0,\"high\":2.0}\n\n");
    assert_matches!(honig::__private::block_on(chunks.next()), Some(Err(Error::Encode(_))));
}

#[test]
fn a_quiet_stream_sends_a_heartbeat() {
    let quiet = Items::events(futures_util::stream::pending::<Sse<String>>());
    let mut resp = honig::__private::new_response(200);
    set_items(&mut resp, SSE, quiet.heartbeat(Duration::from_millis(10)), Framing::Sse).unwrap();
    let first = honig::__private::block_on(resp.into_body().into_stream().next()).unwrap().unwrap();
    assert_eq!(&first[..], b": \n\n");
}
