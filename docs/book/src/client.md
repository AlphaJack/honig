# Client

Each operation of the description is a method of the generated `Client`. It accepts a `Params` with one field per parameter and returns one variant per documented response. The examples of this page call the catalog of the [shop](recipes.md#web-server-and-cli-client): each section shows the part of its `openapi.yaml` that it relies on.

```rust
{{#include ../../../examples/shop/tests/client.rs:client}}
```

`Params::new` accepts the path parameters in URL order. Then it accepts the other required parameters and then a required body. It exists only when no two of them have one type, an alias counting as the type that it names. So a reordered description cannot swap two arguments silently: a swap is a type error, or there is no `new` and the builder names each value. With no arguments `Params::default()` is the same value.

A method accepts its `Params` or a reference to them. So `&params` keeps them for a retry. JSON, XML, form and multipart bodies are encoded from the reference with no copy.

Each security scheme gets its own setter with its name, such as `with_api_key` or `with_bearer_auth`. A misspelt scheme is a compile error.

## Response media type

An operation whose success responses come in several media types has a `media` in its `Params`. The client prefers it in `Accept` and lists the others after it at `q=0.9`. A server whose answer depends on the resource then sends one the client decodes and not `406`. The catalog exports its products as JSON or CSV:

```yaml
{{#include ../../../examples/shop/openapi.yaml:media}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/client.rs:media}}
```

## Links and callbacks

Response `links` become typed structs that fill the parameters of the linked operation. A product links to its deletion:

```yaml
{{#include ../../../examples/shop/openapi.yaml:links}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/client.rs:links}}
```

The URL of a callback comes from the request that declares it. Adding a product notifies the URL that the request names:

```yaml
{{#include ../../../examples/shop/openapi.yaml:callback}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/client.rs:callback}}
```

## OAuth2

A client credentials flow fetches its token with `authorize_<scheme>`. A device authorization flow (OpenAPI 3.2) shows a code to the user and then polls. The catalog accepts only an API key, so this example calls another shop, whose `oAuth2` scheme declares:

```yaml
{{#include ../../../tests/fixtures/v32/shop.yaml:device-flow}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_shop/client.rs:device-flow}}
```

Other flows get helpers by what the scheme declares:

| Flow | Methods |
|---|---|
| Authorization code (PKCE) | `Client::authorization_url_<s>(client_id, ..., &pkce)` and then `exchange_code_<s>(client_id, secret, code, redirect_uri, &pkce)` |
| Password | `authorize_password_<s>(client_id, secret, user, password, scopes)` |
| Implicit | `Client::implicit_url_<s>(client_id, ...)` |
| Refresh | `refresh_<s>(client_id, secret, refresh_token)` |
| OpenID Connect and RFC 8414 | `discover_<s>()` returns the token, authorization and device endpoints |

Every helper takes the client id first and its secret second, so the two stay at one place whatever the flow. A helper of a scheme with scopes takes them as its scope enum, such as `&[api::operations::PatScope::Read]`: a scope that the description does not list does not compile.

## Streams

A response declared as a sequence also gets `<op>_stream`. The sequence is JSON Lines or JSON text sequences or server-sent events. `<op>_stream` gives a `Stream` that yields each item as it arrives. Consume it with `futures_util::StreamExt::next`. A binary response gets the `Response` with its body still to come: `resp.into_body().into_stream()` yields its raw chunks. An item that does not decode is an `Err`. The items after it can still arrive. The `minItems`, `maxItems` and `uniqueItems` of the sequence apply as it arrives: the item past `maxItems` and an item met before are an `Err`, and so is an end short of `minItems`:

The catalog sends each change as a server-sent event:

```yaml
{{#include ../../../examples/shop/openapi.yaml:stream}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/client.rs:stream}}
```

The plain method receives the whole body before it decodes it. On a feed that never ends it waits for ever.

### Reconnecting

{{#include generated/reconnect.md}}

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/client.rs:reconnect}}
```

## Conditional requests

A client sends a conditional request when the operation declares an `If-None-Match` header parameter. It can then ask for a product only if its copy is stale. When the description declares no `304` for the operation a `304` is `Err(api::ClientError::NotModified)`. It has the headers of the response:

```yaml
{{#include ../../../examples/shop/openapi.yaml:not-modified}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/client.rs:not-modified}}
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

A declared response that is not the success is still `Ok` from the call: the description declares it. `into_result` turns the response into the success and anything else into an `Err` that `?` converts into any error type. Deleting a product answers `204` or `404`:

```yaml
{{#include ../../../examples/shop/openapi.yaml:delete}}
```

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/client.rs:into-ok}}
```

One success gives what it carries: nothing, its body, its one header, or a `Success` struct with a field for its body (`body`) and one per header (`e_tag`). A field keeps its name when the description adds or reorders headers. Several successes or a `2XX` range give the response itself. The `Err` is the whole response, boxed, so its body stays typed.

## Browser (WebAssembly)

The client compiles for `wasm32-unknown-unknown` and sends through the browser's `fetch`:

1. Depend on `gloo-net` with its `http` feature under `[target.'cfg(target_arch = "wasm32")'.dependencies]`. Native builds then skip it, as the adapter compiles on wasm32 only.
2. Turn on the `gloo-net` feature of `honig` and set `server = false`.
3. Build the client on `honig::browser::GlooTransport::new("/api")?`.

Futures on wasm32 are not `Send`. A transport service of your own writes `+ honig::MaybeSend` instead of `+ Send` on its futures, so it compiles for both targets.
