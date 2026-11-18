<!-- Generated from src/settings.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Setting | Required | Default | What it does |
|---|---|---|---|
| `spec` | yes |  | the description relative to Cargo.toml; a list ["a.yaml", "api/*.yaml"] merges several |
| `output` | no | `"src/<module>"` | the module directory |
| `mod_rs` | no | `the crate's clippy lints, else true` | true writes the root of the module as `<output>/mod.rs`, false as `<output>.rs` |
| `client` | no | `true` | generate the client; a table sets its adapters = ["reqwest"] and operations = ["getOrder"] |
| `server` | no | `true` | generate the server; a table sets its adapters = ["axum"] and allow_unimplemented = true |
| `tests` | no | `false` | generate a test that the description's examples decode |
| `unknown_fields` | no | `"drop"` | "keep": objects without additionalProperties keep unknown keys |
| `overlays` | no | `[]` | Overlays (1.0 to 1.2) applied in order to the spec |
| `formats` | yes |  | required: library types of string formats, like { uuid = true, dates = "chrono" } or {} |
| `derives` | no | `{}` | extra derives on every type or on one type by Rust name, like { "*" = ["Hash"] } |
