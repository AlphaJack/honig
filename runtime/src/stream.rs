//! Bodies that arrive or leave a piece at a time.

use std::borrow::Cow;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use futures_core::Stream;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::checks::InstancePath;
use crate::codec::{sse_boundary, sse_comment_only, sse_item, sse_state};
use crate::message::Message;
use crate::source::{Blob, BodySource, ByteStream, Feed, Slot, Sse, put_slot, take_slot};
#[cfg(feature = "multipart")]
use crate::text::decode_err;
use crate::transport::Pending;
use crate::{Body, ClientError, Request, codec::json_seq_encode, codec::jsonl_encode, codec::sse_encode};

/// The byte order mark that an SSE stream may start with. It is not part of the first event.
const BOM: &str = "\u{feff}";

/// What a generated `<op>_stream` method returns.
///
/// A success status gives the body as it arrives. Any other status that the description declares gives the
/// `Response` of the operation. It is decoded whole like the method without `_stream`. A status that the description
/// does not declare is `ClientError::UnexpectedStatus` as there.
#[derive(Debug)]
pub enum Streamed<S, R> {
    /// A success status: the body's chunks or items as they arrive.
    Ok(S),
    /// Another declared status such as an error or a redirect. It is decoded whole.
    Whole(R),
}

/// A sequential body (JSON Lines, `json-seq` or SSE): every item in memory or items that arrive or leave one at a
/// time.
///
/// Clients receive `List`. `<op>_stream` yields items as they arrive. Server handlers receive a sequential request
/// body as `Stream`: an upload of events is never buffered whole. `Items` is a `Stream` of its items. Consume them with
/// `futures_util::StreamExt::next`.
#[derive(Debug, Clone)]
pub enum Items<T> {
    /// Every item in memory.
    List(Vec<T>),
    /// Items still to come: a request body the server has not consumed or items that your code produces.
    Stream(ItemStream<T>),
    /// Items and comments that your code produces while the response is sent. A `text/event-stream` writes a comment
    /// as `: text` and clients skip it. This is a keep-alive on an idle stream. Other framings drop it.
    Events(ItemStream<Sse<T>>),
}

impl<T> Items<T> {
    /// Returns the items that `items` produces while the body is sent.
    pub fn stream<S: Stream<Item = T> + Send + 'static>(items: S) -> Self {
        Self::Stream(ItemStream::produced(Box::pin(items)))
    }

    /// Returns the records that `records` produces while the response is sent: items, events with an `id`, comments and
    /// `retry` fields.
    pub fn events<S: Stream<Item = Sse<T>> + Send + 'static>(records: S) -> Self {
        Self::Events(ItemStream::produced(Box::pin(records)))
    }

    /// Returns the items if they are in memory.
    pub fn into_list(self) -> Option<Vec<T>> {
        match self {
            Self::List(v) => Some(v),
            Self::Stream(_) | Self::Events(_) => None,
        }
    }
}

impl<T: Send + 'static> Items<T> {
    /// Sends an empty comment whenever no record came for `every`. Proxies and load balancers that close an idle
    /// connection then keep it open. Clients skip the comment. Only `Events` has comments.
    #[must_use]
    pub fn heartbeat(self, every: std::time::Duration) -> Self {
        match self {
            Self::Events(records) => match records.take_feed() {
                Some(feed) => {
                    let beats = crate::transport::heartbeat(feed, every, || Sse::Comment(String::new()));
                    Self::Events(ItemStream::produced(Box::pin(beats)))
                }
                None => Self::Events(records),
            },
            other @ (Self::List(_) | Self::Stream(_)) => other,
        }
    }
}

/// Each item of the list, each item that arrives or each item produced. An item that arrives may fail to decode or
/// break a constraint and the next may still arrive. A body that broke off or went past the `max_body_bytes` of the
/// server ends after its error. `?` in a handler turns either into an [`ApiError`](crate::ApiError) with its status.
impl<T: DeserializeOwned + Send + 'static> Stream for Items<T> {
    type Item = Result<T, ClientError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Self::List(v) = this {
            let items = futures_util::stream::iter(std::mem::take(v));
            *this = Self::Stream(ItemStream::produced(Box::pin(items)));
        }
        match this {
            Self::List(_) => Poll::Ready(None),
            Self::Stream(items) => Pin::new(items).poll_next(cx),
            Self::Events(records) => loop {
                match ready!(records.poll_with(cx, |_, _| Poll::Ready(None))) {
                    Some(Ok(Sse::Item(item) | Sse::Event { item, .. })) => return Poll::Ready(Some(Ok(item))),
                    Some(Ok(Sse::Retry(_) | Sse::Comment(_))) => {}
                    Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                    None => return Poll::Ready(None),
                }
            },
        }
    }
}

