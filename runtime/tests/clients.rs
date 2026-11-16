//! The client transports over a real socket: each sends to an echo server on localhost and reads back what it sent.
//!
//! Bodies run from empty to 3 MiB. So a response arrives in many reads. Each transport sends a whole body. It reads a
//! response whole and as it arrives. It streams an upload chunked.

#![cfg(any(feature = "reqwest", feature = "ureq"))]
#![expect(clippy::unwrap_used, reason = "test code fails loudly")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use futures_util::StreamExt;
use honig::__private::set_blob;
use honig::{Blob, Request, Transport};

const SIZES: [usize; 4] = [0, 1, 256, 3 << 20];

/// Starts the echo server and returns its base URL.
fn echo_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let conn = conn.unwrap();
            std::thread::spawn(move || echo(conn));
        }
    });
    format!("http://{addr}")
}

/// Answers one request with its body back. Its method, target and `X-Test` header go back as response headers.
fn echo(conn: TcpStream) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let start = line(&mut reader);
    let mut words = start.split(' ');
    let (method, target) = (words.next().unwrap().to_owned(), words.next().unwrap().to_owned());
    let (mut length, mut chunked, mut test) = (0, false, String::new());
    loop {
        let header = line(&mut reader);
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').unwrap();
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse().unwrap(),
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            "x-test" => value.clone_into(&mut test),
            _ => {}
        }
    }
    let body = if chunked { read_chunked(&mut reader) } else { read_exact(&mut reader, length) };
    let mut out = conn;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nX-Method: {method}\r\nX-Target: {target}\r\n\
         X-Test: {test}\r\nX-Framing: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        if chunked { "chunked" } else { "length" },
        body.len()
    );
    out.write_all(head.as_bytes()).unwrap();
    // Written in pieces. So a client that reads as it arrives gets more than one chunk.
    for piece in body.chunks(64 << 10) {
        out.write_all(piece).unwrap();
        out.flush().unwrap();
    }
}

fn line(reader: &mut impl BufRead) -> String {
    let mut s = String::new();
    reader.read_line(&mut s).unwrap();
    s.trim_end_matches(['\r', '\n']).to_owned()
}

fn read_exact(reader: &mut impl Read, n: usize) -> Vec<u8> {
    let mut buf = vec![0; n];
    reader.read_exact(&mut buf).unwrap();
    buf
}

fn read_chunked(reader: &mut impl BufRead) -> Vec<u8> {
    let mut body = vec![];
    loop {
        let size = line(reader);
        let size = usize::from_str_radix(size.split(';').next().unwrap(), 16).unwrap();
        if size == 0 {
            // Trailers and then the blank line.
            while !line(reader).is_empty() {}
            return body;
        }
        body.extend(read_exact(reader, size));
        assert!(line(reader).is_empty(), "a chunk ends with CRLF");
    }
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| u8::try_from(i % 251).unwrap()).collect::<Vec<_>>()
}

fn request(size: usize) -> Request {
    let query = vec!["q=1".to_owned(), "r=two".to_owned()];
    let mut req = honig::__private::new_request("POST", "/echo/a%20b".to_owned(), query).unwrap();
    req.headers_mut().append("x-test", http::HeaderValue::try_from(format!("size {size}")).unwrap());
    req.headers_mut().insert(http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/octet-stream"));
    *req.body_mut() = honig::Body::from(pattern(size));
    req
}

/// Sends every size whole. Reads it back whole and as it arrives. Streams it up in pieces.
async fn roundtrips(transport: &impl Transport) {
    for size in SIZES {
        let want = pattern(size);

        let resp = honig::send(transport, request(size)).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.headers().get("X-Method").and_then(|v| v.to_str().ok()), Some("POST"));
        assert_eq!(resp.headers().get("X-Target").and_then(|v| v.to_str().ok()), Some("/echo/a%20b?q=1&r=two"));
        assert_eq!(resp.headers().get("X-Test").and_then(|v| v.to_str().ok()), Some(format!("size {size}").as_str()));
        assert_eq!(resp.body().bytes().len(), size, "send, {size} bytes");
        assert!(resp.body().bytes() == want.as_slice(), "send, {size} bytes: the body differs");

        let streaming = honig::send_streaming(transport, request(size)).await.unwrap();
        assert_eq!(streaming.status(), 200);
        let got = streaming.into_body().collect().await.unwrap();
        assert!(got == want, "send_streaming, {size} bytes: the body differs");

        let mut up = request(size);
        let pieces: Vec<Vec<u8>> = want.chunks(100_000).map(<[u8]>::to_vec).collect();
        up.headers_mut().remove("content-type");
        set_blob(&mut up, "application/octet-stream", Blob::stream(futures_util::stream::iter(pieces)));
        let resp = honig::send(transport, up).await.unwrap();
        assert!(resp.body().bytes() == want.as_slice(), "streamed upload, {size} bytes: the body differs");

        // The same pieces with their total. They are sent with `Content-Length` for servers that read no chunked body.
        let mut up = request(size);
        let pieces: Vec<Vec<u8>> = want.chunks(100_000).map(<[u8]>::to_vec).collect();
        up.headers_mut().remove("content-type");
        set_blob(&mut up, "application/octet-stream", Blob::sized(size as u64, futures_util::stream::iter(pieces)));
        let resp = honig::send(transport, up).await.unwrap();
        assert!(resp.body().bytes() == want.as_slice(), "sized streamed upload, {size} bytes: the body differs");
        assert_eq!(
            resp.headers().get("X-Framing").and_then(|v| v.to_str().ok()),
            Some("length"),
            "sized streamed upload, {size} bytes"
        );

        // A buffer that another `Blob` still holds goes out as it is with its length and not in chunks.
        let shared = Blob::from(honig::Bytes::from(want.clone()));
        let kept = shared.clone();
        let mut up = request(size);
        up.headers_mut().remove("content-type");
        set_blob(&mut up, "application/octet-stream", shared);
        let resp = honig::send(transport, up).await.unwrap();
        assert!(resp.body().bytes() == want.as_slice(), "shared upload, {size} bytes: the body differs");
        assert_eq!(
            resp.headers().get("X-Framing").and_then(|v| v.to_str().ok()),
            Some("length"),
            "shared upload, {size} bytes"
        );
        drop(kept);
    }
}

