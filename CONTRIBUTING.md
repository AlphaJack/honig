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

## Hostile input

A path of [paths.rs](runtime/benches/support/paths.rs) gets the worst input that its grammar allows, not a typical body: a repeat, no match, the separator in every place, the deepest nesting. The scaling bench then fails it when its cost grows faster than its input. A `perf` or `fix` of a slow path adds the path that fails on `main` before the change.

A decoder that recurses as its input nests joins [depth.rs](runtime/tests/depth.rs): an input nested past any limit must return on a small stack.

## Pull requests

The title is a conventional commit such as `feat(generator): add typed links`. Its subject starts with an imperative verb. [pr-title.yml](.github/workflows/pr-title.yml) checks it.

| Type | Usage |
| --- | --- |
| `feat` | something users can do that they could not before |
| `fix` | a bug users hit |
| `perf` | the same behaviour but faster or smaller |
| `refactor`, `docs`, `test`, `chore` | anything users do not notice |

| Scope | Usage |
| --- | --- |
| `cli` | the command users run |
| `generator` | the generator and the code it writes |
| `runtime` | the dependency crate |

The scope is optional. A `!` after it marks a breaking change. The description then needs `BREAKING CHANGE: <what users must do>`.