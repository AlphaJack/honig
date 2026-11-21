//! Reconnecting an event stream through the transport of the client as the WHATWG `EventSource` does.

use serde::de::DeserializeOwned;

use crate::stream::{Reconnect, Reconnected};
use crate::{ClientError, ItemStream, Message, Request, Transport, stream::Connecting};

/// Sends `req` again through `transport` and expects the `status` of the stream.
struct Resend<Tr> {
    transport: Tr,
    req: Request,
    status: Option<u16>,
}

impl<Tr: Transport> Reconnect for Resend<Tr> {
    fn connect(&mut self, last_event_id: &str) -> Connecting {
        let mut req = crate::message::copy_request(&self.req);
        let headers = req.headers_mut();
        headers.insert(http::header::ACCEPT, http::HeaderValue::from_static("text/event-stream"));
        headers.insert(http::header::CACHE_CONTROL, http::HeaderValue::from_static("no-cache"));
        headers.remove("last-event-id");
        let id = (!last_event_id.is_empty()).then(|| http::HeaderValue::from_str(last_event_id));
        let (transport, status) = (self.transport.clone(), self.status);
        Box::pin(async move {
            match id {
                Some(Ok(id)) => {
                    req.headers_mut().insert("last-event-id", id);
                }
                Some(Err(e)) => return Reconnected::Fatal(ClientError::Encode(format!("Last-Event-ID: {e}"))),
                None => {}
            }
            let resp = match crate::send_streaming(&transport, req).await {
                Ok(resp) => resp,
                Err(e) => return Reconnected::Retry(e),
            };
            let s = resp.status().as_u16();
            let media = crate::media::check_response_media(s, resp.content_type(), None, &["text/event-stream"]);
            match (s, media) {
                (s, _) if s == http::StatusCode::NO_CONTENT => Reconnected::Done,
                (s, Ok(())) if Some(s) == status => Reconnected::Open(resp.into_body().into_stream()),
                (s, Err(e)) if Some(s) == status => Reconnected::Fatal(e),
                (s, _) => {
                    let status = resp.status();
                    let body = resp.into_body().collect().await.unwrap_or_default();
                    let e = ClientError::UnexpectedStatus { status, body };
                    let busy = s == http::StatusCode::TOO_MANY_REQUESTS
                        || http::StatusCode::from_u16(s).is_ok_and(|s| s.is_server_error());
                    if busy { Reconnected::Retry(e) } else { Reconnected::Fatal(e) }
                }
            }
        })
    }
}

impl<T: DeserializeOwned> ItemStream<T> {
    /// Reconnects the stream through `transport` when it ends or breaks off, as the `EventSource` of a browser
    /// does.
    ///
    /// It keeps a clone of `transport`: wrap a transport that is not `Clone` in an `Arc`. It waits
    /// [`ItemStream::retry`] (3 seconds until the server sends a `retry`). Then it sends `req` again with
    /// `Last-Event-ID`, so the server can resume after the last event read. An event that the connection broke inside
    /// is dropped. So no event is read twice.
    ///
    /// A failed reconnection is an `Err` and the next poll tries again. Each time it waits twice as long, up to a
    /// minute. A `429` or `5xx` counts as failed. The stream ends on a `204` or another status, on a response that is
    /// not `text/event-stream` and after [`ItemStream::max_retries`] failed reconnections in a row.
    /// [`ItemStream::no_reconnect`] ends it with the body. Other framings and a `req` of `None` end where the body
    /// ends: a streamed request body cannot be sent twice.
    #[must_use]
    pub fn reconnecting<Tr: Transport>(self, transport: Tr, req: Option<Request>) -> Self {
        let Some(req) = req else { return self };
        let status = self.status();
        let id = req.header("Last-Event-ID").map(std::borrow::Cow::into_owned);
        self.set_reconnect(Box::new(Resend { transport, req, status }), id.as_deref());
        self
    }
}
