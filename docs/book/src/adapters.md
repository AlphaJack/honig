# Adapters

The cargo features of the `honig` dependency are yours to list. Some are needed: the generated code calls them. `generate` and `check` stop and print the `cargo add honig -F ...` line when one is off. The others are optional: an adapter, the types of string formats or `simd`. The table says when the generated code needs each one.

{{#include generated/features.md}}

An adapter is compiled whenever its feature is on. Depend on the major version of its library that the table lists. Otherwise cargo builds two copies whose types do not match.

## axum

`into_axum()` returns a `Router` with a route per path template. It merges next to your own routes or nests under a prefix:

```rust
{{#include ../../../examples/shop/src/main.rs:server}}
```

A path that the description does not declare falls through to the fallback of the router: a bare `404` with no body and no wide event. To answer it in the error schema with a wide event, give the router a fallback that is the server:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/observability.rs:axum-fallback}}
```

axum refuses to merge two routers that both have a fallback. Set it once, on the router that you serve.

[Recipes](recipes.md) serves a web app next to the API and a WebSocket next to the generated routes.

## hyper and other servers

`into_service()` is a tower `Service` over `http` requests:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/adapters.rs:hyper}}
```

## Blocking client

`UreqTransport` sends with ureq. `block_on` runs the call with no async runtime:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/adapters.rs:ureq}}
```

## Testing without a socket

A server's `into_service()` is a transport too. The client then sends each request straight to the server in memory:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/adapters.rs:loopback}}
```

## Your own transport

A transport is any tower `Service` of `http` requests. Write one to send with any HTTP client. Wrap one with tower's layers:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/adapters.rs:transport}}
```

## TLS

TLS comes from your own dependency. `honig` depends on `reqwest` and `ureq` without default features. Cargo adds the features that your crate turns on. The default is rustls. If your crate turns them off then add `rustls` or `native-tls`. Otherwise every `https` request fails.
