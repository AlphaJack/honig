# Recipes

Each recipe is a whole setup that combines the pieces of the other pages. Each one is a test, so it matches the code.

## API server and client

The `examples/shop` crate runs a server and a client for a small catalog of products against each other. Every example in this book is a test that runs on each change. So each example matches the code.

Its `src/api` is what `cargo honig generate` writes. Its `src/supplier` is a second module, a client for a book supplier: see [Several APIs in one crate](settings.md#several-apis-in-one-crate).

The description is `openapi.yaml`:

```yaml
{{#include ../../../examples/shop/openapi.yaml:all}}
```

### Server

The server implements the generated `Api` trait, which has one method per operation:

```rust
{{#include ../../../examples/shop/src/lib.rs:server}}
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

A JWT or session layer that you already run puts its claims in the request's extensions. The [`verify_` method](authentication.md) of the scheme maps them to the value of its scheme:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/recipes.rs:claims}}
```

## Optional sign-in with a limit for anonymous callers

An operation whose `security` contains an empty alternative, `[{}, {bearer: []}]`, accepts anonymous requests and signed-in ones. A public endpoint that limits anonymous callers and not signed-in ones is an `operation_layer`. It runs after the server checked the credentials, so `api::Passed` is in the request's extensions when the token passed. A token that the `Api` refuses is `401`, so a bad token never falls back to the anonymous limit. Every operation of the catalog needs its key, so this recipe is a shop whose deals anyone may browse:

```yaml
{{#include ../../../tests/fixtures/v32/security.yaml:deals}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_security.rs:optional-rate-limit}}
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

## Storing API values

Keep the types of your database plain and convert at the handler. A generated type such as `PhysicalSku` checks what the description says about the wire. A column holds what you stored, maybe under an older version of the description. So the API decides at one place what it serves, and a pattern tightened for new clients does not break every read of the old rows. Your database crate also stays free of honig: it does not depend on the crate of the generated module, which depends on it to run queries.

The catalog's `Physical` has a SKU of a pattern:

```yaml
{{#include ../../../examples/shop/openapi.yaml:physical}}
```

A row stores it as text. The API type takes the checked SKU, so the conversion cannot be left out:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/recipes.rs:storage}}
```

To make the database itself refuse a malformed SKU, for every service that writes to it, add a constraint in a migration:

```sql
ALTER TABLE products ADD CONSTRAINT sku_format CHECK (sku ~ '^[A-Z]{3}-[0-9]{4}$');
```
