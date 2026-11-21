# Layers

Two places accept tower layers. The rustdoc of `Server::operation_layer` compares them:

{{#include generated/operation-layer.md}}

## Inside

Layers inside the server answer in the error schema. Timeouts, load shedding and concurrency limits answer `503`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:inside}}
```

## Rate limits

OpenAPI cannot express a rate limit. Enforce one with tower's `AsyncFilterLayer`. It runs before the body is consumed. The request's extensions contain the operation and the principal. An `ApiError` it returns goes out in the error schema:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:rate-limit}}
```

`req.extensions().get::<api::Operation>()` is the generated `api::Operation`: one variant per operation. Match on it to treat operations apart. `Rejection::operation` of `error_response`, `Context::operation` of a handler and the `operation` of the wide event are the same enum:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:per-operation}}
```

## A layer for any API

A plain tower `Layer` fits any honig server. Its service accepts `honig::Request` and identifies the operation by the `MatchedOperation` in the extensions:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:any-api}}
```

## Outside

tower-http layers go outside the server. A layer outside that answers on its own can render the description's error schema with `errors()`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:outside}}
```

[Recipes](recipes.md#a-full-stack) puts layers outside and inside the server in one stack. It also maps the errors of outside layers that fail with a timeout or a full queue.
