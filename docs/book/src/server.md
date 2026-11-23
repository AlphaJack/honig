# Server

A server is your type that implements the generated `server::Api`. It has one method per operation:

```rust
{{#include ../../../examples/shop/src/lib.rs:catalog}}
```

`api::Server::new(Catalog::default())` builds the server. `into_axum()` serves it as an axum `Router`. `into_service()` serves it as a tower `Service` for hyper and other servers. See [Adapters](adapters.md).

A handler answers with a variant of the operation's `Response`. A `2XX` range or a `default` response carries a `status` of its own enum, such as `DefaultStatus::Conflict`, with the registered codes that no other response of the operation declares. A status that a client would decode as another variant does not compile.

The server keeps your `Api` in an `Arc` and shares it across requests without cloning it. To share the same state with code outside the server, such as a websocket route or a background job, pass an `Arc` you keep a clone of: `api::Server::new(Arc::clone(&catalog))`.

{{#include generated/server-steps.md}}

## Validation

Requests are checked against every constraint of their schemas while they decode: each value is a [checked type](types.md#checked-types) or a type that enforces its keywords. The first value that breaks one stops decoding. The request gets `400` with that violation and its JSON pointer. Your handler only gets valid parameters:

```json
{"title": "Bad Request", "status": 400, "detail": "/body/name: at least 2 characters required, found 1",
 "errors": [{"detail": "at least 2 characters required, found 1", "pointer": "#/name"}]}
```

That is RFC 9457 `problem+json`. `errors` is the extension of the RFC's own validation example: each entry has a
`detail` and a JSON `pointer`, into the request content for a value of the body and into the request otherwise
(`#/query/limit`).

A body whose `Content-Type` the operation does not declare gets `415`.

## Content negotiation

An operation may answer in several media types. It answers in the one that the `Accept` of the request prefers (RFC 9110 section 12.5.1). If `Accept` matches none of them then it answers `406`. When every media type carries the same type the handler returns that type. The server then picks the encoding. When they carry different types `Params` carries the choice as a typed `media`.

## Streams

Binary responses use `api::Blob`. Return bytes. Or pass any `Stream` of chunks to `Blob::stream` for a large download:

```rust
{{#include ../../../tests/e2e/tests/v32_download.rs:download}}
```

A response declared as a range, such as `image/*` or `*/*`, has no one type to send. Give the blob its type with `Blob::with_media`. It goes out as that type when it fits the range, else as `application/octet-stream`. The client gets it back with `Blob::media`:

```rust
{{#include ../../../tests/e2e/tests/v32_download.rs:media-range}}
```

Binary request bodies reach the handler as they arrive. A `Blob` is a `Stream` of its chunks. So the server never buffers a whole upload. A body past `max_body_bytes` fails its chunk. Then `?` answers `413`:

```rust
{{#include ../../../tests/e2e/tests/v32_uploads.rs:upload}}
```

Sequential responses use `api::Items<T>` in the same way. These are JSON Lines and JSON text sequences and server-sent events. Return `Items::List(vec)`. Or pass any `Stream` of items to `Items::stream`. A sequential request body is a `Stream` of its items too. Each item is checked as it is decoded, and the `minItems`, `maxItems` and `uniqueItems` of the sequence apply as it arrives: the item past `maxItems` and an item met before are an `Err`, and so is an end short of `minItems`. Items that your code produces are not checked.

Server-sent events can also use `Items::events`. Its records carry the `id` and name of an event. They also carry a `retry` and comments. A client that reconnects sends the last `id` back as `Last-Event-ID`:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:feed}}
```

Answer `204` to a client that should stop reconnecting.

## Conditional requests

Some `GET` and `HEAD` requests get `304 Not Modified` (RFC 9110 section 13). This happens when the success response carries an `ETag` or a `Last-Modified`. The `If-None-Match` or `If-Modified-Since` of the request must also say that the client already has it. The 304 keeps the validators and caching headers and drops the body. Your handler still runs.

Before a handler changes a resource `ctx.preconditions` checks `If-Match`, `If-Unmodified-Since` and `If-None-Match` against its current validators. It refuses with `412`:

```rust
{{#include ../../../tests/e2e/tests/v32_download.rs:preconditions}}
```

Pass `None` for a resource that does not exist yet. `If-None-Match: *` then lets a create through. `If-Match: *` refuses it.

## Range requests

{{#include generated/ranges.md}}

## Errors

A request that the server refuses itself (no route, bad input, missing credentials, a layer's or a handler's `Err`)
goes through `error_response` of your `Api`. honig fills an error body only where a standard defines its members:

- RFC 9457 `application/problem+json`: `type`, `title`, `status` and `detail` by their names in the RFC, and a required
  `const` at its value. Any other required member has no value that the server knows.
- A text body: the detail and nothing else.

Where every error body of the description is one of these, or there is none, `error_response` has a default and you
write nothing: refusals go out as RFC 9457 `problem+json`. Any other error body, such as `{code, message}`, names
fields that only your API defines. Your `Api` writes `error_response` to fill them, and the crate does not compile
without it:

```rust
{{#include ../../../examples/shop/src/lib.rs:error_response}}
```

A handler that answers in such a body sets it on its `ApiError` with `with_body`, with the status said once. One
server can still replace `error_response` with its own, for example to record each refusal:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:error-response}}
```

`rejection.problem()` returns the default body as a typed RFC 9457 `Problem`. Set a member and send it with
`into_response()`, for example `instance` from the request ID of the client:

```rust
{{#include ../../../tests/e2e/tests/v32_scopes.rs:problem-instance}}
```

{{#include generated/error-rendering.md}}

The members of an error body that the server fills:

{{#include generated/error-fields.md}}

## Server options

The options of `honig::Server`, from its rustdoc:

{{#include generated/server-options.md}}
