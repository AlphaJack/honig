<!-- Generated from runtime/src/server/service.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

A server for the operations `O` with the tower layers `L` between routing and each operation. `A` is whether it
finds who is calling (`Authentication`).

The generated `api::Server::new(api)` starts one. Layers run in the order that they are added. The first one is the
outermost as with the `ServiceBuilder` of tower. Each one is a tower `Service` of `http::Request<Body>`. It
answers with a `Response` or with an error that converts into an `ApiError`. That error goes out in the error
schema that the description declares. Generic tower layers (`Timeout`, `LoadShed` and `ConcurrencyLimit`) fit
too. Their `BoxError` becomes a `503`.

### `max_body_bytes`

Sets the largest request body in bytes that the server reads. Larger ones get `413`. Default: 16 MiB.

The limit is a setting of the server and not a layer outside it, such as `RequestBodyLimitLayer` of
tower-http:

- The server reads the body itself to decode it. It holds the body in memory. So it needs a limit even with
  no layer around it.
- A streamed `Blob` fails the chunk that goes past the limit. The handler gets that error, and `?`
  answers `413` in the error schema of the description.
- `read_body` in a layer reads up to the same limit. A layer that checks a signature over the body and the
  operation that decodes it agree on one number.

A layer outside can only lower the limit. It refuses a request early when its `Content-Length` is too large.
Its answer then has its own body: map it through `Errors::response`. A body without `Content-Length` fails
while the server reads it. The server sees a broken body and answers `400`, not `413`.

### `error_response`

Sets the response for a request that the server answers without the response of your handler. The cause is no
route, bad input, missing credentials, the `Err` of a layer, validation, a handler `Err` or an encoding failure.
Default: `Rejection::response`.

### `on_event`

Sets the sink that receives the wide event (`RequestEvent`) of each request once the response is ready. By
default the event is logged with `tracing` when the server lists the `tracing` adapter, and dropped otherwise.

### `errors`

Returns the error responses of the description for a layer outside the server. Such a layer is a timeout, a body
limit or an IP flood guard. It then answers in the same error schema as the operations.