impl<T: Serialize + DeserializeOwned + Send + 'static> Items<T> {
    /// Returns every item in one list.
    ///
    /// # Errors
    /// The first item that fails as in the `Stream` of `Items`.
    pub async fn collect(mut self) -> Result<Vec<T>, ClientError> {
        if let Self::List(v) = self {
            return Ok(v);
        }
        let mut out = vec![];
        while let Some(item) = futures_util::StreamExt::next(&mut self).await {
            out.push(item?);
        }
        Ok(out)
    }

    /// Returns the body as a request or a response carries it. It is encoded at once or as items framed as they are
    /// produced. An arriving body is passed on as it arrives. That is the bytes not consumed yet and unchecked.
    ///
    /// # Errors
    /// An item of a list does not serialize. Or `framing` is `Multipart`, which is only decoded.
    pub fn into_body(self, framing: Framing) -> Result<Body, ClientError> {
        let items = match self {
            Self::List(v) => {
                let body = match framing {
                    Framing::JsonLines => jsonl_encode(&v)?,
                    Framing::JsonSeq => json_seq_encode(&v)?,
                    Framing::Sse => sse_encode(&v)?,
                    #[cfg(feature = "multipart")]
                    Framing::Multipart => return Err(not_encoded()),
                };
                return Ok(Body::new(body));
            }
            Self::Events(records) => {
                let stream = records.take_feed().map(|s| ByteStream::new(Framed::new(s, framing, frame_record::<T>)));
                return Ok(Body::stream(stream.unwrap_or_default()));
            }
            Self::Stream(items) => items,
        };
        let stream = match items.take_inner() {
            Some(Inner::Produced(feed)) => ByteStream::new(Framed::new(feed, framing, frame_one::<T>)),
            Some(Inner::Decoding(decoder)) => {
                let (read, rest) = decoder.into_unread();
                ByteStream::new(crate::source::Leftover(Some(bytes::Bytes::from(read)), rest.take_source()))
            }
            None => ByteStream::default(),
        };
        Ok(Body::stream(stream))
    }
}

impl<T: DeserializeOwned> Items<T> {
    /// Returns a sequential request body as the server received it: arriving when the adapter left it unread in
    /// `stream`, else decoded from `body`.
    ///
    /// # Errors
    /// `body` does not decode.
    pub fn received(body: &[u8], stream: &ByteStream, framing: Framing) -> Result<Self, ClientError> {
        if let Some(source) = stream.take_source() {
            return Ok(Self::Stream(ItemStream::new(REQUEST, ByteStream::pinned(source), framing)));
        }
        match framing {
            Framing::JsonLines => crate::codec::jsonl_decode(REQUEST, body),
            Framing::JsonSeq => crate::codec::json_seq_decode(REQUEST, body),
            Framing::Sse => crate::codec::sse_decode(REQUEST, body),
            #[cfg(feature = "multipart")]
            Framing::Multipart => Err(not_encoded()),
        }
        .map(Self::List)
    }

    /// Checks the sequence against `limits`: a list at once, an arriving one as its items arrive.
    ///
    /// # Errors
    /// A list that does not meet them.
    #[doc(hidden)]
    pub fn limited(self, limits: Limits) -> Result<Self, ClientError>
    where
        T: Serialize,
    {
        Ok(match self {
            Self::List(items) => Self::List(limit_items(REQUEST, items, limits)?),
            Self::Stream(items) => Self::Stream(items.limited(limits)),
            // Items that your code produces are not checked.
            events @ Self::Events(_) => events,
        })
    }

    /// Returns [`Items::received`] with the text of the library formats of each item checked first, as `H` says
    /// (`runtime::formats`).
    ///
    /// # Errors
    /// `body` does not decode.
    pub fn received_validated<H: for<'de> crate::formats::Holds<'de, T>>(
        body: &[u8],
        stream: &ByteStream,
        framing: Framing,
    ) -> Result<Self, ClientError> {
        type Validated<T, H> = crate::formats::Validated<T, H>;
        if let Some(source) = stream.take_source() {
            let items = ItemStream::new(REQUEST, ByteStream::pinned(source), framing).validated::<H>();
            return Ok(Self::Stream(items));
        }
        let items: Vec<Validated<T, H>> = match framing {
            Framing::JsonLines => crate::codec::jsonl_decode(REQUEST, body),
            Framing::JsonSeq => crate::codec::json_seq_decode(REQUEST, body),
            Framing::Sse => crate::codec::sse_decode(REQUEST, body),
            #[cfg(feature = "multipart")]
            Framing::Multipart => Err(not_encoded()),
        }?;
        Ok(Self::List(items.into_iter().map(Validated::into_inner).collect()))
    }
}

/// The status a request body decodes with: there is no response yet.
const REQUEST: Option<u16> = None;

/// No item is pinned: each is moved out of a list, a decoder or a boxed stream.
impl<T> Unpin for Items<T> {}

impl<T> From<Vec<T>> for Items<T> {
    fn from(v: Vec<T>) -> Self {
        Self::List(v)
    }
}

impl<T: PartialEq> PartialEq for Items<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Stream(a), Self::Stream(b)) => a.same(b),
            (Self::Events(a), Self::Events(b)) => a.same(b),
            (Self::List(_) | Self::Stream(_) | Self::Events(_), _) => false,
        }
    }
}

impl<T: Eq> Eq for Items<T> {}

/// Returns the error of a multipart body to encode: `Framing::Multipart` only decodes. A server returns a multipart
/// body whole from a list.
#[cfg(feature = "multipart")]
fn not_encoded() -> ClientError {
    ClientError::Encode("a multipart body is written whole, not item by item".to_owned())
}

/// Returns where `needle` first starts in `hay`.
#[cfg(feature = "multipart")]
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(hay, needle)
}

/// Appends one item in a framing to `out`.
fn frame_one<T: Serialize>(item: &T, framing: Framing, out: &mut Vec<u8>) -> Result<(), ClientError> {
    match framing {
        Framing::JsonLines => crate::codec::jsonl_write(out, item),
        Framing::JsonSeq => crate::codec::json_seq_write(out, item),
        Framing::Sse => crate::codec::sse_write(out, item),
        #[cfg(feature = "multipart")]
        Framing::Multipart => Err(not_encoded()),
    }
}

