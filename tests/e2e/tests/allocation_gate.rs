//! The PR gate on allocations: no kind of path makes more than its `max_allocs` times the allocations of its
//! hand-rolled twin at any of `cost::GATE_SIZES`. Counts do not depend on the machine or the build, so the gate runs
//! with the other tests. An extra copy of a body or a round trip through `serde_json::Value` shows here.
//! `benches/time_gate.rs` gates time.

#[path = "../cost/mod.rs"]
mod cost;

#[test]
fn no_generated_path_allocates_much_more_than_its_twin() {
    let runtime = cost::runtime();
    let mut failed = vec![];
    println!("size_bytes\tpath\thonig_allocs\ttwin_allocs\tratio\tmax_ratio\tresult");
    for size in cost::GATE_SIZES {
        let (generated, twin) = cost::at(size);
        for path in cost::paths(size) {
            let (allocs, twin_allocs) = runtime.block_on(async {
                cost::both_succeed(&generated, &twin, &path).await.unwrap();
                (cost::heap(&generated, &path).await.0, cost::heap(&twin, &path).await.0)
            });
            let ratio = allocs as f64 / twin_allocs.max(1) as f64;
            let over = ratio > path.max_allocs;
            println!(
                "{size}\t{}\t{allocs}\t{twin_allocs}\t{ratio:.2}\t{:.2}\t{}",
                path.name,
                path.max_allocs,
                if over { "FAIL" } else { "ok" }
            );
            if over {
                failed.push(format!("{} at {size} B", path.name));
            }
        }
    }
    assert!(failed.is_empty(), "more allocations than the budget:\n{}", failed.join("\n"));
}
