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

/// The table of the bench: per kind and size in KiB, the peak heap of both sides in KiB, then the allocations and the
/// frames of both sides, each with the ratio that is judged and its limit.
fn memory_table(sizes: &[usize]) -> Table {
    let widest_size = sizes.iter().map(|&s| table::kib(s)).max_by_key(String::len).unwrap_or_default();
    let widest_path = cost::paths(sizes[0]).iter().map(|p| p.name.len()).max().unwrap_or_default();
    Table::new(&[
        Column::left("request", widest_path),
        Column::right("size_KiB", &widest_size),
        Column::right("head_KiB", "9999999"),
        Column::right("base_KiB", "9999999"),
        Column::right("peak_ratio", "99.99"),
        Column::right("peak_max", "9.99"),
        Column::right("head_allocs", "99999999"),
        Column::right("base_allocs", "99999999"),
        Column::right("allocs_ratio", "99.99"),
        Column::right("allocs_max", "9.99"),
        Column::right("head_frames", "9999999"),
        Column::right("base_frames", "9999999"),
        Column::left("result", "FAIL".len()),
    ])
}

/// What one side measured for a request: its peak heap in KiB, its allocations and the frames of its response body.
struct Side {
    peak: isize,
    allocs: u64,
    frames: usize,
}

/// The checks of a kind at one size that do not hold, by name, and its row of the table.
fn judged(path: &cost::Path, size: usize, head: &Side, base: &Side) -> (Vec<&'static str>, Vec<String>) {
    let streamed = STREAMED.contains(&path.name);
    let kib = |p: isize| u64::try_from(p).unwrap_or_default();
    let peak_ratio = ratio(kib(head.peak), kib(base.peak));
    let allocs_ratio = ratio(head.allocs, base.allocs);
    let mut failed = vec![];
    // A peak below the cap always holds: a few KiB more than the baseline is a ratio, not a cost.
    let peak_ok = head.peak <= STREAMED_PEAK || (!streamed && peak_ratio <= WHOLE_RATIO);
    if !peak_ok {
        failed.push("peak");
    }
    if allocs_ratio > path.max_allocs {
        failed.push("allocations");
    }
    // A streamed body that its baseline sends in fewer pieces is written more often over a socket.
    if streamed && head.frames > base.frames {
        failed.push("frames");
    }
    let unjudged = || "-".to_owned();
    let row = vec![
        path.name.to_owned(),
        table::kib(size),
        head.peak.to_string(),
        base.peak.to_string(),
        if streamed { unjudged() } else { format!("{peak_ratio:.2}") },
        if streamed { unjudged() } else { format!("{WHOLE_RATIO:.2}") },
        head.allocs.to_string(),
        base.allocs.to_string(),
        format!("{allocs_ratio:.2}"),
        format!("{:.2}", path.max_allocs),
        head.frames.to_string(),
        base.frames.to_string(),
        if failed.is_empty() { "ok" } else { "FAIL" }.to_owned(),
    ];
    (failed, row)
}

#[expect(clippy::cast_precision_loss, reason = "a heap in KiB or a count of allocations is far below 2^52")]
fn ratio(head: u64, base: u64) -> f64 {
    head as f64 / base.max(1) as f64
}

fn main() -> ExitCode {
    // `cargo bench` passes `--bench`.
    let given: Vec<usize> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let sizes = if given.is_empty() { SIZES.to_vec() } else { given };
    let runtime = cost::runtime();
    println!(
        "peak: streamed at most {STREAMED_PEAK} KiB, read whole at most {WHOLE_RATIO} times the baseline or \
         {STREAMED_PEAK} KiB; allocations: at most the kind's budget times the baseline; frames: streamed at most the \
         baseline's"
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
                let ((_, head_frames), (_, base_frames)) =
                    (cost::serve(&generated, &path).await, cost::serve(&baseline, &path).await);
                let (head_allocs, head_peak) = cost::heap(&generated, &path).await;
                let (base_allocs, base_peak) = cost::heap(&baseline, &path).await;
                let head = Side { peak: head_peak, allocs: head_allocs, frames: head_frames };
                let base = Side { peak: base_peak, allocs: base_allocs, frames: base_frames };
                let (failed, row) = judged(&path, size, &head, &base);
                table.row(&row);
                if !failed.is_empty() {
                    over.push(format!("{} at {}: {}", path.name, table::size(size), failed.join(", ")));
                }
            }
        });
    }
    if over.is_empty() {
        return ExitCode::SUCCESS;
    }
    println!("over a limit: {}", over.join("; "));
    ExitCode::FAILURE
}
