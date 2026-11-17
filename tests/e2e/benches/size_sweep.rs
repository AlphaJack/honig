//! How the cost of a request grows with its payload: each kind of `tests/e2e/cost` from 256 B to 32 MiB (a form stays
//! small), generated against its hand-rolled axum baseline. It prints CSV: the time of a request, its allocations, its
//! peak live heap and the frames of its response body. Over a socket each frame is at least one write, so the frames
//! show what the process does not pay for.
//!
//! - `cargo bench -p e2e --bench size_sweep`: every size, about 20 seconds.
//! - `cargo bench -p e2e --bench size_sweep -- 256 1048576`: those sizes.

use std::time::{Duration, Instant};

#[path = "../cost/mod.rs"]
mod cost;

/// The sizes when none is given: each 64 times the one before.
const SIZES: [usize; 4] = [256, 16 << 10, 1 << 20, 32 << 20];
/// Rounds of both sides in turn. The fastest round of each side counts.
const ROUNDS: usize = 5;
/// About how long one round of one side takes.
const ROUND: Duration = Duration::from_millis(20);

fn in_process(sizes: &[usize]) {
    let runtime = cost::runtime();
    let started = Instant::now();
    println!(
        "size,path,generated_us,baseline_us,time_ratio,generated_allocs,baseline_allocs,generated_peak_kib,baseline_peak_kib,\
         generated_frames,baseline_frames"
    );
    for &size in sizes {
        let (generated, baseline) = cost::at(size);
        runtime.block_on(async {
            for path in cost::paths(size) {
                if let Err(e) = cost::same_answer(&generated, &baseline, &path).await {
                    eprintln!("{e}");
                    continue;
                }
                let ((_, frames), (_, baseline_frames)) =
                    (cost::serve(&generated, &path).await, cost::serve(&baseline, &path).await);
                let timing = cost::times(&generated, &baseline, &path, ROUNDS, ROUND).await;
                let (allocs, peak) = cost::heap(&generated, &path).await;
                let (baseline_allocs, baseline_peak) = cost::heap(&baseline, &path).await;
                println!(
                    "{size},{},{:.1},{:.1},{:.2},{allocs},{baseline_allocs},{peak},{baseline_peak},{frames},{baseline_frames}",
                    path.name,
                    timing.generated.as_secs_f64() * 1e6,
                    timing.baseline.as_secs_f64() * 1e6,
                    timing.ratio,
                );
            }
        });
    }
    eprintln!("{:.1}s", started.elapsed().as_secs_f64());
}

fn main() {
    // `cargo bench` passes `--bench`.
    let sizes: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    in_process(if sizes.is_empty() { &SIZES } else { &sizes });
}
