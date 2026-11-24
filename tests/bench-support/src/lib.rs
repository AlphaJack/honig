//! What the benches of the generator and of honig share: the [`Path`] a bench times, or the [`Phased`] path whose
//! phases it counts apart, [`callgrind::main`] and [`callgrind::main_phased`], which fail when the cost grows faster
//! than its input allows, and the [`table`] that the benches print.

pub mod callgrind;
mod functions;
mod path;
pub mod results;
pub mod table;

pub use path::{Path, Phased, Work, phase};
