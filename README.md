# honig

<!-- ANCHOR: lead -->
honig: OpenAPI 3.2 to a Rust client and server.

It is meant for spec-first workflow and catches drift from the OpenAPI description at compile time.

It supports OpenAPI 3.2 and earlier, focusing on both correctness and runtime performance.
<!-- ANCHOR_END: lead -->

## Install

<!-- ANCHOR: install -->
honig installs as a pre-built binary with [cargo-binstall](https://github.com/cargo-bins/cargo-binstall):

```sh
cargo binstall cargo-honig
```
<!-- ANCHOR_END: install -->

## Getting started

<!-- ANCHOR: getting-started -->
These commands add honig to your crate:

```sh
cargo honig init path/to/openapi.yaml
cargo add honig@=x.y.z -F client,server,...
cargo honig generate
```

`init` adds the `[package.metadata.honig.api]` section to your `Cargo.toml` and prints the exact `cargo add` lines:
`honig` with every feature that the description needs, such as `regress` for a `pattern`.
`generate` writes the module to `src/api`. It never edits `Cargo.toml`.
When the code needs a `honig` feature that is off, `generate` stops and prints the `cargo add` line that turns it on.

Then you can declare the module in `src/lib.rs` or `src/main.rs`:

```rust
pub mod api;
```
<!-- ANCHOR_END: getting-started -->

The book's [recipes](https://alphajack.github.io/honig/recipes.html) build whole setups: a server and its client, a web app next to its API, auth from a layer that you already run, and a full middleware stack.

### Update the API

<!-- ANCHOR: update -->
`cargo honig generate` updates the module after every change to the description.
Every place your code must change becomes a compile error: a new operation is a missing method, a new response is a missing match arm.

Run `cargo honig generate --check` in CI to assert that the module is up to date with the description or the generator.
<!-- ANCHOR_END: update -->

### Update honig

<!-- ANCHOR: upgrade -->
Install the new generator, then generate again:

```sh
cargo binstall cargo-honig
cargo honig generate
```

`generate` moves the `honig` dependency to the version of the generator. The two always match: the generated code calls runtime internals that change between versions.
<!-- ANCHOR_END: upgrade -->

## Full documentation

The [book](https://alphajack.github.io/honig) covers usage and features.
