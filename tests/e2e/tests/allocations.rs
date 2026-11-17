//! The allocation bench of a pull request: no kind of path makes more than its `max_allocs` times the allocations
//! of its hand-rolled baseline at any of `cost::BENCH_SIZES`. Counts do not depend on the machine or the build, so it
//! runs with the other tests. An extra copy of a body or a round trip through `serde_json::Value` shows here.
//! `benches/time.rs` times the same paths.

#[path = "../cost/mod.rs"]
mod cost;
#[path = "../cost/roundtrip.rs"]
mod roundtrip;

#[test]
fn no_generated_path_allocates_much_more_than_its_baseline() {
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::bench_table("head", "base", "999999");
    let mut rows = vec![];
    for size in cost::BENCH_SIZES {
        let (generated, baseline) = cost::at(size);
        for path in cost::paths(size) {
            let (allocs, baseline_allocs) = runtime.block_on(async {
                cost::same_answer(&generated, &baseline, &path).await.unwrap();
                (cost::heap(&generated, &path).await.0, cost::heap(&baseline, &path).await.0)
            });
            let ratio = allocs as f64 / baseline_allocs.max(1) as f64;
            let sides = [allocs.to_string(), baseline_allocs.to_string()];
            rows.push(cost::Measured { path: path.name, size, sides, ratio, max: path.max_allocs });
        }
    }
    failed.extend(cost::print_rows(&table, rows));
    assert!(failed.is_empty(), "more allocations than the budget:\n{}", failed.join("\n"));
}

/// The same of a roundtrip: the generated client and server against a hand-rolled client and server.
#[test]
fn no_generated_roundtrip_allocates_much_more_than_its_baseline() {
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::bench_table("head", "base", "999999");
    let mut rows = vec![];
    for size in cost::BENCH_SIZES {
        let (head, base) = roundtrip::at(size);
        for trip in &roundtrip::TRIPS {
            let (allocs, baseline_allocs) = runtime.block_on(async {
                roundtrip::same_answer(&head, &base, trip.name).await.unwrap();
                let ours = cost::heap_of(async || {
                    roundtrip::head(&head, trip.name).await.ok();
                });
                let theirs = cost::heap_of(async || {
                    roundtrip::base(&base, trip.name).await.ok();
                });
                (ours.await.0, theirs.await.0)
            });
            let ratio = allocs as f64 / baseline_allocs.max(1) as f64;
            let sides = [allocs.to_string(), baseline_allocs.to_string()];
            rows.push(cost::Measured { path: trip.name, size, sides, ratio, max: trip.max_allocs });
        }
    }
    failed.extend(cost::print_rows(&table, rows));
    assert!(failed.is_empty(), "more allocations than the budget:\n{}", failed.join("\n"));
}
