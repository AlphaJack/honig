# Authentication

`Api::Principal` is who is calling. Your `Api` has one `verify_` method per security scheme that its operations use. It gets the credential that the request presented for that scheme and answers who is calling, or `None` for a credential that is not valid:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/auth.rs:credentials}}
```

An `Api` without the method does not compile, so no credential reaches an operation unchecked. A method takes the credential as its scheme carries it:

| Scheme | Argument |
|---|---|
| `apiKey`, `http bearer`, `oauth2`, `openIdConnect`, another `http` scheme | the key or token as `&str` |
| `http basic` | `api::Basic { user, password }` |
| `mutualTLS` | none: read what your TLS terminator vouched for from the request |

{{#include generated/principals.md}}

Every `401` of an operation carries `WWW-Authenticate` with a challenge for its `http` schemes (`Bearer`, `Basic`), also one that your handler returns. An `apiKey` or `mutualTLS` scheme has no HTTP authentication scheme to name in a challenge (RFC 9110 section 11.6.1), so a `401` for one carries none.

`server::credentials(req)` gives one `Credential` variant per security scheme that the operation matched. A scheme added to the description is then a compile error until you handle it.

## Request context

Values that outer tower layers attach also reach the `verify_` methods. So does the address of the client:

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

A scheme with scopes or roles has an enum of its own: `server::PatScope` for a scheme `pat`. Its `verify_` method returns `api::Granted { principal, scopes }`, the scopes that the credential grants. The server checks them against the operation's requirement and answers `403` with `WWW-Authenticate: Bearer error="insufficient_scope"` when one is missing. One enum per scheme keeps a token of one scheme from granting a scope of another:

```rust
{{#include ../../../tests/e2e/tests/v32_scopes.rs:scopes}}
```

## Client certificates

A `mutualTLS` scheme is satisfied only when its `verify_` method answers `Some`. It reads what your TLS terminator vouched for, such as a header that it sets or an extension that a layer outside the server adds.
