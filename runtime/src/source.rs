//! Bodies and items produced one piece at a time: what a request or a response sends or receives as it goes.

use std::borrow::Cow;
use std::fmt;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use futures_core::Stream;

use crate::{Error, MaybeSend};

/// A body produced piece by piece. It is polled in place as an `http_body::Body` is: a piece costs no allocation of
/// its own.
pub trait BodySource: MaybeSend + 'static {
    /// Returns the next piece, or `Ready(None)` at the end of the body. Nothing follows an error.
    fn poll_piece(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<bytes::Bytes, Error>>>;

    /// Returns the whole body when the source already holds it in one shared buffer. A transport sends that buffer as
    /// it is with its length. It does not copy it out piece by piece. `None` for every other source.
    fn shared(&self) -> Option<bytes::Bytes> {
        None
    }

    /// Returns the bytes still to come when the source knows them before it produces them. A transport then sends the
    /// body with `Content-Length` and not chunked. Some servers need this, such as an S3 presigned `PUT` and many
    /// embedded ones. `None` for a source that does not know.
    fn size(&self) -> Option<u64> {
        self.shared().map(|b| b.len() as u64)
    }

    /// Returns the length of the whole body when the source can start anywhere in it and has sent nothing yet. The
    /// server then answers a `Range` request with [`BodySource::cut`]. `None` for every other source.
    fn seekable(&self) -> Option<u64> {
        None
    }

    /// Sends only bytes `first` to `last` of the whole body, both included. The server calls it only when
    /// [`BodySource::seekable`] gave the length and the range lies inside it.
    fn cut(self: Pin<&mut Self>, _first: u64, _last: u64) {}

    /// Returns the rest of the body in one buffer or why not. The cause is more than `limit` bytes or a piece that
    /// failed. A source that can read its body at once overrides this, such as a server adapter that reads into a
    /// buffer sized from `Content-Length`. This default joins the pieces. It keeps a lone piece as it is.
    fn read_all(mut self: Pin<Box<Self>>, limit: usize) -> ReadAll {
        Box::pin(async move {
            let mut out: Vec<u8> = vec![];
            while let Some(piece) = std::future::poll_fn(|cx| self.as_mut().poll_piece(cx)).await {
                let piece = piece.map_err(|e| match e {
                    Error::TooLarge(_) => BodyError::TooLarge,
                    other @ (Error::Transport(_)
                    | Error::Encode(_)
                    | Error::Decode { .. }
                    | Error::UnexpectedMedia { .. }
                    | Error::NotModified { .. }
                    | Error::UnexpectedStatus { .. }
                    | Error::Invalid(_)) => BodyError::Unreadable(other.to_string()),
                })?;
                if out.len() + piece.len() > limit {
                    return Err(BodyError::TooLarge);
                }
                if out.is_empty() {
                    out = Vec::from(piece);
                } else {
                    out.extend_from_slice(&piece);
                }
            }
            Ok(out)
        })
    }
}

/// Returns the next piece of `source`, or `None` at its end.
#[doc(hidden)]
pub async fn next_piece(source: &mut Pin<Box<dyn BodySource>>) -> Option<Result<bytes::Bytes, Error>> {
    std::future::poll_fn(|cx| source.as_mut().poll_piece(cx)).await
}

/// A whole body read by [`BodySource::read_all`].
pub type ReadAll = crate::transport::Pending<'static, Result<Vec<u8>, BodyError>>;

/// Why a request body could not be read whole.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BodyError {
    /// More bytes than the limit.
    TooLarge,
    /// The connection failed while the body arrived.
    Unreadable(String),
}

impl fmt::Display for BodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("body over the size limit"),
            Self::Unreadable(why) => write!(f, "body unreadable: {why}"),
        }
    }
}

/// A body that arrives or leaves a chunk at a time.
///
/// It is a request body that the server has not read, a response body that a client reads as it arrives, or chunks
/// that your code produces. It is a `Stream` of chunks. Read it with `futures_util::StreamExt::next`. Nothing follows
/// an error. Clones share it: what one reads the others do not.
#[derive(Clone, Default)]
pub struct ByteStream(Option<Slot<Pin<Box<dyn BodySource>>>>);

