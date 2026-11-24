<!-- Generated from honig/src/client/reconnect.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Reconnects the stream through `transport` when it ends or breaks off, as the `EventSource` of a browser
does.

It keeps a clone of `transport`: wrap a transport that is not `Clone` in an `Arc`. It waits
`ItemStream::retry` (3 seconds until the server sends a `retry`). Then it sends `req` again with
`Last-Event-ID`, so the server can resume after the last event received. An event that the connection broke
inside is dropped. So no event is received twice.

A failed reconnection is an `Err` and the next poll tries again. Each time it waits twice as long, up to a
minute. A `429` or `5xx` counts as failed. The stream ends on a `204` or another status, on a response that is
not `text/event-stream` and after `ItemStream::max_retries` failed reconnections in a row.
`ItemStream::no_reconnect` ends it with the body. Other framings and a `req` of `None` end where the body
ends: a streamed request body cannot be sent twice.
