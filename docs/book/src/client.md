# Client

Each operation of the description is a method of the generated `Client`. It accepts a `Params` with one field per parameter and returns one variant per documented response:

```rust
{{#include ../../../examples/shop/tests/client.rs:client}}
```

`Params::new` accepts the path parameters in URL order. Then it accepts the other required parameters and then a required body. The URL fixes the argument order. So a reordered description cannot swap two arguments silently. With no arguments `Params::default()` is the same value.

A method accepts its `Params` or a reference to them. So `&params` keeps them for a retry. JSON, XML, form and multipart bodies are encoded from the reference with no copy.

Each security scheme gets its own setter with its name, such as `with_api_key` or `with_bearer_auth`. A misspelt scheme is a compile error.

## Response media type

An operation whose success responses come in several media types has a `media` in its `Params`. The client prefers it in `Accept` and lists the others after it at `q=0.9`. A server whose answer depends on the resource then sends one the client decodes and not `406`:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:media}}
```

## Links and callbacks

Response `links` become typed structs that fill the parameters of the linked operation:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/links.rs:links}}
```

The URL of a callback comes from the request that declares it:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:callback}}
```

## OAuth2

A client credentials flow fetches its token with `authorize_<scheme>`. A device authorization flow (OpenAPI 3.2) shows a code to the user and then polls:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:device-flow}}
```

Other flows get helpers by what the scheme declares:

| Flow | Methods |
|---|---|
| Authorization code (PKCE) | `Client::authorization_url_<s>(..., &pkce)` and then `exchange_code_<s>(code, ..., &pkce)` |
| Password | `authorize_password_<s>(user, password, client_id, secret, scopes)` |
| Implicit | `Client::implicit_url_<s>(...)` |
| Refresh | `refresh_<s>(refresh_token, client_id, secret)` |
| OpenID Connect and RFC 8414 | `discover_<s>()` returns the token, authorization and device endpoints |

## Streams

A response declared as a sequence also gets `<op>_stream`. The sequence is JSON Lines or JSON text sequences or server-sent events. `<op>_stream` gives a `Stream` that yields each item as it arrives. Consume it with `futures_util::StreamExt::next`. A binary response gets the `Response` with its body still to come: `resp.into_body().into_stream()` yields its raw chunks. An item that does not decode is an `Err`. The items after it can still arrive. The `minItems`, `maxItems` and `uniqueItems` of the sequence apply as it arrives: the item past `maxItems` and an item met before are an `Err`, and so is an end short of `minItems`:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:stream}}
```

The plain method receives the whole body before it decodes it. On a feed that never ends it waits for ever.

### Reconnecting

{{#include generated/reconnect.md}}

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:reconnect}}
```

## Conditional requests

A client sends a conditional request when the operation declares an `If-None-Match` header parameter. It can then ask for a product only if its copy is stale. When the description declares no `304` for the operation a `304` is `Err(api::ClientError::NotModified)`. It has the headers of the response:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:not-modified}}
```

## Retries and caching

Retries, timeouts and caching come from a `reqwest-middleware` stack. `ReqwestMiddlewareTransport` sends through it. This one caches with `http-cache-reqwest` and retries with `reqwest-retry`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/adapters.rs:retry}}
```

Each piece of that stack does one job and has limits:

| Piece | What it does | Limits |
|---|---|---|
| `RetryTransientMiddleware` | It retries a lost connection, a 5xx, a 408 or a 429 with exponential backoff. | It ignores `Retry-After`: the backoff alone sets the wait. It does not tell methods apart, so on its own it sends a failed `POST` again. |
| `IdempotentOnly` | It lets only `GET`, `HEAD`, `OPTIONS`, `PUT` and `DELETE` through to the retry. | None. |
| `Cache` | It keeps responses as RFC 9111 allows and revalidates a stale one with `If-None-Match` or `If-Modified-Since`. A `304` comes back as the cached `200`. | Only its 1.0 alpha builds on reqwest 0.13. |

A streamed upload cannot be sent twice. So retry middleware does not retry it.

`ureq` and the `GlooTransport` of the browser have no middleware stack. They are tower services, so tower layers wrap them. See [your own transport](adapters.md#your-own-transport).

## Errors

A call returns `Err(api::ClientError)` when it gets no response the description declares:

{{#include generated/errors.md}}

Every other variant that received a response carries its raw body.

A declared response that is not the success is still `Ok` from the call: the description declares it. `into_result` turns the response into the success and anything else into an `Err` that `?` converts into any error type:

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:into-ok}}
```

One success gives what it carries: nothing, its body, or its body and headers in order as `Success`. Several successes, a `2XX` range, or two values of one type (`ETag` and `Last-Modified`, which a reorder would swap) give the response itself. The `Err` is the whole response, boxed, so its body stays typed.

## Browser (WebAssembly)

The client compiles for `wasm32-unknown-unknown` and sends through the browser's `fetch`:

1. Depend on `gloo-net` with its `http` feature under `[target.'cfg(target_arch = "wasm32")'.dependencies]`. Native builds then skip it, as the adapter compiles on wasm32 only.
2. Turn on the `gloo-net` feature of `honig` and set `server = false`.
3. Build the client on `api::GlooTransport::new("/api")?`.

Futures on wasm32 are not `Send`. A transport service of your own writes `+ api::MaybeSend` instead of `+ Send` on its futures, so it compiles for both targets.
