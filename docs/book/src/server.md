# Server

A server is your type that implements the generated `server::Api`. It has one method per operation:

```rust
{{#include ../../../examples/shop/src/lib.rs:catalog}}
```

`api::Server::new(Catalog::default())` builds the server. `into_axum()` serves it as an axum `Router`. `into_service()` serves it as a tower `Service` for hyper and other servers. See [Adapters](adapters.md).

The server keeps your `Api` in an `Arc` and shares it across requests without cloning it. To share the same state with code outside the server, such as a websocket route or a background job, pass an `Arc` you keep a clone of: `api::Server::new(Arc::clone(&catalog))`.

{{#include generated/server-steps.md}}

## Validation

Requests are checked against every constraint of their schemas. A value that its [checked type](types.md#checked-types) refuses stops decoding: the request gets `400` with that violation and its JSON pointer. The other constraints are checked after decoding, and the `400` lists every one that fails:

```json
{"violations": [{"path": "/body/name", "message": "at least 2 characters required, found 1"}]}
```

A body whose `Content-Type` the operation does not declare gets `415`.

## Content negotiation

An operation may answer in several media types. It answers in the one that the `Accept` of the request prefers (RFC 9110 section 12.5.1). If `Accept` takes none of them then it answers `406`. When every media type holds the same type the handler returns that type. The server then picks the encoding. When they hold different types `Params` carries the choice as a typed `media`.

## Streams

Binary responses take `api::Blob`. Return bytes. Or pass any `Stream` of chunks to `Blob::stream` for a large download:

```rust
{{#include ../../../tests/e2e/tests/v32_download.rs:download}}
```

A response declared as a range, such as `image/*` or `*/*`, has no one type to send. Give the blob its type with `Blob::with_media`. It goes out as that type when it fits the range, else as `application/octet-stream`. The client reads it back with `Blob::media`:

```rust
{{#include ../../../tests/e2e/tests/v32_download.rs:media-range}}
```

Binary request bodies reach the handler as they arrive. A `Blob` is a `Stream` of its chunks. So the server never holds a whole upload. A body past `max_body_bytes` fails its chunk. Then `?` answers `413`:

```rust
{{#include ../../../tests/e2e/tests/v32_uploads.rs:upload}}
```

Sequential responses take `api::Items<T>` in the same way. These are JSON Lines and JSON text sequences and server-sent events. Return `Items::List(vec)`. Or pass any `Stream` of items to `Items::stream`. A sequential request body is a `Stream` of its items too. Each item is checked as it is read, and the `minItems`, `maxItems` and `uniqueItems` of the sequence hold as it arrives: the item past `maxItems` and an item met before are an `Err`, and so is an end short of `minItems`. Items that your code produces are not checked.

Server-sent events can also take `Items::events`. Its records carry the `id` and name of an event. They also carry a `retry` and comments. A client that reconnects sends the last `id` back as `Last-Event-ID`:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:feed}}
```

Answer `204` to a client that should stop reconnecting.

## Conditional requests

Some `GET` and `HEAD` requests get `304 Not Modified` (RFC 9110 section 13). This happens when the success response carries an `ETag` or a `Last-Modified`. The `If-None-Match` or `If-Modified-Since` of the request must also say that the client already holds it. The 304 keeps the validators and caching headers and drops the body. Your handler still runs.

Before a handler changes a resource `ctx.preconditions` checks `If-Match`, `If-Unmodified-Since` and `If-None-Match` against its current validators. It refuses with `412`:

```rust
{{#include ../../../tests/e2e/tests/v32_download.rs:preconditions}}
```

Pass `None` for a resource that does not exist yet. `If-None-Match: *` then lets a create through. `If-Match: *` refuses it.

## Range requests

{{#include generated/ranges.md}}

## Errors

The error of a handler can be the error schema of the description. honig generates `From<types::Problem> for api::ApiError` for it. Errors that the server raises itself go through `error_response`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:error-response}}
```

{{#include generated/error-rendering.md}}

The fields of an error schema that the server fills:

{{#include generated/error-fields.md}}

## Server options

The options of `honig::Server`, from its rustdoc:

{{#include generated/server-options.md}}
