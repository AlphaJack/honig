<!-- Generated from src/emit/rust.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| File | Contents |
|---|---|
| `mod.rs` | The root of the module. It declares the other files and re-exports the client, the server and the runtime. |
| `types.rs` | The schemas of the description as Rust types: structs, enums and newtypes. |
| `operations.rs` | One module per operation with its `Params`, its `Response` enum and their body types. |
| `client.rs` | The `Client` with one method per operation, and the `WebhookClient` when the description has webhooks. |
| `server.rs` | The `Api` trait with one method per operation, the router and the `Server` of this API. |
| `runtime.rs` | The runtime items that the generated code calls, re-exported from `honig`. |
| `documented_examples.rs` | With `tests = true`: a test that decodes each example of the description into its type. |
