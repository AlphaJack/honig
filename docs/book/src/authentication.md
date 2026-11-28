# Authentication

Your `Api` has one `verify_` method and one associated type per security scheme that its operations use: `verify_key` and `type Key` for a scheme `key`. The method gets the credential that the request presented for that scheme and answers who is calling as that type, or `None` for a credential that is not valid:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/auth.rs:credentials}}
```

An `Api` without the method does not compile, so no credential reaches an operation unchecked. A method receives the credential as its scheme carries it:

| Scheme | Argument |
|---|---|
| `apiKey`, `http bearer`, `oauth2`, `openIdConnect`, another `http` scheme | the key or token as `&str` |
| `http basic` | `api::Basic { user, password }` |
| `mutualTLS` | none: check what your TLS terminator vouched for from the request |

{{#include generated/principals.md}}

A handler matches the `Security` of its operation. With `security: [{key: [], session: []}]` it gets `Security::KeyAndSession(key, session)`, both values, and checks that they belong to one caller. The catalog has one API key, so this example is a shop whose checkout needs the key of its app and the session of the shopper:

```yaml
{{#include ../../../tests/fixtures/v32/security.yaml:checkout}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_security.rs:checkout}}
```

Every `401` of an operation carries `WWW-Authenticate` with a challenge for its `http` schemes (`Bearer`, `Basic`), also one that your handler returns. An `apiKey` or `mutualTLS` scheme has no HTTP authentication scheme to put in a challenge (RFC 9110 section 11.6.1), so a `401` for one carries none.

`server::credentials(req)` gives one `Credential` variant per security scheme that the operation matched. A scheme added to the description is then a compile error until you handle it.

## Request context

Values that outer tower layers attach also reach the `verify_` methods. So does the address of the client:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/auth.rs:request-context}}
```

[Recipes](recipes.md#claims-from-an-auth-layer-you-already-run) maps the claims of a JWT or session layer that you already run to who is calling.

## Signatures over the body

Some schemes sign the body, such as webhooks that carry a signature of the body in a header. The layer that checks the signature collects the body with `honig::read_body(&mut req).await`. The operation decodes the same bytes and collects nothing again:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:signed-body}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/layers.rs:read-body}}
```

A layer that does not call `read_body` runs before the body is consumed. So a refused request never consumes its body. A client signs in its transport: the request is final there, body included. `reqwest-middleware` and tower layers work too.

## Scopes and roles

A scheme with scopes or roles has an enum of its own: `server::PatScope` for a scheme `pat`. Its `verify_` method returns `api::Granted { value, scopes }`, the scopes that the credential grants. The server checks them against the operation's requirement and answers `403` when one is missing: `WWW-Authenticate: Bearer error="insufficient_scope", scope="orders:write"` names the scopes that the operation requires, and the message names the missing ones. One enum per scheme keeps a token of one scheme from granting a scope of another. The catalog's API key grants no scopes, so this example is a shop whose orders need a token with `orders:read` or `orders:write`:

```yaml
{{#include ../../../tests/fixtures/v32/scopes.yaml:scopes}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_scopes.rs:scopes}}
```

## Client certificates

A `mutualTLS` scheme is satisfied only when its `verify_` method answers `Some`. It checks what your TLS terminator vouched for, such as a header that it sets or an extension that a layer outside the server adds.
