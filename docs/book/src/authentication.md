# Authentication

`Api::Principal` is who is calling. `authenticate` finds the principal from the credentials that the request presented:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/auth.rs:credentials}}
```

`find` answers `None` for nobody. When your `Api` declares a `Principal`, a server without `authenticate` does not compile. Layers added after `authenticate` read the principal from the request's extensions:

{{#include generated/principals.md}}

Every `401` of an operation carries `WWW-Authenticate` with a challenge for its `http` schemes (`Bearer`, `Basic`), also one that your handler returns. An `apiKey` or `mutualTLS` scheme has no HTTP authentication scheme to name in a challenge (RFC 9110 section 11.6.1), so a `401` for one carries none.

`server::credentials(req)` gives one `Credential` variant per security scheme that the operation matched. A scheme added to the description is then a compile error until you handle it.

## Request context

Values that outer tower layers attach also reach `authenticate`. So does the address of the client:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/auth.rs:request-context}}
```

[Recipes](recipes.md#claims-from-an-auth-layer-you-already-run) maps the claims of a JWT or session layer that you already run to the principal.

## Signatures over the body

Some schemes sign the body, such as webhooks that carry a signature of the body in a header. The layer that checks the signature reads the body with `api::read_body(&mut req).await`. The operation decodes the same bytes and reads nothing again:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:signed-body}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:read-body}}
```

A layer that does not call `read_body` runs before the body is read. So a refused request costs no body read. A client signs in its transport: the request is final there, body included. `reqwest-middleware` and tower layers work too.

## Scopes and roles

Scopes and roles are typed. There is one `server::Scope` variant per name in the description. Match them exhaustively. A permission added to the description is then a compile error until you map it:

```rust
{{#include ../../../tests/e2e/tests/v32_scopes.rs:scopes}}
```

## Client certificates

A `mutualTLS` alternative is never satisfied on its own. A layer that trusts the certificate your TLS terminator verified adds `api::ClientCertificateVerified` to the request's extensions.