/// Appends one [`Sse`] record in a framing to `out`. An `id`, an event name, a `retry` or a comment exists only in
/// `text/event-stream`.
fn frame_record<T: Serialize>(record: &Sse<T>, framing: Framing, out: &mut Vec<u8>) -> Result<(), ClientError> {
    match record {
        Sse::Event { id, event, item } if framing == Framing::Sse => {
            for (key, value) in [("id", id), ("event", event)] {
                if let Some(value) = value {
                    crate::codec::push_field(out, key, value)?;
                }
            }
            frame_one(item, framing, out)
        }
        Sse::Item(item) | Sse::Event { item, .. } => frame_one(item, framing, out),
        Sse::Retry(wait) if framing == Framing::Sse => {
            crate::codec::push_text(out, &["retry: ", &wait.as_millis().to_string(), "\n\n"]);
            Ok(())
        }
        Sse::Comment(text) if framing == Framing::Sse => {
            for line in crate::codec::text_lines(text) {
                crate::codec::push_text(out, &[": ", line, "\n"]);
            }
            out.push(b'\n');
            Ok(())
        }
        Sse::Comment(_) | Sse::Retry(_) => Ok(()),
    }
}

/// The bytes of encoded items that one piece of a streamed body may gather. Items already produced go out together.
/// They do not get one piece (and one HTTP chunk) each.
const BATCH: usize = 16 << 10;

/// Encodes each item that your code produces with `frame` as the body is sent. A piece contains the items that are
/// ready when it is cut up to [`BATCH`] bytes. An iterator fills whole pieces. A feed that waits for its next item
/// sends each item at once.
struct Framed<T> {
    items: Feed<T>,
    framing: Framing,
    frame: Frame<T>,
    /// An item that failed to encode after others in the same piece: sent as the next piece.
    failed: Option<ClientError>,
    /// The items ended after the last piece was cut.
    ended: bool,
}

/// Appends one item in a framing to the piece that is being gathered.
type Frame<T> = fn(&T, Framing, &mut Vec<u8>) -> Result<(), ClientError>;

impl<T> Framed<T> {
    fn new(items: Feed<T>, framing: Framing, frame: Frame<T>) -> Self {
        Self { items, framing, frame, failed: None, ended: false }
    }
}

impl<T: 'static> BodySource for Framed<T> {
    fn poll_piece(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<bytes::Bytes, ClientError>>> {
        let this = self.get_mut();
        if let Some(e) = this.failed.take() {
            return Poll::Ready(Some(Err(e)));
        }
        // Every item of the piece is written into it: no buffer per item.
        let mut piece = vec![];
        while !this.ended && piece.len() < BATCH {
            match this.items.as_mut().poll_next(cx) {
                Poll::Ready(Some(item)) => {
                    let whole = piece.len();
                    if let Err(e) = (this.frame)(&item, this.framing, &mut piece) {
                        // What the item wrote before it failed is not sent. The error goes after what is gathered.
                        piece.truncate(whole);
                        if piece.is_empty() {
                            return Poll::Ready(Some(Err(e)));
                        }
                        this.failed = Some(e);
                        break;
                    }
                }
                Poll::Ready(None) => this.ended = true,
                // Not ready. What is gathered goes now. The item that is still coming is polled next.
                Poll::Pending if piece.is_empty() => return Poll::Pending,
                Poll::Pending => break,
            }
        }
        Poll::Ready((!piece.is_empty()).then(|| Ok(bytes::Bytes::from(piece))))
    }
}

/// Puts sequential `items` into a request or a response. They are encoded at once or streamed as they are produced.
///
/// # Errors
/// An item of a `List` does not serialize.
pub fn set_items<T: Serialize + DeserializeOwned + Send + 'static>(
    message: &mut impl Message,
    content_type: impl Into<Cow<'static, str>>,
    items: Items<T>,
    framing: Framing,
) -> Result<(), ClientError> {
    message.set_body(content_type, items.into_body(framing)?);
    Ok(())
}

/// Puts a binary body into a request or a response at once or streamed chunk by chunk.
pub fn set_blob(message: &mut impl Message, content_type: impl Into<Cow<'static, str>>, blob: Blob) {
    message.set_body(content_type, blob.into_body());
}

/// Puts a binary body declared as a range (`image/*`, `*/*`) into a request or a response.
///
/// It goes out as the media type of the blob when that fits the range. Otherwise as `application/octet-stream`: no
/// `Content-Type` may contain a wildcard.
pub fn set_blob_in(message: &mut impl Message, range: &str, blob: Blob) {
    let media: Cow<'static, str> = match blob.media() {
        Some(m) if crate::media::media_matches(m, range) && !m.contains('*') => m.to_owned().into(),
        _ => "application/octet-stream".into(),
    };
    message.set_body(media, blob.into_body());
}

/// How items of a sequential media type are separated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// `application/jsonl`: one JSON value per line.
    JsonLines,
    /// `application/json-seq` (RFC 7464): RS, JSON, LF.
    JsonSeq,
    /// `text/event-stream`: events separated by a blank line.
    Sse,
    /// `multipart/mixed` and `multipart/byteranges` (an `itemSchema`). One item per part between the delimiters of
    /// the boundary of the response. Decoding only. A server returns such a body whole.
    #[cfg(feature = "multipart")]
    Multipart,
}

/// How long an event stream waits before it reconnects. This applies until the server sends a `retry` field.
const RETRY: std::time::Duration = std::time::Duration::from_secs(3);

/// The longest wait that [`backoff`] grows to. A longer `retry` wins.
const MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
/// Failed reconnections that double the wait: 64 times `RETRY` is past [`MAX_BACKOFF`] already.
const MAX_DOUBLINGS: u32 = 6;

/// Returns the wait before a reconnection. It is the `retry` of the stream doubled for each failed reconnection in a
/// row. It stops at a minute or at `retry` if that is longer.
fn backoff(retry: std::time::Duration, failures: u32) -> std::time::Duration {
    retry.saturating_mul(1 << failures.min(MAX_DOUBLINGS)).min(retry.max(MAX_BACKOFF))
}

