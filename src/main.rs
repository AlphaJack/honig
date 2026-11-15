//! `cargo honig`.

// The library under one name whatever the package is called.
extern crate cargo_honig as generator;

use std::process::ExitCode;

fn main() -> ExitCode {
    generator::run(std::env::args().skip(1))
}
