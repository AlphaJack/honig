# Recipes

Each recipe is a whole setup that combines the pieces of the other pages. Each one is a test, so it matches the code.

## Web server and CLI client

The `examples/shop` crate runs a server and a client for a small catalog of products against each other. Every example in this book is a test that runs on each change. So each example matches the code.

Its `src/api` is what `cargo honig generate` writes. Its `src/supplier` is a second module, a client for a book supplier: see [Several APIs in one crate](settings.md#several-apis-in-one-crate).

The description is `openapi.yaml`:

```yaml
{{#include ../../../examples/shop/openapi.yaml}}
```

### Server

The server implements the generated `Api` trait, which has one method per operation:

```rust
{{#include ../../../examples/shop/src/lib.rs:catalog}}
```

Serve it with axum in `src/main.rs`:

```rust
{{#include ../../../examples/shop/src/main.rs:server}}
```

### Client

The client has one method per operation. Each returns one variant per documented response:

```rust
{{#include ../../../examples/shop/tests/client.rs:client}}
```

## API server and SPA

A single-page app and its API share one port. Unknown paths under the API's base path still get the API's `404` in its error schema, so a client never receives the app's HTML by mistake. [rust-embed](https://crates.io/crates/rust-embed) compiles the app's folder into the binary, so the server ships as one file:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/recipes.rs:spa}}
```

## Claims from an auth layer you already run

A JWT or session layer that you already run puts its claims in the request's extensions. The [`verify_` method](authentication.md) of the scheme maps them to the principal of your `Api`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/recipes.rs:claims}}
```

## Errors of layers outside the server

A layer outside the server answers in its own format. Its answers go out in the error schema when they pass through `errors()`. [Layers](layers.md#outside) shows a fallback that does this. A layer that fails with an error, such as a timeout or a full queue, goes through axum's `HandleErrorLayer`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/recipes.rs:outer-errors}}
```

## Middlewares

Layers outside the server and layers inside it, in the order that a request passes them:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/recipes.rs:stack}}
```

## WebSockets

OpenAPI cannot describe a WebSocket. So honig generates nothing for one. Serve it as a plain axum route next to the generated routes. Share state through your own `Arc`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/websocket.rs:websocket}}
```
