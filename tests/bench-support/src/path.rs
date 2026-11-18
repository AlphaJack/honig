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

/// A path whose work runs in phases, each counted alone: a phase that grows too fast shows by name.
#[derive(Debug)]
pub struct Phased {
    /// The name a bench reports, and picks the path by on the command line.
    pub name: &'static str,
    /// The names of the phases, in the order the work runs them.
    pub phases: &'static [&'static str],
    /// Builds the input of about `size` bytes. Returns its exact length and the work, which runs each phase in
    /// [`phase`].
    pub prepare: fn(usize) -> (usize, Work),
}

/// Runs one phase of a [`Phased`] work and returns what it returns. callgrind counts each phase apart: each runs in
/// `measured_phase`, whose exact name `--dump-after` takes (it takes no pattern).
///
/// # Panics
/// Never: `measured_phase` runs the phase once.
pub fn phase<T>(work: impl FnOnce() -> T) -> T {
    let mut work = Some(work);
    let mut out = None;
    measured_phase(&mut || out = work.take().map(|w| w()));
    out.expect("measured_phase runs the phase once")
}

/// The one function that every phase runs in. It is not generic, so callgrind knows it by one name and dumps a count as
/// it ends. It holds a value across the call: as a tail call, `work` would end it unseen and callgrind would dump once.
#[inline(never)]
fn measured_phase(work: &mut dyn FnMut()) -> u8 {
    let mark = std::hint::black_box(1);
    work();
    std::hint::black_box(mark)
}