/// The piece that a held server sends first and the one that it sends once the client has read it.
const FIRST: &[u8] = b"data: first\n\n";
const SECOND: &[u8] = b"data: second\n\n";

/// How long a held server waits for the client to read the first piece. Then it sends the rest anyway. A client
/// that buffers the body gets both pieces as one after this.
const HOLD: std::time::Duration = std::time::Duration::from_secs(5);

/// Starts a server that answers one event stream. It sends its head and [`FIRST`]. Then it sends [`SECOND`] once `read`
/// says that the client has the first piece. Returns its base URL.
fn held_server(read: std::sync::mpsc::Receiver<()>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (conn, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(conn.try_clone().unwrap());
        while !line(&mut reader).is_empty() {}
        let mut out = conn;
        out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").unwrap();
        out.write_all(FIRST).unwrap();
        out.flush().unwrap();
        // A timeout means that the client buffers. The rest goes anyway. So its test fails and does not hang.
        read.recv_timeout(HOLD).ok();
        out.write_all(SECOND).unwrap();
    });
    format!("http://{addr}")
}

/// A transport gives the first piece while the server still holds the rest. An event stream needs this.
async fn streams_as_it_arrives(transport: &impl Transport, read: std::sync::mpsc::Sender<()>) {
    let events = honig::__private::new_request("GET", "/events".to_owned(), vec![]).unwrap();
    let resp = honig::send_streaming(transport, events).await.unwrap();
    assert_eq!(resp.status(), 200);
    let mut streaming = resp.into_body().into_stream();
    let first = streaming.next().await.unwrap().unwrap();
    assert_eq!(&first[..], FIRST, "the first piece alone, before the server sends the second");
    read.send(()).unwrap();
    let rest = streaming.collect().await.unwrap();
    assert_eq!(rest, SECOND);
}

fn run<F: Future>(fut: F) -> F::Output {
    tokio::runtime::Runtime::new().unwrap().block_on(fut)
}

#[cfg(feature = "reqwest")]
#[test]
fn reqwest_sends_and_reads_back_every_body() {
    let transport = honig::ReqwestTransport::new(echo_server());
    run(roundtrips(&transport));
}

#[cfg(feature = "reqwest-middleware")]
#[test]
fn reqwest_middleware_sends_and_reads_back_every_body() {
    let stack = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build();
    let transport = honig::ReqwestMiddlewareTransport::with_client(stack, echo_server());
    run(roundtrips(&transport));
}

#[cfg(feature = "ureq")]
#[test]
fn ureq_sends_and_reads_back_every_body() {
    let transport = honig::UreqTransport::new(echo_server());
    run(roundtrips(&transport));
}

#[cfg(feature = "reqwest")]
#[test]
fn reqwest_streams_a_response_as_it_arrives() {
    let (read, held) = std::sync::mpsc::channel();
    let transport = honig::ReqwestTransport::new(held_server(held));
    run(streams_as_it_arrives(&transport, read));
}

#[cfg(feature = "ureq")]
#[test]
fn ureq_streams_a_response_as_it_arrives() {
    let (read, held) = std::sync::mpsc::channel();
    let transport = honig::UreqTransport::new(held_server(held));
    run(streams_as_it_arrives(&transport, read));
}
