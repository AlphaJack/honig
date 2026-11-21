# Settings

`cargo honig init openapi.yaml` writes the module `api` with the settings that most crates change. Here the crate depends on `reqwest` and `uuid`:

```toml
[dependencies]
honig = { version = "=x.y.z", features = ["simd"] }

[package.metadata.honig.api]
spec = "openapi.yaml"
client = { adapters = ["reqwest"] }
server = true
formats = { uuid = true }
```

Each table `[package.metadata.honig.<module>]` is one module and sets what is generated into it. These are all its settings:

{{#include generated/settings.md}}

## Client and server

`client` and `server` are each `true`, `false` or a table that turns the half on with settings of its own:

| Value | What is generated | Runtime features |
|---|---|---|
| `client = false` | no client | none for the client |
| `client = true` | the client; you bring a transport of your own | `client` |
| `client = { adapters = ["reqwest"] }` | the client and `ReqwestTransport` | `client`, `reqwest` |
| `client = { strict = false }` | the client; a response accepts what the server added: an enum value or a member of a tagged `oneOf` as `Other`, and a property of a closed object by ignoring it. Requests stay strict: each such type gets a `<Name>Request` copy that is closed | `client` |
| `server = { adapters = ["axum", "tracing"] }` | the server, `into_axum` and a span per request | `server`, `axum`, `tracing` |
| `server = { allow_unimplemented = true }` | the server; a method you do not write answers 501 | `server` |

`operations = ["getOrder"]` on the module table keeps only those operations, by `operationId`, for both halves: the
client calls them, the server serves them, and the module contains only the types that they reach.

An adapter serves one half, and `tracing-opentelemetry` serves both. Each adapter needs its library in `[dependencies]`. [Adapters](adapters.md) lists them all.

## Several root documents

Some APIs describe themselves in many complete documents, one per area. List them, with `*` for the files of a directory:

```toml
[package.metadata.honig.api]
spec = ["specs/*.yaml"]
```

Each file is bundled against its own directory and upgraded to 3.2 alone. Then paths, webhooks, components and tags merge into one module.

- Files whose one server differs only in its path share the host. Each file's base path moves into its paths (`/login` under `/api/client/v3` is `/api/client/v3/login`). The client's base URL then serves every operation.
- Files on different hosts keep their servers on their path items.
- One method of one path in two files is an error. So is one component name with two different values.

Overlays apply to the merged document.

## Overlays

Overlays fix a description that you do not own without editing it. List [Overlays](https://spec.openapis.org/overlay/latest.html) here. They apply in order to the description as written. This happens before `$ref`s are bundled.

```toml
[package.metadata.honig.api]
spec = "vendor/openapi.yaml"
overlays = ["public.overlay.yaml"]
```

```yaml
overlay: 1.2.0
info: {title: Public operations, version: "1"}
actions:
  - target: $.paths['/internal']
    remove: true
  - target: $.paths['/products'].get
    update: {operationId: listProducts}
```

honig supports Overlay 1.0 to 1.2 with every action. That is `update`, `remove`, `copy` and the reusable actions under `components.actions`. Targets are RFC 9535 JSONPath.

honig reports a broken action with its index and skips it. The others still apply. `cargo honig check` applies the overlays too. A changed overlay makes the module stale.

## Several APIs in one crate

A crate often serves its own API and calls others. Add a table per module:

```toml
[package.metadata.honig.api]
spec = "own.yaml"
client = false

[package.metadata.honig.billing]
spec = "vendor/billing.yaml"
server = false
operations = ["createInvoice", "getInvoice"]
```

Then declare each module: `mod api;` and `mod billing;`.

- The table name is the module name. It must be a Rust identifier and not a keyword.
- The generated code of a module depends only on its description and its table.
- The runtime features are the union of what every module needs. One module with XML bodies turns on `xml` for the crate.
- Each module has its own types. Identical schemas in two descriptions are two types, so the APIs can change apart. Convert between them with `From` impls.

`cargo honig generate` writes every module. `cargo honig generate billing` writes one, and `Cargo.toml` keeps the features of all of them. `check` and `validate` accept a module name the same way.

## Workspaces

Each member keeps its own `[package.metadata.honig.<module>]`. Run the commands in the member, or at the workspace root:

- `cargo honig generate` at a virtual workspace root runs for every member with module tables. `check` does too.
- `cargo honig generate -p shop` runs for the member `shop` only. `cargo honig generate billing` runs for each member with the module `billing`.
- A member with `honig = { workspace = true }` lists its features. `generate` pins the version in the root's `[workspace.dependencies]`.