/// A value shared by clones and taken once.
pub(crate) type Slot<T> = std::sync::Arc<std::sync::Mutex<Option<T>>>;

/// Takes what `slot` holds; later calls get `None` until it is put back.
pub(crate) fn take_slot<T>(slot: &std::sync::Mutex<Option<T>>) -> Option<T> {
    slot.lock().ok().and_then(|mut s| s.take())
}

/// Returns a slot that holds `value`.
pub(crate) fn shared<T>(value: T) -> Slot<T> {
    std::sync::Arc::new(std::sync::Mutex::new(Some(value)))
}

/// Puts `value` back into `slot`.
pub(crate) fn put_slot<T>(slot: &std::sync::Mutex<Option<T>>, value: T) {
    if let Ok(mut s) = slot.lock() {
        *s = Some(value);
    }
}

impl ByteStream {
    /// Creates a body read from `source`.
    #[doc(hidden)]
    pub fn new(source: impl BodySource) -> Self {
        Self::pinned(Box::pin(source))
    }

    /// Returns a body read from `source`, already pinned.
    pub(crate) fn pinned(source: Pin<Box<dyn BodySource>>) -> Self {
        Self(Some(shared(source)))
    }

    /// Returns a body read from the chunks of `chunks`. With `len` it goes out with `Content-Length` and not chunked.
    #[doc(hidden)]
    pub fn chunks<S>(chunks: S, len: Option<u64>) -> Self
    where
        S: Stream<Item = Result<bytes::Bytes, Error>> + MaybeSend + 'static,
    {
        Self::new(Chunks { chunks, left: len })
    }

    /// Returns the bytes still to come when the source knows them.
    pub(crate) fn size(&self) -> Option<u64> {
        self.0.as_ref()?.lock().ok()?.as_ref()?.size()
    }

    /// Is a body attached that nothing has taken?
    pub fn is_some(&self) -> bool {
        self.0.as_ref().is_some_and(|m| m.lock().is_ok_and(|s| s.is_some()))
    }

    /// Takes the source; later calls get `None`.
    #[doc(hidden)]
    pub fn take_source(&self) -> Option<Pin<Box<dyn BodySource>>> {
        self.0.as_ref().and_then(|m| take_slot(m))
    }

    /// Returns the length of the whole body when its source can start anywhere in it. See [`BodySource::seekable`].
    #[cfg(feature = "server")]
    pub(crate) fn seekable(&self) -> Option<u64> {
        let held = self.0.as_ref()?.lock().ok()?;
        held.as_ref()?.seekable()
    }

    /// Makes the source send only bytes `first` to `last`. See [`BodySource::cut`].
    #[cfg(feature = "server")]
    pub(crate) fn cut(&self, first: u64, last: u64) {
        if let Some(Ok(mut held)) = self.0.as_ref().map(|m| m.lock())
            && let Some(source) = held.as_mut()
        {
            source.as_mut().cut(first, last);
        }
    }

    /// Returns every byte in one buffer.
    ///
    /// # Errors
    /// A chunk that failed: the connection broke off or the body went past the `max_body_bytes` of the server.
    pub async fn collect(mut self) -> Result<Vec<u8>, Error> {
        let mut out = vec![];
        while let Some(chunk) = futures_util::StreamExt::next(&mut self).await {
            out.extend_from_slice(&chunk?);
        }
        Ok(out)
    }
}