/// What a reconnection of an event stream got.
pub enum Reconnected {
    /// The stream again: its body.
    Open(ByteStream),
    /// `204 No Content`: the server ends the stream.
    Done,
    /// The request failed or the server is busy (`429`) or failing (`5xx`). Wait and then try again.
    Retry(ClientError),
    /// Another status or a body that is not an event stream. The stream ends.
    Fatal(ClientError),
}

impl fmt::Debug for Reconnected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(_) => f.write_str("Open(..)"),
            Self::Done => f.write_str("Done"),
            Self::Retry(e) => f.debug_tuple("Retry").field(e).finish(),
            Self::Fatal(e) => f.debug_tuple("Fatal").field(e).finish(),
        }
    }
}

/// A reconnection of an event stream under way.
pub type Connecting = Pending<'static, Reconnected>;

/// Sends the request of an event stream again. It resumes after the last event that it received.
pub trait Reconnect: crate::transport::MaybeSend {
    /// Returns the request again with `Last-Event-ID` when `last_event_id` is not empty.
    fn connect(&mut self, last_event_id: &str) -> Connecting;
}

/// Returns the request that a generated `<op>_stream` sent. A reconnection uses it. `None` when its body was streamed
/// because that cannot be sent twice.
pub fn resendable(req: &Request) -> Option<Request> {
    (!req.body().is_streamed()).then(|| crate::message::copy_request(req))
}

/// Items of a sequential body that arrive or leave one at a time.
///
/// They are a request body that the server has not consumed, a response body that a client consumes as it arrives, or
/// items that your code produces. It is a `Stream` of items. Read them with `futures_util::StreamExt::next`. Clones
/// share it: what one consumes the others do not.
///
/// An arriving item is decoded where it lies as bytes arrive. A `text/event-stream` keeps the last event ID and
/// reconnection time of the stream from every block that it parses. The WHATWG `EventSource` does the same. A stream
/// that a generated `<op>_stream` returns also reconnects.
#[cfg_attr(feature = "client", doc = "See [`ItemStream::reconnecting`].")]
pub struct ItemStream<T>(Slot<Inner<T>>);

/// Reads the next item of an arriving body.
type Decode<T> = fn(&mut Decoder<T>, &mut Context<'_>) -> Poll<Option<Result<T, ClientError>>>;

/// What an [`ItemStream`] pulls from.
pub(crate) enum Inner<T> {
    /// Items decoded from a body as it arrives.
    Decoding(Box<Decoder<T>>),
    /// Items that your code produces.
    Produced(Feed<T>),
}

/// No item is pinned: each is decoded or pulled out of a boxed stream.
impl<T> Unpin for ItemStream<T> {}

impl<T> Clone for ItemStream<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> fmt::Debug for ItemStream<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("ItemStream");
        if let Ok(held) = self.0.lock()
            && let Some(Inner::Decoding(items)) = held.as_ref()
        {
            d.field("status", &items.status).field("framing", &items.framing);
            d.field("last_event_id", &items.last_event_id).field("retry", &items.retry);
        }
        d.finish_non_exhaustive()
    }
}

impl<T> ItemStream<T> {
    fn with(inner: Inner<T>) -> Self {
        Self(crate::source::shared(inner))
    }

    /// Returns the items of `feed`.
    pub(crate) fn produced(feed: Feed<T>) -> Self {
        Self::with(Inner::Produced(feed))
    }

    /// Returns whether both are the same stream: clones share it.
    pub(crate) fn same(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }

    /// Returns what the stream pulls from. `None` once something else moved it out.
    pub(crate) fn take_inner(&self) -> Option<Inner<T>> {
        take_slot(&self.0)
    }

    /// Returns the items that your code produces. `None` for a body that arrives.
    pub(crate) fn take_feed(&self) -> Option<Feed<T>> {
        match self.take_inner()? {
            Inner::Produced(feed) => Some(feed),
            decoding @ Inner::Decoding(_) => {
                put_slot(&self.0, decoding);
                None
            }
        }
    }

    /// Runs `f` on the decoder of an arriving body. `None` for items that your code produces.
    fn decoder<R>(&self, f: impl FnOnce(&mut Decoder<T>) -> R) -> Option<R> {
        let mut held = self.0.lock().ok()?;
        match held.as_mut()? {
            Inner::Decoding(items) => Some(f(items)),
            Inner::Produced(_) => None,
        }
    }

    /// Returns the `id` of the last event parsed from any block. A reconnection sends it as `Last-Event-ID`. It is
    /// empty before the first one and after an `id` field with no value.
    pub fn last_event_id(&self) -> String {
        self.decoder(|items| items.last_event_id.clone()).unwrap_or_default()
    }

    /// Returns how long the stream waits before it reconnects: 3 seconds until the server sends a `retry` field.
    pub fn retry(&self) -> std::time::Duration {
        self.decoder(|items| items.retry).unwrap_or(RETRY)
    }

    /// Returns the HTTP status of the response. `None` for a request body and for items that your code produces.
    pub fn status(&self) -> Option<u16> {
        self.decoder(|items| items.status).flatten()
    }

    /// Ends the stream after `n` reconnections in a row fail. Without this it tries for ever.
    #[must_use]
    pub fn max_retries(self, n: u32) -> Self {
        self.decoder(|items| items.max_retries = Some(n));
        self
    }

    /// Ends the stream where the body ends. Do not reconnect.
    #[must_use]
    pub fn no_reconnect(self) -> Self {
        self.decoder(|items| items.reconnect = None);
        self
    }

    /// Sends the request again through `reconnect` when an event stream ends or breaks off.
    #[cfg(feature = "client")]
    pub(crate) fn set_reconnect(&self, reconnect: Box<dyn Reconnect>, last_event_id: Option<&str>) {
        self.decoder(|items| {
            if items.framing == Framing::Sse {
                if let Some(id) = last_event_id {
                    id.clone_into(&mut items.last_event_id);
                }
                items.reconnect = Some(reconnect);
            }
        });
    }

