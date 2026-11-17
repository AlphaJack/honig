//! The PR gate on allocations: no kind of path makes more than its `max_allocs` times the allocations of its
//! hand-rolled baseline at any of `cost::GATE_SIZES`. Counts do not depend on the machine or the build, so the gate runs
//! with the other tests. An extra copy of a body or a round trip through `serde_json::Value` shows here.
//! `benches/time_gate.rs` gates time.

#[path = "../cost/mod.rs"]
mod cost;

#[test]
fn no_generated_path_allocates_much_more_than_its_baseline() {
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::gate_table("honig", "base", "999999");
    for size in cost::GATE_SIZES {
        let (generated, baseline) = cost::at(size);
        for path in cost::paths(size) {
            let (allocs, baseline_allocs) = runtime.block_on(async {
                cost::same_answer(&generated, &baseline, &path).await.unwrap();
                (cost::heap(&generated, &path).await.0, cost::heap(&baseline, &path).await.0)
            });
            let ratio = allocs as f64 / baseline_allocs.max(1) as f64;
            let measured = [allocs.to_string(), baseline_allocs.to_string()];
            if cost::gate_row(&table, size, &path, measured, ratio, path.max_allocs) {
                failed.push(format!("{} at {size} B", path.name));
            }
        }
    }
    assert!(failed.is_empty(), "more allocations than the budget:\n{}", failed.join("\n"));
}
