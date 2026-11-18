<!-- Generated from runtime/src/error.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Variant | Meaning |
|---|---|
| `Transport` | The request could not be sent or the connection broke off (connection or TLS for example). |
| `Encode` | A parameter or the body could not be serialized. |
| `Decode` | A documented response does not fit its schema. `pointer` says where. |
| `UnexpectedMedia` | The status is declared but the response's `Content-Type` is none that the description declares for it. |
| `NotModified` | `304 Not Modified` to a request that sent `If-None-Match` or `If-Modified-Since`. The operation declares no `304`. The representation that the client holds is still current. |
| `UnexpectedStatus` | A status that the operation declares neither as itself nor as a `default`. |
| `TooLarge` | A body still arriving went past a limit in bytes: the `max_body_bytes` of the server. |