    /// Polls what the stream pulls from. `decode` decodes an arriving body. Usually no clone shares the stream. Then it
    /// is polled without the lock.
    fn poll_with(&mut self, cx: &mut Context<'_>, decode: Decode<T>) -> Poll<Option<Result<T, ClientError>>> {
        let poll = |held: &mut Option<Inner<T>>, cx: &mut Context<'_>| match held.as_mut() {
            Some(Inner::Decoding(items)) => decode(items, cx),
            Some(Inner::Produced(feed)) => feed.as_mut().poll_next(cx).map(|item| item.map(Ok)),
            None => Poll::Ready(None),
        };
        if let Some(held) = std::sync::Arc::get_mut(&mut self.0).and_then(|m| m.get_mut().ok()) {
            return poll(held, cx);
        }
        match self.0.lock() {
            Ok(mut held) => poll(&mut held, cx),
            Err(_) => Poll::Ready(None),
        }
    }
}

impl<T: DeserializeOwned> ItemStream<T> {
    /// Creates items of `body`. It is a response with this status.
    pub fn new(status: Option<u16>, body: ByteStream, framing: Framing) -> Self {
        Self::with(Inner::Decoding(Box::new(Decoder::new(status, body, framing))))
    }

    /// Reads each item with the text of its library formats validated first, as `H` says (`runtime::formats`).
    #[doc(hidden)]
    #[must_use]
    pub fn validated<H: for<'de> crate::formats::Holds<'de, T>>(self) -> Self {
        self.decoder(|items| items.item = read_validated::<T, H>);
        self
    }

    /// Checks the sequence against `limits` as its items arrive.
    #[doc(hidden)]
    #[must_use]
    pub fn limited(self, limits: Limits) -> Self
    where
        T: Serialize,
    {
        let key: fn(&T) -> Option<String> = crate::checks::unique_key::<T>;
        self.decoder(|items| {
            items.counted = Some(Box::new(Counted {
                limits,
                count: 0,
                seen: std::collections::BTreeSet::new(),
                key,
                short: false,
            }));
        });
        self
    }

    /// Returns the parts of a `multipart/*` body. It is a response with this status and `Content-Type`. The parts are
    /// decoded as they arrive.
    #[cfg(feature = "multipart")]
    #[must_use]
    pub fn multipart(status: Option<u16>, body: ByteStream, content_type: &str) -> Self {
        let mut items = Decoder::new(status, body, Framing::Multipart);
        items.boundary = crate::codec::boundary_of(content_type).unwrap_or_default();
        Self::with(Inner::Decoding(Box::new(items)))
    }
}

/// Each item as it is decoded or produced.
///
/// A `text/event-stream` that reconnects ends only on a `204 No Content` or a response that is not the stream. It
/// also ends after [`ItemStream::max_retries`]. A reconnection that fails is an `Err`. The next poll tries again.
impl<T: DeserializeOwned> Stream for ItemStream<T> {
    type Item = Result<T, ClientError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_with(cx, Decoder::poll_limited)
    }
}

/// The keywords of a sequence as a whole: `minItems`, `maxItems` and `uniqueItems` of its `schema`. A sequence that
/// arrives an item at a time is checked as it arrives: the item past `max` and an item met before are an `Err`, and
/// so is the end of a sequence short of `min`.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// `minItems`.
    pub min: Option<u64>,
    /// `maxItems`.
    pub max: Option<u64>,
    /// `uniqueItems`.
    pub unique: bool,
}

/// Returns `items` once they meet `limits`, as one sequence received whole.
///
/// # Errors
/// They do not: a decode error with this status.
#[doc(hidden)]
pub fn limit_items<T: Serialize>(status: Option<u16>, items: Vec<T>, limits: Limits) -> Result<Vec<T>, ClientError> {
    let mut out = vec![];
    crate::checks::check_len(&mut out, &InstancePath::Root, items.len(), limits.min, limits.max, "items");
    if limits.unique {
        crate::checks::check_unique(&mut out, &InstancePath::Root, &items);
    }
    match out.into_iter().next() {
        Some(v) => Err(limit_error(status, v.message)),
        None => Ok(items),
    }
}

/// Returns the error of a sequence that breaks one of its [`Limits`].
fn limit_error(status: Option<u16>, message: String) -> ClientError {
    ClientError::Decode { status, message, body: bytes::Bytes::new(), pointer: String::new() }
}

/// What a sequence checked against its [`Limits`] has seen so far.
struct Counted<T> {
    limits: Limits,
    count: u64,
    /// The canonical JSON of each item seen: `uniqueItems` only.
    seen: std::collections::BTreeSet<String>,
    key: fn(&T) -> Option<String>,
    /// Whether the end short of `min` was reported: once.
    short: bool,
}

impl<T> Counted<T> {
    /// Returns `item` once it keeps the sequence within its limits.
    fn admit(&mut self, status: Option<u16>, item: T) -> Result<T, ClientError> {
        self.count += 1;
        if let Some(max) = self.limits.max.filter(|max| self.count > *max) {
            return Err(limit_error(status, format!("at most {max} items allowed, found more")));
        }
        if self.limits.unique && (self.key)(&item).is_some_and(|k| !self.seen.insert(k)) {
            return Err(limit_error(status, "items must be unique".to_owned()));
        }
        Ok(item)
    }

    /// Returns the error of a sequence that ended short of `min`, once.
    fn at_end(&mut self, status: Option<u16>) -> Option<ClientError> {
        let min = self.limits.min.filter(|min| self.count < *min && !self.short)?;
        self.short = true;
        Some(limit_error(status, format!("at least {min} items required, found {}", self.count)))
    }
}