/// The source in the slot is polled where it is. A chunk costs no allocation of its own. At the end or after an
/// error the slot is emptied.
impl Stream for ByteStream {
    type Item = Result<bytes::Bytes, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(slot) = self.get_mut().0.as_mut() else { return Poll::Ready(None) };
        // Usually no clone shares the body. Then it is read without the lock.
        let poll = |held: &mut Option<Pin<Box<dyn BodySource>>>, cx: &mut Context<'_>| {
            let Some(source) = held.as_mut() else { return Poll::Ready(None) };
            let piece = ready!(source.as_mut().poll_piece(cx));
            if !matches!(piece, Some(Ok(_))) {
                *held = None;
            }
            Poll::Ready(piece)
        };
        if let Some(held) = std::sync::Arc::get_mut(slot).and_then(|m| m.get_mut().ok()) {
            return poll(held, cx);
        }
        match slot.lock() {
            Ok(mut held) => poll(&mut held, cx),
            Err(_) => Poll::Ready(None),
        }
    }
}

impl fmt::Debug for ByteStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() { "ByteStream(..)" } else { "ByteStream(none)" })
    }
}

impl PartialEq for ByteStream {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            (None | Some(_), _) => false,
        }
    }
}

impl Eq for ByteStream {}

/// Items that your code produces one at a time while the body is sent.
pub(crate) type Feed<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

/// A binary body: every byte in memory or a body that arrives or leaves a chunk at a time.
///
/// A large download or upload streams. Clients receive responses as `Bytes`. `<op>_stream` reads chunks as they
/// arrive. Server handlers receive binary request bodies as `Stream`: an upload is never held whole. A `Blob` is a
/// `Stream` of its chunks.
///
/// It may carry its media type, as a browser `Blob` does. A body that the description declares as a range
/// (`image/*`, `*/*`) goes out with it. A client and a server fill it from the `Content-Type` of such a body.
#[derive(Debug, Clone)]
pub struct Blob {
    body: BlobBody,
    media: Option<Cow<'static, str>>,
}

/// The bytes of a [`Blob`].
#[derive(Debug, Clone)]
enum BlobBody {
    /// Every byte in memory. Clones share the buffer. So the same `Params` given to a client again copies nothing.
    Bytes(bytes::Bytes),
    /// Chunks still to come: a request body the server has not read or chunks that your code produces.
    Stream(ByteStream),
}

impl Blob {
    /// Returns a body of the chunks that `chunks` produces while it is sent.
    pub fn stream<S, B>(chunks: S) -> Self
    where
        S: Stream<Item = B> + Send + 'static,
        B: Into<bytes::Bytes>,
    {
        Self::of(BlobBody::Stream(ByteStream::chunks(futures_util::StreamExt::map(chunks, |b| Ok(b.into())), None)))
    }

    /// Returns a body of chunks that add up to `len` bytes. It goes out with `Content-Length` and not chunked. A server
    /// that reads no chunked body takes it, such as some embedded ones and an S3 presigned `PUT`. Chunks that add up to
    /// another length fail the request.
    pub fn sized<S, B>(len: u64, chunks: S) -> Self
    where
        S: Stream<Item = B> + Send + 'static,
        B: Into<bytes::Bytes>,
    {
        Self::of(BlobBody::Stream(ByteStream::chunks(
            futures_util::StreamExt::map(chunks, |b| Ok(b.into())),
            Some(len),
        )))
    }

    /// Returns a body of `len` bytes that can start anywhere in it, such as a file. `open(first, last)` gives the
    /// chunks of bytes `first` to `last`, both included. The server calls it once, when it sends the body: for the
    /// whole body or for the one range that a `Range` request asks for. A download that broke off then resumes and a
    /// player seeks without the bytes before them being read. Chunks that add up to another length fail the response.
    pub fn ranged<F, S, B>(len: u64, open: F) -> Self
    where
        F: FnOnce(u64, u64) -> S + Send + 'static,
        S: Stream<Item = B> + Send + 'static,
        B: Into<bytes::Bytes>,
    {
        let open: Open =
            Box::new(move |first, last| Box::pin(futures_util::StreamExt::map(open(first, last), |b| Ok(b.into()))));
        Self::of(BlobBody::Stream(ByteStream::new(Ranged { len, first: 0, left: len, open: Some(open), chunks: None })))
    }

    const fn of(body: BlobBody) -> Self {
        Self { body, media: None }
    }

