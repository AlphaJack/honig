//! How the cost of each path grows with its input. [`main`] fails when a path costs more than `n^MAX_EXPONENT`. So a
//! quadratic path shows on the run that adds it.
//!
//! The cost is the count of instructions that the callgrind of Valgrind sees. It is the same on every run, so there is
//! no noise and no retry. Each path runs at each size in a process of its own. Only the call of its work is counted and
//! not the building of its input. The steepest step between two sizes counts. So fixed costs at the small sizes cannot
//! hide growth at the large ones. Needs `valgrind` on the path (Linux).

use std::process::{Command, ExitCode};

use crate::table::{Column, Table};
use crate::{Path, Work};

/// The largest `k` in `cost ~ n^k` that a path may show. n log n fits under it. n^1.5 and n^2 do not.
const MAX_EXPONENT: f64 = 1.3;
/// A step this steep stops a path before its larger sizes. It is quadratic give or take fixed costs.
const STOP_EXPONENT: f64 = 1.8;
/// The argument that runs one path at one size in the process that callgrind watches.
const MEASURE: &str = "--measure";

/// Checks every path at every size. When callgrind runs it with `--measure` it runs one path at one size. Names on the
/// command line (`cargo bench --bench scaling_gate -- text_encode`) check only those paths.
pub fn main(paths: &[Path], sizes: &[usize]) -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [_, MEASURE, name, size] => measure(paths, name, size),
        [program, ref rest @ ..] => {
            let names: Vec<&str> = rest.iter().copied().filter(|a| !a.starts_with('-')).collect();
            let picked: Vec<&Path> = paths.iter().filter(|p| names.is_empty() || names.contains(&p.name)).collect();
            check(&picked, sizes, program)
        }
        [] => ExitCode::FAILURE,
    }
}

/// Runs `name` once at `size`. Only [`measured`] is counted.
fn measure(paths: &[Path], name: &str, size: &str) -> ExitCode {
    let (Some(path), Ok(size)) = (paths.iter().find(|p| p.name == name), size.parse()) else {
        return ExitCode::FAILURE;
    };
    let (_, mut work) = (path.prepare)(size);
    measured(&mut work);
    ExitCode::SUCCESS
}

/// The call callgrind counts: `--toggle-collect` names it.
#[inline(never)]
fn measured(work: &mut Work) {
    work();
}

/// How the cost of a path grew. That is the steepest step between two sizes and whether a step was so steep that the
/// larger sizes were left out.
struct Growth {
    steepest: f64,
    stopped: bool,
}

/// Counts every path at every size, prints how each grew and fails if one grew faster than [`MAX_EXPONENT`].
fn check(paths: &[&Path], sizes: &[usize], program: &str) -> ExitCode {
    let mut steep = vec![];
    // The cost grows as n^k with the size n of the input. The limit is the same for every path.
    println!("k may be at most {MAX_EXPONENT}; stopped: a step was steeper than k = {STOP_EXPONENT}");
    let table = Table::new(&[
        Column::left("path", paths.iter().map(|p| p.name.len()).max().unwrap_or_default()),
        Column::right("n^k", &format!("{MAX_EXPONENT:.2}")),
        Column::left("result", "stopped".len()),
    ]);
    for path in paths {
        let Some(growth) = growth(path, sizes, program) else {
            eprintln!("{}: callgrind did not run: install valgrind", path.name);
            return ExitCode::FAILURE;
        };
        let over = growth.steepest > MAX_EXPONENT;
        // A path stops only on a step steeper than the limit: it fails, and its larger sizes did not run.
        let result = match (over, growth.stopped) {
            (_, true) => "stopped",
            (true, false) => "FAIL",
            (false, false) => "ok",
        };
        table.row(&[path.name.to_owned(), format!("{:.2}", growth.steepest), result.to_owned()]);
        if over {
            steep.push(path.name);
        }
    }
    if steep.is_empty() {
        return ExitCode::SUCCESS;
    }
    eprintln!("cost grows faster than n^{MAX_EXPONENT}: {}", steep.join(", "));
    ExitCode::FAILURE
}

/// Counts `path` at each size in turn. A step steeper than [`STOP_EXPONENT`] ends it: a quadratic path at the next,
/// larger size could run for hours under callgrind.
fn growth(path: &Path, sizes: &[usize], program: &str) -> Option<Growth> {
    let mut growth = Growth { steepest: 0.0, stopped: false };
    let mut before: Option<(f64, f64)> = None;
    for &size in sizes {
        let (bytes, _) = (path.prepare)(size);
        let point = (to_f64(u64::try_from(bytes).ok()?), to_f64(instructions(program, path.name, size)?));
        if let Some(before) = before {
            growth.steepest = growth.steepest.max(exponent(before, point));
            if growth.steepest > STOP_EXPONENT {
                growth.stopped = true;
                break;
            }
        }
        before = Some(point);
    }
    Some(growth)
}

/// The instructions that `name` takes at `size` as callgrind counts them in a run of this program.
#[expect(clippy::disallowed_methods, reason = "runs the bench under valgrind and reads its report")]
fn instructions(program: &str, name: &str, size: usize) -> Option<u64> {
    let out = std::env::temp_dir().join(format!("callgrind-{name}-{size}-{}", std::process::id()));
    let status = Command::new("valgrind")
        .args(["--tool=callgrind", "--collect-atstart=no", "--toggle-collect=*measured*"])
        .arg(format!("--callgrind-out-file={}", out.display()))
        .args([program, MEASURE, name, &size.to_string()])
        .output()
        .ok()?
        .status;
    let report = std::fs::read_to_string(&out).ok();
    std::fs::remove_file(&out).ok();
    status.success().then_some(())?;
    totals(&report?)
}

/// The `totals:` line of a callgrind report: the instructions of every counted call.
fn totals(report: &str) -> Option<u64> {
    report.lines().find_map(|l| l.strip_prefix("totals:"))?.split_whitespace().next()?.parse().ok()
}

/// `k` in `y ~ x^k` between two points.
fn exponent((x0, y0): (f64, f64), (x1, y1): (f64, f64)) -> f64 {
    (y1 / y0).log(x1 / x0)
}

/// A count as a float: the counts here are far below 2^52.
#[expect(clippy::cast_precision_loss, reason = "counts here are far below 2^52, where f64 is exact")]
fn to_f64(count: u64) -> f64 {
    count as f64
}
