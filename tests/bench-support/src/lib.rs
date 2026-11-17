//! What the benches of the generator and of the runtime share: the [`Path`] a bench times, [`callgrind::main`], which
//! fails when the cost of a path grows faster than its input allows, and the [`table`] that the benches print.

pub mod callgrind;
mod path;
pub mod table;

pub use path::{Path, Work};
