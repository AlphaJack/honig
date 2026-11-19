//! The roundtrip bench: business code calls the generated client, the generated server, the handler and back, against
//! a hand-rolled client and server. No kind takes more than its `max_time` times its baseline at any of
//! `cost::BENCH_SIZES`. It needs a release build: `cargo bench -p e2e --bench roundtrip`.

use std::time::{Duration, Instant};

#[path = "../cost/mod.rs"]
mod cost;
#[path = "../cost/roundtrip.rs"]
mod roundtrip;

/// Pairs of rounds, one of each side in turn. The median ratio of a pair counts: a stall moves it least.
const ROUNDS: usize = 15;
/// About how long one round of one side takes.
const ROUND: Duration = Duration::from_millis(4);

fn main() -> std::process::ExitCode {
    let started = Instant::now();
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::bench_table("head_µs", "base_µs", "99999.9");
    let mut rows = vec![];
    for size in cost::BENCH_SIZES {
        let (head, base) = roundtrip::at(size);
        for trip in &roundtrip::TRIPS {
            let measured = runtime.block_on(async {
                roundtrip::same_answer(&head, &base, trip.name).await?;
                let ours = async || {
                    std::hint::black_box(roundtrip::head(&head, trip.name).await.ok());
                };
                let theirs = async || {
                    std::hint::black_box(roundtrip::base(&base, trip.name).await.ok());
                };
                Ok::<_, String>(cost::times_of(&ours, &theirs, ROUNDS, ROUND).await)
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
            let max = cost::time_budget(trip.max_time, size);
            rows.push(cost::Measured { path: trip.name, size, sides, ratio: timing.ratio, max });
        }
    }
    let mut results = bench_support::results::Results::open("roundtrip");
    failed.extend(cost::print_rows(&table, rows, Some(&mut results)));
    results.save();
    // stdout, after the table: on stderr a line lands inside it.
    println!("took {:.1}s", started.elapsed().as_secs_f64());
    for f in &failed {
        println!("FAIL {f}");
    }
    if failed.is_empty() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}
