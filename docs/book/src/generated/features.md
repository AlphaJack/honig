<!-- Generated from runtime/Cargo.toml by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Feature | Crate | Adds | On when |
|---|---|---|---|
| `client` | tower-service | The generated client: the `Transport` bound over tower services and `TowerTransport` | You generate the client |
| `server` | tower-service | The generated server: a tower `Service`, routing, conditional requests and the wide event | You generate the server |
| `reqwest` | reqwest | `ReqwestTransport`: async client with streaming bodies | `client = { adapters = ["reqwest"] }` |
| `reqwest-middleware` | reqwest-middleware | `ReqwestMiddlewareTransport`: retries, tracing and caching from your middleware stack | `client = { adapters = ["reqwest-middleware"] }` |
| `ureq` | ureq | `UreqTransport`: blocking client | `client = { adapters = ["ureq"] }` |
| `gloo-net` | gloo-net | `GlooTransport`: browser `fetch` on `wasm32` only | `client = { adapters = ["gloo-net"] }` |
| `axum` | axum | `Server::into_axum` and the address of the client from the `ConnectInfo` of axum | `server = { adapters = ["axum"] }` |
| `tracing` | tracing | A span per request and a log line for each wide event | `server = { adapters = ["tracing"] }` |
| `tracing-opentelemetry` | opentelemetry | W3C Trace Context: `traceparent` and `tracestate` | `adapters = ["tracing-opentelemetry"]` in `client` or `server` |
| `jiff` | jiff | `FullTime`: `format: time` with its offset in `jiff` types | The description calls for it |
| `idn` | idna | `email`, `idn-email`, `hostname` and `idn-hostname` checks | The description calls for it |
| `regress` | regress | `pattern` and `patternProperties` checks (ECMA-262) | The description calls for it |
| `oauth` |  | The `OAuth` 2.0 flows of the client: token grants, device authorization and discovery | The description calls for it |
| `pkce` | sha2 | `Pkce`, `authorization_url` and `exchange_code`: the OAuth 2.0 authorization code flow. Outside wasm32 `Pkce::new` makes a verifier from OS randomness | The description calls for it |
| `cookie` | cookie | `Cookie` parameters and API keys | The description calls for it |
| `multipart` | multer | `multipart/*` bodies and parts | The description calls for it |
| `base64` | base64 | Binary data in JSON (`format: byte`, `contentEncoding`, `base64url`) and `http basic` credentials | The description calls for it |
| `xml` | quick-xml | XML bodies and parts | The description calls for it |
| `simd` | sonic-rs | SIMD for large bodies: JSON with `sonic-rs` (x86_64 and aarch64), the UTF-8 check of text with `simdutf8` and base64 with `base64-simd`. Multipart boundaries use `memchr` with or without it | You or `cargo honig init` add it |
