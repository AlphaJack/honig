<!-- Generated from runtime/Cargo.toml by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Feature | Built against | Adds | On when |
|---|---|---|---|
| `client` | tower-service 0.3 | The generated client: the `Transport` bound over tower services and `TowerTransport` | You generate the client |
| `server` | tower-service 0.3 | The generated server: a tower `Service`, routing, conditional requests and the wide event | You generate the server |
| `reqwest` | reqwest 0.13 | `ReqwestTransport`: async client with streaming bodies | `client = { adapters = ["reqwest"] }` |
| `reqwest-middleware` | reqwest-middleware 0.5 | `ReqwestMiddlewareTransport`: retries, tracing and caching from your middleware stack | `client = { adapters = ["reqwest-middleware"] }` |
| `ureq` | ureq 3 | `UreqTransport`: blocking client | `client = { adapters = ["ureq"] }` |
| `gloo-net` | gloo-net 0.6 | `GlooTransport`: browser `fetch` on `wasm32` only | `client = { adapters = ["gloo-net"] }` |
| `axum` | axum 0.8 | `Server::into_axum` and the address of the client from the `ConnectInfo` of axum | `server = { adapters = ["axum"] }` |
| `tracing` | tracing 0.1 | A span per request and a log line for each wide event | `server = { adapters = ["tracing"] }` |
| `tracing-opentelemetry` | opentelemetry 0.33 | W3C Trace Context: `traceparent` and `tracestate` | `adapters = ["tracing-opentelemetry"]` in `client` or `server` |
| `ipnet` | ipnet 2 | `format: ipv4-cidr` and `ipv6-cidr` checks | The description calls for it |
| `idn` | idna 1 | `email`, `idn-email`, `hostname` and `idn-hostname` checks | The description calls for it |
| `regress` | regress 0.12 | `pattern` and `patternProperties` checks (ECMA-262) | The description calls for it |
| `oauth` |  | The `OAuth` 2.0 flows of the client: token grants, device authorization and discovery | The description calls for it |
| `pkce` | sha2 0.10 | `Pkce`, `authorization_url` and `exchange_code`: the OAuth 2.0 authorization code flow. Outside wasm32 `Pkce::new` makes a verifier from OS randomness | The description calls for it |
| `cookie` | cookie 0.18 | `Cookie` parameters and API keys | The description calls for it |
| `multipart` | multer 3.1 | `multipart/*` bodies and parts | The description calls for it |
| `base64` | base64 0.23 | Binary data in JSON (`format: byte`, `contentEncoding`, `base64url`) and `http basic` credentials | The description calls for it |
| `xml` | quick-xml 0.42 | XML bodies and parts | The description calls for it |
| `simd` | sonic-rs 0.5 | SIMD for large bodies: JSON with `sonic-rs` (x86_64 and aarch64), the UTF-8 check of text with `simdutf8` and base64 with `base64-simd`. Multipart boundaries use `memchr` with or without it | You or `cargo honig init` add it |