    /// Returns the same body with its media type, such as `image/png`. It goes out as the `Content-Type` of a body that
    /// the description declares as a range it fits (`image/*`).
    #[must_use]
    pub fn with_media(mut self, media: impl Into<Cow<'static, str>>) -> Self {
        self.media = Some(media.into());
        self
    }

    /// Returns the media type: the one given with [`Blob::with_media`] or the `Content-Type` it was received with.
    pub fn media(&self) -> Option<&str> {
        self.media.as_deref()
    }

    /// Returns the same body with the media type of a `Content-Type` that was received. An empty one gives none.
    #[must_use]
    pub fn received_as(self, content_type: &str) -> Self {
        let media = content_type.trim();
        if media.is_empty() { self } else { self.with_media(media.to_owned()) }
    }

    /// Returns whether chunks are still to come. A request body that the server has not read has them.
    pub const fn is_streamed(&self) -> bool {
        matches!(self.body, BlobBody::Stream(_))
    }

    /// Returns the bytes if they are in memory.
    pub fn into_bytes(self) -> Option<Vec<u8>> {
        match self.body {
            BlobBody::Bytes(b) => Some(Vec::from(b)),
            BlobBody::Stream(_) => None,
        }
    }

    /// Returns every byte in one buffer.
    ///
    /// # Errors
    /// As [`ByteStream::collect`].
    pub async fn collect(self) -> Result<Vec<u8>, Error> {
        match self.body {
            BlobBody::Bytes(b) => Ok(Vec::from(b)),
            BlobBody::Stream(s) => s.collect().await,
        }
    }

    /// Returns the body as a request or a response carries it: bytes or chunks still to come. Shared bytes are not
    /// copied.
    pub fn into_body(self) -> Body {
        match self.body {
            BlobBody::Bytes(b) => Body::new(b),
            BlobBody::Stream(stream) => Body::stream(stream),
        }
    }

    /// Returns a binary request body as the server received it: arriving when the adapter left it unread in `stream`,
    /// else `body`.
    pub fn received(body: Cow<'_, [u8]>, stream: &ByteStream) -> Self {
        match stream.take_source() {
            Some(source) => Self::of(BlobBody::Stream(ByteStream::pinned(source))),
            None => Self::of(BlobBody::Bytes(body.into_owned().into())),
        }
    }
}

/// The bytes at once or each chunk as it comes. Nothing follows an error.
impl Stream for Blob {
    type Item = Result<bytes::Bytes, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut self.get_mut().body {
            BlobBody::Bytes(b) => Poll::Ready((!b.is_empty()).then(|| Ok(std::mem::take(b)))),
            BlobBody::Stream(s) => Pin::new(s).poll_next(cx),
        }
    }
}

impl Default for Blob {
    fn default() -> Self {
        Self::of(BlobBody::Bytes(bytes::Bytes::new()))
    }
}

impl From<Vec<u8>> for Blob {
    fn from(v: Vec<u8>) -> Self {
        Self::of(BlobBody::Bytes(v.into()))
    }
}

impl From<bytes::Bytes> for Blob {
    fn from(b: bytes::Bytes) -> Self {
        Self::of(BlobBody::Bytes(b))
    }
}

impl PartialEq for Blob {
    fn eq(&self, other: &Self) -> bool {
        let same = match (&self.body, &other.body) {
            (BlobBody::Bytes(a), BlobBody::Bytes(b)) => a == b,
            (BlobBody::Stream(a), BlobBody::Stream(b)) => a == b,
            (BlobBody::Bytes(_) | BlobBody::Stream(_), _) => false,
        };
        same && self.media == other.media
    }
}

impl Eq for Blob {}

pin_project_lite::pin_project! {
    /// Sends each chunk of a stream as it comes. It also gives the bytes still to come when they were known.
    struct Chunks<S> {
        #[pin]
        chunks: S,
        left: Option<u64>,
    }
}