/// Decodes the items of a body as its bytes arrive.
pub(crate) struct Decoder<T> {
    status: Option<u16>,
    body: ByteStream,
    /// Bytes received; those before `pos` are records already returned.
    buf: Vec<u8>,
    pos: usize,
    /// Where the search for the next record's end resumes: the bytes from `pos` to here contain none.
    scanned: usize,
    framing: Framing,
    done: bool,
    /// Whether the first bytes were seen (an SSE stream may start with a BOM).
    started: bool,
    /// `Framing::Multipart`: the boundary of the `Content-Type` of the response and whether its closing delimiter came.
    #[cfg(feature = "multipart")]
    boundary: String,
    #[cfg(feature = "multipart")]
    closed: bool,
    /// JSON Lines and `json-seq`: the frame being parsed and where in it the next record starts. Its records are
    /// decoded where they lie. Only a record that spans two frames is joined in `buf`. `buf` then contains only that
    /// record.
    frame: bytes::Bytes,
    at: usize,
    /// SSE: the last `id` parsed and the wait before a reconnection. A reconnection sends the `id` as `Last-Event-ID`.
    last_event_id: String,
    retry: std::time::Duration,
    /// SSE: sends the request again when the stream ends or breaks off; `None` once it may not.
    reconnect: Option<Box<dyn Reconnect>>,
    /// The reconnection under way: its wait and then its request.
    connecting: Option<Connecting>,
    /// Failed reconnections in a row and how many may fail.
    failures: u32,
    max_retries: Option<u32>,
    /// Reads one record as an item: as `T`, or with the text of its library formats checked first.
    item: ReadItem<T>,
    /// The [`Limits`] of the sequence and what it has seen: `None` without any.
    counted: Option<Box<Counted<T>>>,
}

/// Reads one record as an item.
type ReadItem<T> = fn(&Record<'_>) -> Result<T, ClientError>;

/// One record of an arriving body and what decoding it needs.
pub(crate) struct Record<'a> {
    status: Option<u16>,
    framing: Framing,
    /// `Framing::Multipart`: the boundary of the body.
    #[cfg(feature = "multipart")]
    boundary: &'a str,
    bytes: &'a [u8],
}

/// Returns `record` decoded as `U`: `T`, or `T` decoded through `runtime::formats`.
fn decode<U: DeserializeOwned>(record: &Record<'_>) -> Result<U, ClientError> {
    let (status, bytes) = (record.status, record.bytes);
    match record.framing {
        Framing::JsonLines | Framing::JsonSeq => crate::codec::json_record(status, bytes),
        Framing::Sse => sse_item(status, &String::from_utf8_lossy(bytes)),
        #[cfg(feature = "multipart")]
        Framing::Multipart => {
            // One part decoded as a one-part body would be. Headers, raw bytes and media types are as in a whole
            // body.
            let b = record.boundary;
            let mut one = format!("--{b}\r\n").into_bytes();
            one.extend_from_slice(bytes);
            one.extend_from_slice(format!("\r\n--{b}--\r\n").as_bytes());
            let ct = format!("multipart/mixed; boundary={b}");
            let items: Vec<U> = crate::codec::mixed_decode(status, &ct, &one, None)?;
            items.into_iter().next().ok_or_else(|| decode_err(status, bytes, "an empty part"))
        }
    }
}

/// Decodes a record as `Validated<T, H>`: the text of each library format in `T` is checked before its library parses
/// it.
fn read_validated<T: DeserializeOwned, H: for<'de> crate::formats::Holds<'de, T>>(
    record: &Record<'_>,
) -> Result<T, ClientError> {
    decode::<crate::formats::Validated<T, H>>(record).map(crate::formats::Validated::into_inner)
}

impl<T: DeserializeOwned> Decoder<T> {
    fn new(status: Option<u16>, body: ByteStream, framing: Framing) -> Self {
        Self {
            status,
            body,
            buf: vec![],
            pos: 0,
            scanned: 0,
            framing,
            done: false,
            started: false,
            #[cfg(feature = "multipart")]
            boundary: String::new(),
            #[cfg(feature = "multipart")]
            closed: false,
            frame: bytes::Bytes::new(),
            at: 0,
            last_event_id: String::new(),
            retry: RETRY,
            reconnect: None,
            connecting: None,
            failures: 0,
            max_retries: None,
            item: decode::<T>,
            counted: None,
        }
    }

