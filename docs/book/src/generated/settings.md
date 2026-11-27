<!-- Generated from src/settings.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Setting | Required | Default | What it does |
|---|---|---|---|
| `descriptions` | yes |  | the descriptions relative to Cargo.toml, like ["openapi.yaml"]; several, such as ["a.yaml", "api/*.yaml"], merge |
| `output` | no | `"src/<module>"` | the module directory |
| `mod_rs` | no | `true` | true writes the root of the module as `<output>/mod.rs`, false as `<output>.rs` |
| `client` | no | `true` | generate the client; a table sets allow_unknown_values = true |
| `server` | no | `true` | generate the server; a table sets allow_unimplemented = true |
| `operations` | no | `every one` | the only operations, by operationId, that the client calls and the server serves; the module contains only the types that they reach |
| `tests` | no | `false` | generate a test that the description's examples decode |
| `keep_unknown_fields` | no | `false` | objects without additionalProperties keep unknown keys in a map |
| `overlays` | no | `[]` | Overlays (1.0 to 1.2) applied in order to the spec |
| `formats` | no | `{}` | your own types for formats with a check, like { uuid = "crate::Id" } |
| `derives` | no | `{}` | extra derives on every type or on one type by Rust name, like { "*" = ["Hash"] } |
