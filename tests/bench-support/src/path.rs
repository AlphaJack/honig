//! A path a bench times: a function from an input size to the work it does on such an input.

/// The work that a path does on one input. It is built ahead so that the building is not counted.
pub type Work = Box<dyn FnMut()>;

/// One path: its name and what it does on an input of about `size` bytes with the exact length of that input.
#[derive(Debug)]
pub struct Path {
    /// The name a bench reports, and picks the path by on the command line.
    pub name: &'static str,
    /// Builds the input of about `size` bytes. Returns its exact length and the work to time on it.
    pub prepare: fn(usize) -> (usize, Work),
}
