//! The memory bench of a pull request: the peak heap of a request, generated against its hand-rolled axum baseline at
//! 1 MiB and 32 MiB, after both answer alike. A kind that streams its body must hold no more than [`STREAMED_PEAK`]
//! whatever the size: a change that buffers the whole body shows here while its time may look fine. A kind read whole
//! may hold up to [`WHOLE_RATIO`] times what its baseline holds, or any heap below that same limit. Allocations and the frames of the response body are
//! shown beside the peak: `tests/allocations.rs` judges allocations, and the frames explain a streamed body (each is at
//! least one write over a socket). `benches/time.rs` judges time.
//!
//! - `cargo bench -p e2e --bench memory`: both sizes, about 20 seconds.
//! - `cargo bench -p e2e --bench memory -- 262144`: other sizes.

use std::process::ExitCode;

use bench_support::table::{self, Column, Table};

#[path = "../cost/mod.rs"]
mod cost;

/// The sizes when none is given: 32 times apart, so a body held whole shows against one that streams.
const SIZES: [usize; 2] = [1 << 20, 32 << 20];
/// The kinds that stream their body: they hold about the same at any size.
const STREAMED: [&str; 4] = ["upload", "download", "sse", "jsonl"];
/// The most heap in KiB that a streamed kind may hold. Today they hold 16 to 64 KiB.
const STREAMED_PEAK: isize = 1024;
/// How many times its baseline's peak a kind read whole may hold. Today 0.55 to 1.23.
const WHOLE_RATIO: f64 = 1.3;

/// The table of the bench: per kind and size, the peak heap of both sides in KiB, their ratio and its limit, then the
/// allocations and the frames of both sides.
fn memory_table(sizes: &[usize]) -> Table {
    let widest_size = sizes.iter().map(|&s| table::size(s)).max_by_key(String::len).unwrap_or_default();
    let widest_path = cost::paths(sizes[0]).iter().map(|p| p.name.len()).max().unwrap_or_default();
    Table::new(&[
        Column::left("path", widest_path),
        Column::right("size", &widest_size),
        Column::right("head_KiB", "9999999"),
        Column::right("base_KiB", "9999999"),
        Column::right("ratio", "99.99"),
        Column::right("max", "1024KiB"),
        Column::right("head_allocs", "99999999"),
        Column::right("base_allocs", "99999999"),
        Column::right("head_frames", "9999999"),
        Column::right("base_frames", "9999999"),
        Column::left("result", "FAIL".len()),
    ])
}

/// Whether `path` holds `peak` KiB within its limit beside its baseline's `base`, and the limit as the table shows it.
/// A peak below [`STREAMED_PEAK`] always holds: a few KiB more than the baseline is a ratio, not a cost.
fn judged(path: &str, peak: isize, base: isize) -> (bool, String) {
    if STREAMED.contains(&path) {
        return (peak <= STREAMED_PEAK, format!("{STREAMED_PEAK}KiB"));
    }
    (peak <= STREAMED_PEAK || ratio(peak, base) <= WHOLE_RATIO, format!("{WHOLE_RATIO:.2}"))
}

#[expect(clippy::cast_precision_loss, reason = "a heap in KiB is far below 2^52")]
fn ratio(peak: isize, base: isize) -> f64 {
    peak as f64 / base.max(1) as f64
}

fn main() -> ExitCode {
    // `cargo bench` passes `--bench`.
    let given: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let sizes = if given.is_empty() { SIZES.to_vec() } else { given };
    let runtime = cost::runtime();
    println!(
        "streamed: peak at most {STREAMED_PEAK} KiB; read whole: peak at most {WHOLE_RATIO} times the baseline or {STREAMED_PEAK} KiB"
    );
    let table = memory_table(&sizes);
    let mut over = vec![];
    for &size in &sizes {
        let (generated, baseline) = cost::at(size);
        runtime.block_on(async {
            for path in cost::paths(size) {
                if let Err(e) = cost::same_answer(&generated, &baseline, &path).await {
                    over.push(e);
                    continue;
                }
                let ((_, frames), (_, base_frames)) =
                    (cost::serve(&generated, &path).await, cost::serve(&baseline, &path).await);
                let (allocs, peak) = cost::heap(&generated, &path).await;
                let (base_allocs, base_peak) = cost::heap(&baseline, &path).await;
                let (ok, max) = judged(path.name, peak, base_peak);
                let shown_ratio = if STREAMED.contains(&path.name) {
                    "-".to_owned()
                } else {
                    format!("{:.2}", ratio(peak, base_peak))
                };
                table.row(&[
                    path.name.to_owned(),
                    table::size(size),
                    peak.to_string(),
                    base_peak.to_string(),
                    shown_ratio,
                    max,
                    allocs.to_string(),
                    base_allocs.to_string(),
                    frames.to_string(),
                    base_frames.to_string(),
                    if ok { "ok" } else { "FAIL" }.to_owned(),
                ]);
                if !ok {
                    over.push(format!("{} at {}", path.name, table::size(size)));
                }
            }
        });
    }
    if over.is_empty() {
        return ExitCode::SUCCESS;
    }
    println!("peak heap over its limit: {}", over.join(", "));
    ExitCode::FAILURE
}
