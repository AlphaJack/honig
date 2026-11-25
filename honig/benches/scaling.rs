//! How the cost of each path of [`paths::PATHS`] grows with its input. callgrind counts it. See `tests/bench-support/src/callgrind.rs`.

#[path = "support/paths.rs"]
mod paths;

/// Input sizes in bytes, each 4 times the one before.
const SIZES: [usize; 4] = [16 << 10, 64 << 10, 256 << 10, 1 << 20];

fn main() -> std::process::ExitCode {
    bench_support::callgrind::main("scaling-runtime", paths::PATHS, &SIZES)
}
