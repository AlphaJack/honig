# Performance

## Allocator

Use mimalloc in your binary. Decoding a JSON body allocates every string and `Vec` it contains. The system allocator is the largest cost after parsing.

Add `mimalloc = "0.1"` to `[dependencies]`. Then declare it in `main.rs`: `#[global_allocator] static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;`.

## SIMD

The `simd` feature speeds up large bodies. Small ones see no difference. Its row in the [feature table](adapters.md) lists the library of each path and says when it is on.

A body that fails to decode is decoded again by `serde_json`. Errors still point to the field that is wrong.

Leaving `simd` out of the features trades runtime performance for a lighter build. JSON goes through `serde_json` and base64 through the scalar engine of the `base64` crate. `memchr` stays.

`sonic-rs` has SIMD on x86_64 and aarch64 only. On other targets JSON stays on `serde_json`. This includes wasm32 clients. The other libraries of `simd` still apply. On wasm32 they use SIMD when you build with `-C target-feature=+simd128`.

`simdutf8` and `base64-simd` pick the instructions of the CPU when the program starts. `sonic-rs` picks them at compile time. A plain x86_64 build uses SSE2, which every x86_64 CPU has. `RUSTFLAGS="-C target-cpu=native"` adds AVX2. A binary built that way stops with an illegal instruction on a CPU without AVX2, so target the oldest CPU you deploy to. aarch64 needs no flag.

## Logging

The span and log line of the `tracing` adapter cost a fixed amount per request. It shows on small requests and fades on large ones.

## Patterns

`pattern` checks run on `regress`. This is the ECMA-262 engine that JSON Schema asks for. One check is cheap. A large array of patterned fields adds up.

## Benches

`cargo bench -p honig --all-features` times honig from 256 B to 16 MiB. CI also counts instructions under Valgrind. It fails when a path costs more than n^1.3 (`--bench scaling`). To see what SIMD gives, compare `--bench simd --features multipart` with `--bench simd --features multipart,simd`.
