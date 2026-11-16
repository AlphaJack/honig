# Contributing

## Setup

```sh
# to test the browser client
rustup target add wasm32-unknown-unknown
cargo binstall cargo-nextest cargo-deny cargo-hack cargo-acyclic mdbook
# for cargo-acyclic
rustup component add rust-analyzer
```

## Checks

Run these before pushing:

```sh
cargo fmt --all
CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --all-targets
cargo nextest run --workspace --run-ignored all
cargo acyclic
CARGO_BUILD_WARNINGS=deny cargo hack clippy -p honig --each-feature --all-targets
cargo hack nextest run -p honig --each-feature
```

## Pull requests

The title is a conventional commit such as `feat(codegen): add typed links`. Its subject starts with an imperative verb. [pr-title.yml](.github/workflows/pr-title.yml) checks it.

| Type | Usage |
| --- | --- |
| `feat` | something users can do that they could not before |
| `fix` | a bug users hit |
| `perf` | the same behaviour but faster or smaller |
| `refactor`, `docs`, `test`, `chore` | anything users do not notice |

| Scope | Usage |
| --- | --- |
| `cli` | the command users run |
| `codegen` | the generated code |
| `runtime` | the dependency crate |

The scope is optional. A `!` after it marks a breaking change. The description then needs `BREAKING CHANGE: <what users must do>`.