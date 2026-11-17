//! The time bench of a pull request: no kind of path takes more than its `max_time` times the time of its
//! hand-rolled baseline at any of `cost::BENCH_SIZES`. Both run in turn in one process, so the speed of the machine
//! cancels out of the ratio. It needs a release build: `cargo bench -p e2e --bench time`. `tests/allocations.rs` counts
//! allocations.

use std::time::{Duration, Instant};

#[path = "../cost/mod.rs"]
mod cost;

/// Pairs of rounds, one of each side in turn. The median ratio of a pair counts: a stall moves it least.
const ROUNDS: usize = 15;
/// About how long one round of one side takes.
const ROUND: Duration = Duration::from_millis(4);
/// Measurements of a path before it fails.
const ATTEMPTS: usize = 3;

fn main() -> std::process::ExitCode {
    let started = Instant::now();
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::bench_table("head µs", "base µs", "99999.9");
    let mut rows = vec![];
    for size in cost::BENCH_SIZES {
        let (generated, baseline) = cost::at(size);
        for path in cost::paths(size) {
            let measured = runtime.block_on(async {
                cost::same_answer(&generated, &baseline, &path).await?;
                // A shared CI runner stalls now and then. A path over its limit is measured again. A real slowdown
                // fails every attempt and a stall does not.
                let mut best = cost::times(&generated, &baseline, &path, ROUNDS, ROUND).await;
                for _ in 1..ATTEMPTS {
                    if best.ratio <= path.max_time {
                        break;
                    }
                    let again = cost::times(&generated, &baseline, &path, ROUNDS, ROUND).await;
                    if again.ratio < best.ratio {
                        best = again;
                    }
                }
                Ok::<_, String>(best)
            });
            let timing = match measured {
                Ok(timing) => timing,
                Err(e) => {
                    failed.push(e);
                    continue;
                }
            };
            let micros = |d: Duration| format!("{:.1}", d.as_secs_f64() * 1e6);
            let sides = [micros(timing.generated), micros(timing.baseline)];
            rows.push(cost::Measured { path: path.name, size, sides, ratio: timing.ratio, max: path.max_time });
        }
    }
    failed.extend(cost::print_rows(&table, rows));
    // stdout, after the table: on stderr a line lands inside it.
    println!("took {:.1}s", started.elapsed().as_secs_f64());
    for f in &failed {
        println!("FAIL {f}");
    }
    if failed.is_empty() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}
