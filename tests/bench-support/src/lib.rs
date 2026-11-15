//! What the benches of the generator and of the runtime share: the [`Path`] a bench times, and [`callgrind::main`],
//! which fails when the cost of a path grows faster than its input allows.

pub mod callgrind;
mod path;

pub use path::{Path, Work};