    /// Returns the next item checked against the [`Limits`] of the sequence, and the error of an end short of them.
    fn poll_limited(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<T, ClientError>>> {
        let next = ready!(self.poll_item(cx));
        let status = self.status;
        let Some(counted) = self.counted.as_mut() else { return Poll::Ready(next) };
        Poll::Ready(match next {
            Some(Ok(item)) => Some(counted.admit(status, item)),
            None => counted.at_end(status).map(Err),
            other @ Some(Err(_)) => other,
        })
    }

    /// Returns the bytes received but not yet returned as items. Also the body still to come.
    fn into_unread(self) -> (Vec<u8>, ByteStream) {
        let Self { mut buf, pos, frame, at, body, .. } = self;
        buf.drain(..pos.min(buf.len()));
        buf.extend_from_slice(frame.get(at..).unwrap_or_default());
        (buf, body)
    }

    /// Returns the next item or `None` once the body has ended.
    fn poll_item(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<T, ClientError>>> {
        if matches!(self.framing, Framing::JsonLines | Framing::JsonSeq) {
            return self.poll_line(cx);
        }
        loop {
            // A record is decoded where it lies in the buffer. It is never copied out.
            if let Some(at) = self.take_record() {
                if self.framing == Framing::Sse {
                    let text = String::from_utf8_lossy(self.buf.get(at).unwrap_or_default());
                    let (id, retry) = sse_state(&text);
                    if let Some(id) = id {
                        id.clone_into(&mut self.last_event_id);
                    }
                    if let Some(ms) = retry {
                        self.retry = std::time::Duration::from_millis(ms);
                    }
                    if crate::codec::blank_record(text.as_bytes()) || sse_comment_only(&text) {
                        continue;
                    }
                    return Poll::Ready(Some(sse_item(self.status, &text)));
                }
                let record = self.buf.get(at).unwrap_or_default();
                if self.skips(record) {
                    continue;
                }
                return Poll::Ready(Some((self.item)(&self.record(record))));
            }
            if self.done && self.framing == Framing::Sse {
                // An event the body ended inside is dropped: the stream broke off before it was whole.
                self.pos = self.buf.len();
                self.scanned = self.pos;
                match ready!(self.poll_reconnect(cx)) {
                    None => return Poll::Ready(None),
                    Some(Ok(())) => continue,
                    Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                }
            }
            if self.done {
                let rest = self.pos.min(self.buf.len())..self.buf.len();
                self.pos = self.buf.len();
                let record = self.buf.get(rest).unwrap_or_default();
                if self.skips(record) {
                    return Poll::Ready(None);
                }
                return Poll::Ready(Some((self.item)(&self.record(record))));
            }
            match ready!(Pin::new(&mut self.body).poll_next(cx)) {
                Some(Ok(chunk)) => {
                    // Returned records are dropped once they are half the buffer: each byte moves at most once.
                    if self.pos > self.buf.len() / 2 {
                        self.buf.drain(..self.pos);
                        self.scanned -= self.pos;
                        self.pos = 0;
                    }
                    self.buf.extend_from_slice(&chunk);
                }
                // A stream that reconnects resumes after a break as after its end.
                Some(Err(_)) if self.reconnect.is_some() => self.done = true,
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => self.done = true,
            }
        }
    }

    /// Sends the request again after the wait. `None` when the stream may not. `Err` when this attempt failed.
    fn poll_reconnect(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<(), ClientError>>> {
        if self.connecting.is_none() {
            if self.max_retries.is_some_and(|max| self.failures >= max) {
                self.reconnect = None;
            }
            let Some(again) = self.reconnect.as_mut() else { return Poll::Ready(None) };
            let wait = crate::transport::sleep(backoff(self.retry, self.failures));
            let connect = again.connect(&self.last_event_id);
            self.connecting = Some(Box::pin(async move {
                wait.await;
                connect.await
            }));
        }
        let Some(connecting) = self.connecting.as_mut() else { return Poll::Ready(None) };
        let got = ready!(connecting.as_mut().poll(cx));
        self.connecting = None;
        Poll::Ready(match got {
            Reconnected::Open(body) => {
                self.body = body;
                self.buf.clear();
                (self.pos, self.scanned, self.done, self.started, self.failures) = (0, 0, false, false, 0);
                Some(Ok(()))
            }
            Reconnected::Done => {
                self.reconnect = None;
                None
            }
            Reconnected::Retry(e) => {
                self.failures += 1;
                Some(Err(e))
            }
            Reconnected::Fatal(e) => {
                self.reconnect = None;
                Some(Err(e))
            }
        })
    }

    /// Returns the next item of a line framing. It is decoded where it lies in its frame. A line that spans two frames
    /// is joined in `buf` first. Nothing else is copied.
    fn poll_line(&mut self, cx: &mut Context<'_>) -> Poll<Option<Result<T, ClientError>>> {
        loop {
            let rest = self.frame.get(self.at..).unwrap_or_default();
            if let Some(end) = memchr::memchr(b'\n', rest) {
                let line = self.at..self.at + end;
                self.at += end + 1;
                if !self.buf.is_empty() {
                    self.buf.extend_from_slice(self.frame.get(line.clone()).unwrap_or_default());
                }
                let record =
                    if self.buf.is_empty() { self.frame.get(line).unwrap_or_default() } else { self.buf.as_slice() };
                let item = (!self.skips(record)).then(|| (self.item)(&self.record(record)));
                self.buf.clear();
                match item {
                    Some(item) => return Poll::Ready(Some(item)),
                    None => continue,
                }
            }
            self.buf.extend_from_slice(rest);
            self.at = self.frame.len();
            if self.done {
                let item = (!self.skips(&self.buf)).then(|| (self.item)(&self.record(&self.buf)));
                self.buf.clear();
                return Poll::Ready(item);
            }
            match ready!(Pin::new(&mut self.body).poll_next(cx)) {
                Some(Ok(frame)) => (self.frame, self.at) = (frame, 0),
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => self.done = true,
            }
        }
    }

    /// Returns a record that carries no item. It is blank or an SSE comment (a heartbeat).
    fn skips(&self, record: &[u8]) -> bool {
        #[cfg(feature = "multipart")]
        if self.framing == Framing::Multipart {
            // After the closing delimiter is the epilogue. It carries no part.
            return self.closed && record.is_empty();
        }
        crate::codec::blank_record(record)
            || (self.framing == Framing::Sse && sse_comment_only(&String::from_utf8_lossy(record)))
    }

    /// Returns where the next complete SSE event or multipart part lies in the buffer if it contains one. It counts as
    /// consumed.
    fn take_record(&mut self) -> Option<std::ops::Range<usize>> {
        #[cfg(feature = "multipart")]
        if self.framing == Framing::Multipart {
            return self.take_part();
        }
        if self.framing == Framing::Sse && !self.started {
            // The BOM may arrive a byte at a time: decide once it could be whole.
            if self.buf.len() < BOM.len() && !self.done {
                return None;
            }
            self.started = true;
            if self.buf.starts_with(BOM.as_bytes()) {
                self.pos = BOM.len();
                self.scanned = self.pos;
            }
        }
        let pending = self.buf.get(self.pos..)?;
        let from = self.scanned - self.pos;
        let (end, consumed) = match sse_boundary(pending, from, self.done) {
            Ok(found) => found,
            Err(stopped) => {
                self.scanned = self.pos + stopped;
                return None;
            }
        };
        let record = self.pos..self.pos + end;
        self.pos += consumed;
        self.scanned = self.pos;
        Some(record)
    }

    /// Returns the record at `bytes` with what decoding it needs.
    fn record<'a>(&'a self, bytes: &'a [u8]) -> Record<'a> {
        Record {
            status: self.status,
            framing: self.framing,
            #[cfg(feature = "multipart")]
            boundary: &self.boundary,
            bytes,
        }
    }

    /// Returns the next whole part with its headers and body between two delimiter lines. The preamble before the first
    /// delimiter and the epilogue after the closing one are skipped.
    #[cfg(feature = "multipart")]
    fn take_part(&mut self) -> Option<std::ops::Range<usize>> {
        if self.closed {
            self.pos = self.buf.len();
            self.scanned = self.pos;
            return None;
        }
        let delimiter = format!("--{}", self.boundary);
        let pending = self.buf.get(self.pos..)?;
        if !self.started {
            // The first delimiter line. It is at the start or after the preamble.
            let at = find(pending, delimiter.as_bytes())?;
            let eol = find(&pending[at..], b"\n")?;
            self.pos += at + eol + 1;
            self.started = true;
            self.scanned = self.pos;
            return self.take_part();
        }
        let next = find(pending, format!("\r\n{delimiter}").as_bytes())?;
        let after = next + 2 + delimiter.len();
        let rest = pending.get(after..)?;
        // The delimiter line must be whole. `--` closes the body. Otherwise a line break opens the next part.
        if rest.len() < 2 && !self.done {
            return None;
        }
        let record = self.pos..self.pos + next;
        if rest.starts_with(b"--") {
            self.closed = true;
            self.pos = self.buf.len();
        } else {
            let eol = find(rest, b"\n")?;
            self.pos += after + eol + 1;
        }
        self.scanned = self.pos;
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::StreamExt;

    use super::{Framing, ItemStream, Items, backoff};
    use crate::{Body, ByteStream, Bytes};

    #[test]
    fn the_wait_doubles_up_to_a_minute_or_the_retry() {
        let s = Duration::from_secs;
        let waits: Vec<Duration> = (0..8).map(|f| backoff(s(1), f)).collect();
        assert_eq!(waits, [s(1), s(2), s(4), s(8), s(16), s(32), s(60), s(60)]);
        assert_eq!(backoff(s(90), 3), s(90), "a longer retry is the cap");
    }

    fn body(text: &'static str) -> ByteStream {
        ByteStream::chunks(futures_util::stream::iter([Ok(Bytes::from_static(text.as_bytes()))]), None)
    }

    #[test]
    fn items_are_equal_by_list_or_by_stream() {
        assert_eq!(Items::List(vec![1]), Items::List(vec![1]));
        assert_ne!(Items::List(vec![1]), Items::List(vec![2]));
        let stream: ItemStream<i32> = ItemStream::new(Some(200), body("1\n"), Framing::JsonLines);
        let other: ItemStream<i32> = ItemStream::new(Some(200), body("1\n"), Framing::JsonLines);
        assert_eq!(Items::Stream(stream.clone()), Items::Stream(stream.clone()));
        assert_ne!(Items::Stream(stream.clone()), Items::Stream(other));
        assert_ne!(Items::Stream(stream), Items::List(vec![1]));
    }

    /// Asking a decoding stream for the items of your code gives nothing and leaves the stream as it was.
    #[test]
    fn a_decoding_stream_is_no_feed_and_still_reads() {
        let mut stream: ItemStream<i32> = ItemStream::new(Some(200), body("1\n2\n"), Framing::JsonLines);
        assert!(stream.take_feed().is_none());
        let got: Vec<i32> = crate::block_on(stream.by_ref().map(Result::unwrap).collect());
        assert_eq!(got, [1, 2]);
    }

    #[test]
    fn a_list_is_in_memory() {
        assert_eq!(Items::List(vec![1]).into_list(), Some(vec![1]));
    }

    /// A `retry` or a comment exists only in `text/event-stream`. Other framings write nothing for it.
    #[test]
    fn retry_and_comments_are_written_only_as_events() {
        let framed = |record: &super::Sse<i32>, framing| {
            let mut out = vec![];
            super::frame_record(record, framing, &mut out).unwrap();
            String::from_utf8(out).unwrap()
        };
        let retry = super::Sse::Retry(Duration::from_millis(1500));
        let comment = super::Sse::Comment("hi".to_owned());
        assert_eq!(framed(&retry, Framing::Sse), "retry: 1500\n\n");
        assert_eq!(framed(&comment, Framing::Sse), ": hi\n\n");
        assert_eq!(
            (framed(&retry, Framing::JsonLines), framed(&comment, Framing::JsonSeq)),
            (String::new(), String::new())
        );
    }

    #[test]
    fn a_request_with_a_streamed_body_cannot_be_sent_again() {
        let mut req = crate::message::new_request("POST", "/a".to_owned(), vec![]).unwrap();
        *req.body_mut() = Body::from("x");
        assert_eq!(super::resendable(&req).map(|again| again.body().bytes().to_vec()), Some(b"x".to_vec()));
        *req.body_mut() = Body::from(body("x"));
        assert!(super::resendable(&req).is_none());
    }
}