impl<S> BodySource for Chunks<S>
where
    S: Stream<Item = Result<bytes::Bytes, Error>> + MaybeSend + 'static,
{
    fn poll_piece(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<bytes::Bytes, Error>>> {
        let this = self.project();
        let chunk = ready!(this.chunks.poll_next(cx));
        if let (Some(left), Some(Ok(chunk))) = (this.left.as_mut(), &chunk) {
            *left = left.saturating_sub(chunk.len() as u64);
        }
        Poll::Ready(chunk)
    }

    fn size(&self) -> Option<u64> {
        self.left
    }
}

/// What [`Blob::ranged`] calls with the first and the last byte to send.
type Open = Box<dyn FnOnce(u64, u64) -> Feed<Result<bytes::Bytes, Error>> + Send>;

/// A body that is opened at the bytes it sends. See [`Blob::ranged`].
struct Ranged {
    /// The length of the whole body.
    len: u64,
    first: u64,
    /// The bytes still to come.
    left: u64,
    /// Until the first piece.
    open: Option<Open>,
    chunks: Option<Feed<Result<bytes::Bytes, Error>>>,
}

impl BodySource for Ranged {
    fn poll_piece(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<bytes::Bytes, Error>>> {
        let this = self.get_mut();
        if let Some(open) = this.open.take()
            && this.left > 0
        {
            this.chunks = Some(open(this.first, this.first + this.left - 1));
        }
        let Some(chunks) = this.chunks.as_mut() else { return Poll::Ready(None) };
        let chunk = ready!(chunks.as_mut().poll_next(cx));
        if let Some(Ok(chunk)) = &chunk {
            this.left = this.left.saturating_sub(chunk.len() as u64);
        }
        Poll::Ready(chunk)
    }

    fn size(&self) -> Option<u64> {
        Some(self.left)
    }

    fn seekable(&self) -> Option<u64> {
        self.open.is_some().then_some(self.len)
    }

    fn cut(self: Pin<&mut Self>, first: u64, last: u64) {
        let this = self.get_mut();
        if this.open.is_some() && first <= last && last < this.len {
            (this.first, this.left) = (first, last - first + 1);
        }
    }
}

/// A buffer that other clones of a [`Blob`] still hold. Transports that take a buffer send it as it is. Others send it
/// as one piece.
pub(crate) struct Shared(pub(crate) Option<bytes::Bytes>);

impl BodySource for Shared {
    fn poll_piece(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Result<bytes::Bytes, Error>>> {
        Poll::Ready(self.get_mut().0.take().filter(|b| !b.is_empty()).map(Ok))
    }

    fn shared(&self) -> Option<bytes::Bytes> {
        self.0.clone()
    }
}

/// What an arriving body holds unread and then the rest of it.
pub(crate) struct Leftover(pub(crate) Option<bytes::Bytes>, pub(crate) Option<Pin<Box<dyn BodySource>>>);

impl BodySource for Leftover {
    fn poll_piece(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<bytes::Bytes, Error>>> {
        let Self(read, rest) = self.get_mut();
        if let Some(read) = read.take().filter(|b| !b.is_empty()) {
            return Poll::Ready(Some(Ok(read)));
        }
        rest.as_mut().map_or(Poll::Ready(None), |rest| rest.as_mut().poll_piece(cx))
    }
}

/// The body of a [`Request`](crate::Request) or a [`Response`](crate::Response).
///
/// It is the bytes held whole and then the chunks of a [`ByteStream`] still to come: an upload that a client streams,
/// a request body that the server has not read, or a download that a handler produces. Clones share the stream.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Body {
    bytes: bytes::Bytes,
    stream: ByteStream,
}

impl Body {
    /// Creates a body of these bytes.
    pub fn new(bytes: impl Into<bytes::Bytes>) -> Self {
        Self { bytes: bytes.into(), stream: ByteStream::default() }
    }

    /// Returns a body of the chunks still to come in `stream`.
    pub fn stream(stream: ByteStream) -> Self {
        Self { bytes: bytes::Bytes::new(), stream }
    }

    /// Returns the bytes held whole. A body that streams holds what came before its chunks.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns how many bytes are held whole.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether no bytes are held whole. A body that streams may still have chunks to come.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns the chunks still to come.
    pub fn chunks(&self) -> &ByteStream {
        &self.stream
    }

    /// Does part of the body still have to come?
    pub fn is_streamed(&self) -> bool {
        self.stream.is_some()
    }

    /// Returns the bytes held whole and the chunks still to come.
    pub fn into_parts(self) -> (bytes::Bytes, ByteStream) {
        (self.bytes, self.stream)
    }

    /// Returns the bytes held whole and then the chunks: one stream of the whole body.
    pub fn into_stream(self) -> ByteStream {
        match (self.bytes.is_empty(), self.stream.is_some()) {
            (_, false) => ByteStream::new(Shared(Some(self.bytes))),
            (true, true) => self.stream,
            (false, true) => ByteStream::new(Leftover(Some(self.bytes), self.stream.take_source())),
        }
    }

    /// Returns every byte in one buffer. A body held whole is not copied.
    ///
    /// # Errors
    /// A chunk still to come failed.
    pub async fn collect(self) -> Result<bytes::Bytes, Error> {
        if !self.stream.is_some() {
            return Ok(self.bytes);
        }
        let mut out = Vec::from(self.bytes);
        out.extend(self.stream.collect().await?);
        Ok(out.into())
    }
}

impl From<Vec<u8>> for Body {
    fn from(v: Vec<u8>) -> Self {
        Self::new(v)
    }
}

impl From<bytes::Bytes> for Body {
    fn from(b: bytes::Bytes) -> Self {
        Self::new(b)
    }
}

impl From<String> for Body {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&'static str> for Body {
    fn from(s: &'static str) -> Self {
        Self::new(s)
    }
}

impl From<&'static [u8]> for Body {
    fn from(b: &'static [u8]) -> Self {
        Self::new(b)
    }
}

impl From<ByteStream> for Body {
    fn from(stream: ByteStream) -> Self {
        Self::stream(stream)
    }
}

impl http_body::Body for Body {
    type Data = bytes::Bytes;
    type Error = Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if !this.bytes.is_empty() {
            return Poll::Ready(Some(Ok(http_body::Frame::data(std::mem::take(&mut this.bytes)))));
        }
        Pin::new(&mut this.stream).poll_next(cx).map(|chunk| chunk.map(|c| c.map(http_body::Frame::data)))
    }

    fn is_end_stream(&self) -> bool {
        self.bytes.is_empty() && !self.stream.is_some()
    }

    /// Returns an exact size for a body held whole or streamed from a source that knows its length. Such a body goes
    /// out with `Content-Length` and not in chunks. At least the whole part when more is streamed after it.
    fn size_hint(&self) -> http_body::SizeHint {
        let whole = self.bytes.len() as u64;
        if !self.stream.is_some() {
            return http_body::SizeHint::with_exact(whole);
        }
        match self.stream.size() {
            Some(len) => http_body::SizeHint::with_exact(whole + len),
            None => {
                let mut hint = http_body::SizeHint::new();
                hint.set_lower(whole);
                hint
            }
        }
    }
}

/// One record of a streamed `text/event-stream` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sse<T> {
    /// An item written as an event.
    Item(T),
    /// An item with the `id` and `event` name of the event. A client that reconnects sends the `id` back as
    /// `Last-Event-ID` to resume after it. The fields of the item come after them. So the fields of an event-shaped
    /// item win.
    Event {
        /// The event's `id`: no line break or NUL.
        id: Option<String>,
        /// The event's name: no line break.
        event: Option<String>,
        /// The item written as the other fields of the event.
        item: T,
    },
    /// How long clients wait before they reconnect. It is sent as a `retry` field.
    Retry(std::time::Duration),
    /// A comment line that clients skip. It is a keep-alive. See [`Items::heartbeat`](crate::Items::heartbeat).
    Comment(String),
}
