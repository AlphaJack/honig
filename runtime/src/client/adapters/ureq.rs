//! Blocking `ureq` transport. It is included when the crate depends on `ureq`.

use std::io::Read;

use ureq::{RequestExt, http};

use crate::{Body, BodySource, ByteStream, Error, Request, Response};

/// The most that a read of a streamed response takes at once. A read returns what has arrived up to this.
const CHUNK: usize = 16 << 10;

/// Blocking `ureq` transport: `Client::new(api::UreqTransport::new("https://api.example.com"))`,
/// then `api::block_on(client.some_operation(params))`. Each call blocks its thread.
#[derive(Debug, Clone)]
pub struct UreqTransport {
    /// The underlying agent. Configure timeouts, proxies, TLS and middleware on it. Error statuses come back as
    /// responses whatever it says about them.
    pub agent: ureq::Agent,
    /// Base URL without a trailing slash.
    pub base_url: String,
}

impl UreqTransport {
    /// Creates a transport with a default agent for this base URL.
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_agent(ureq::Agent::new_with_defaults(), base_url)
    }

    /// Creates a transport with your own agent for this base URL.
    pub fn with_agent(agent: ureq::Agent, base_url: impl Into<String>) -> Self {
        Self { agent, base_url: crate::base_url(base_url) }
    }

    /// Runs `req` on the agent. Error statuses come back as responses. So documented error bodies decode.
    fn run(&self, req: http::Request<impl ureq::AsSendBody>) -> Result<http::Response<ureq::Body>, ureq::Error> {
        req.with_agent(&self.agent).configure().http_status_as_error(false).run()
    }

    /// Sends `req` and returns once the head of the response has arrived. Its body is still to read.
    fn start(&self, req: Request) -> Result<http::Response<ureq::Body>, Error> {
        let target = http::Uri::try_from(crate::target(&self.base_url, req.uri())).map_err(Error::transport)?;
        let (mut head, body) = req.into_parts();
        head.uri = target;
        let (bytes, stream) = body.into_parts();
        // A streamed body goes out as it is produced. It is chunked or has its length when its source knows it.
        match stream.take_source() {
            None => self.run(http::Request::from_parts(head, &bytes[..])),
            Some(source) => {
                if let Some(len) = source.size() {
                    head.headers
                        .insert(http::header::CONTENT_LENGTH, http::HeaderValue::from(bytes.len() as u64 + len));
                }
                let mut pieces = Pieces { buf: bytes, pos: 0, rest: Some(source) };
                self.run(http::Request::from_parts(head, ureq::SendBody::from_reader(&mut pieces)))
            }
        }
        .map_err(Error::transport)
    }
}

impl tower_service::Service<Request> for UreqTransport {
    type Response = Response;
    type Error = Error;
    type Future = std::future::Ready<Result<Response, Error>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    /// Blocks until the head has arrived. Each chunk of the body is what has arrived by the time that it is asked
    /// for. So an event stream yields each event as the server sends it.
    fn call(&mut self, req: Request) -> Self::Future {
        std::future::ready(self.start(req).map(|resp| {
            resp.map(|body| {
                let reader = body.into_with_config().limit(u64::MAX).reader();
                Body::stream(ByteStream::chunks(futures_util::stream::iter(Incoming(reader)), None))
            })
        }))
    }
}

/// A response body read as it arrives. Each read blocks this thread until some bytes come. Every `ureq` call does this.
struct Incoming(ureq::BodyReader<'static>);

impl Iterator for Incoming {
    type Item = Result<bytes::Bytes, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut buf = vec![0; CHUNK];
        let read = loop {
            match self.0.read(&mut buf) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                other => break other,
            }
        };
        match read {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some(Ok(bytes::Bytes::from(buf)))
            }
            Err(e) => Some(Err(Error::transport(e))),
        }
    }
}

/// A streamed upload as `Read`. First the bytes held whole and then each piece. This thread waits for each piece.
struct Pieces {
    buf: bytes::Bytes,
    pos: usize,
    rest: Option<std::pin::Pin<Box<dyn BodySource>>>,
}

impl Read for Pieces {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos == self.buf.len() {
            let Some(source) = self.rest.as_mut() else { return Ok(0) };
            match crate::block_on(crate::source::next_piece(source)) {
                Some(Ok(piece)) => (self.buf, self.pos) = (piece, 0),
                None => {
                    self.rest = None;
                    return Ok(0);
                }
                Some(Err(e)) => {
                    self.rest = None;
                    return Err(std::io::Error::other(e.to_string()));
                }
            }
        }
        let left = self.buf.get(self.pos..).unwrap_or_default();
        let n = left.len().min(out.len());
        out.get_mut(..n).unwrap_or_default().copy_from_slice(left.get(..n).unwrap_or_default());
        self.pos += n;
        Ok(n)
    }
}
