//! The codecs of the runtime from 256 B to 16 MiB with one bench per path of [`paths::PATHS`]. Throughput is per input
//! byte.

#[path = "support/paths.rs"]
mod paths;

use std::fmt;

use divan::Bencher;
use divan::counter::BytesCount;

const SIZES: [usize; 5] = [256, 8 << 10, 128 << 10, 2 << 20, 16 << 20];

fn main() {
    // A time cap per bench and size. A whole run takes seconds. A slow path shows as few samples and not as a wait.
    // Options on the command line (`--max-time 30` to profile one) override these.
    divan::Divan::default().sample_count(20).max_time(std::time::Duration::from_millis(200)).config_with_args().main();
}

/// One path at one size.
struct Case {
    path: &'static bench_support::Path,
    size: usize,
}

impl fmt::Display for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.path.name, self.size)
    }
}

fn cases() -> Vec<Case> {
    paths::PATHS.iter().flat_map(|path| SIZES.map(|size| Case { path, size })).collect()
}

#[divan::bench(args = cases())]
fn codec(b: Bencher<'_, '_>, case: &Case) {
    let (bytes, mut work) = (case.path.prepare)(case.size);
    b.counter(BytesCount::new(bytes)).bench_local(&mut work);
}
