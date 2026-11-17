//! The PR gate on time: no kind of path takes more than its `max_time` times the time of its hand-rolled twin at any
//! of `cost::GATE_SIZES`. Both run in turn in one process, so the speed of the machine cancels out of the ratio. It
//! needs a release build, so it is a bench: `cargo bench -p e2e --bench time_gate`. `tests/allocation_gate.rs` gates
//! allocations.

use std::time::{Duration, Instant};

#[path = "../cost/mod.rs"]
mod cost;

/// Rounds of both sides in turn. The fastest round of each side counts: it is the one least disturbed.
const ROUNDS: usize = 15;
/// About how long one round of one side takes.
const ROUND: Duration = Duration::from_millis(4);
/// Measurements of a path before it fails.
const ATTEMPTS: usize = 3;

/// How many times the time of its twin the generated path takes.
fn ratio_of((time, twin_time): (Duration, Duration)) -> f64 {
    time.as_secs_f64() / twin_time.as_secs_f64()
}

fn main() -> std::process::ExitCode {
    let started = Instant::now();
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::gate_table("honig µs", "twin µs", "99999.9");
    for size in cost::GATE_SIZES {
        let (generated, twin) = cost::at(size);
        for path in cost::paths(size) {
            let measured = runtime.block_on(async {
                cost::both_succeed(&generated, &twin, &path).await?;
                // A shared CI runner stalls now and then. A path over its limit is measured again. A real slowdown
                // fails every attempt and a stall does not.
                let mut best = cost::times(&generated, &twin, &path, ROUNDS, ROUND).await;
                for _ in 1..ATTEMPTS {
                    if ratio_of(best) <= path.max_time {
                        break;
                    }
                    let again = cost::times(&generated, &twin, &path, ROUNDS, ROUND).await;
                    if ratio_of(again) < ratio_of(best) {
                        best = again;
                    }
                }
                Ok::<_, String>(best)
            });
            let (time, twin_time) = match measured {
                Ok(times) => times,
                Err(e) => {
                    failed.push(e);
                    continue;
                }
            };
            let micros = |d: Duration| format!("{:.1}", d.as_secs_f64() * 1e6);
            let measured = [micros(time), micros(twin_time)];
            if cost::gate_row(&table, size, &path, measured, ratio_of((time, twin_time)), path.max_time) {
                failed.push(format!("{} at {size} B", path.name));
            }
        }
    }
    // stdout, after the table: on stderr a line lands inside it.
    println!("took {:.1}s", started.elapsed().as_secs_f64());
    for f in &failed {
        println!("FAIL {f}");
    }
    if failed.is_empty() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}
