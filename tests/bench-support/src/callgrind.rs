//! How the cost of each path grows with its input. [`main`] and [`main_phased`] fail when a path costs more than
//! `n^MAX_EXPONENT`. So a quadratic path shows on the run that adds it.
//!
//! The cost is the count of instructions that the callgrind of Valgrind sees. It is the same on every run, so there is
//! no noise and no retry. Each path runs at each size in a process of its own. The runs go side by side, one per core,
//! the smallest sizes first. Only the call of its work is counted and
//! not the building of its input. A [`Phased`] path counts each phase apart in that one run: callgrind writes a count
//! each time a phase ends. The steepest step between two sizes counts. So fixed costs at the small sizes cannot hide
//! growth at the large ones. Needs `valgrind` on the path (Linux).

use std::process::{Command, ExitCode};

use crate::functions::{self, Report};
use crate::table::{Column, Table};
use crate::{Path, Phased, Work};

/// The largest `k` in `cost ~ n^k` that a path may show. n log n fits under it. n^1.5 and n^2 do not.
const MAX_EXPONENT: f64 = 1.3;
/// A step this steep stops a path before its larger sizes. It is quadratic give or take fixed costs.
const STOP_EXPONENT: f64 = 1.8;
/// The name that callgrind gives the function each phase runs in (`crate::path`).
const PHASE_FN: &str = "bench_support::path::measured_phase";
/// How many lines of the stderr of a failed run an error quotes: the last ones, where the panic is.
const ERROR_LINES: usize = 5;
/// The argument that runs one path at one size in the process that callgrind watches.
const MEASURE: &str = "--measure";

/// A path as [`check`] counts it: [`Path`] is one phase without a name, [`Phased`] has its own.
struct Target {
    name: &'static str,
    /// Empty for a [`Path`]: its whole work is one count.
    phases: &'static [&'static str],
    prepare: fn(usize) -> (usize, Work),
}

/// Checks every path at every size. When callgrind runs it with `--measure` it runs one path at one size. Names on the
/// command line (`cargo bench --bench scaling -- text_encode`) check only those paths.
pub fn main(paths: &[Path], sizes: &[usize]) -> ExitCode {
    let targets: Vec<Target> = paths.iter().map(|p| Target { name: p.name, phases: &[], prepare: p.prepare }).collect();
    run(&targets, sizes)
}

/// [`main`] for paths whose phases are counted apart. Its table has a row per phase.
pub fn main_phased(paths: &[Phased], sizes: &[usize]) -> ExitCode {
    let targets: Vec<Target> =
        paths.iter().map(|p| Target { name: p.name, phases: p.phases, prepare: p.prepare }).collect();
    run(&targets, sizes)
}

fn run(targets: &[Target], sizes: &[usize]) -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [_, MEASURE, name, size] => measure(targets, name, size),
        [program, ref rest @ ..] => {
            let names: Vec<&str> = rest.iter().copied().filter(|a| !a.starts_with('-')).collect();
            let picked: Vec<&Target> = targets.iter().filter(|t| names.is_empty() || names.contains(&t.name)).collect();
            check(&picked, sizes, program)
        }
        [] => ExitCode::FAILURE,
    }
}

/// Runs `name` once at `size`. Only [`measured`] is counted, or each phase of a phased path.
fn measure(targets: &[Target], name: &str, size: &str) -> ExitCode {
    let (Some(target), Ok(size)) = (targets.iter().find(|t| t.name == name), size.parse()) else {
        return ExitCode::FAILURE;
    };
    let (_, mut work) = (target.prepare)(size);
    if target.phases.is_empty() {
        measured(&mut work);
    } else {
        work();
    }
    ExitCode::SUCCESS
}

/// The call callgrind counts: `--toggle-collect` names it.
#[inline(never)]
fn measured(work: &mut Work) {
    work();
}

/// How the cost of a phase grew. That is the steepest step between two sizes and whether a step was so steep that the
/// larger sizes were left out.
#[derive(Clone, Copy)]
struct Growth {
    steepest: f64,
    stopped: bool,
}

/// Counts every path at every size, prints how each phase grew and fails if one grew faster than [`MAX_EXPONENT`].
fn check(targets: &[&Target], sizes: &[usize], program: &str) -> ExitCode {
    let mut steep = vec![];
    // In the order of their names, so a path is easy to find in the table. Its phases stay in the order they run.
    let mut targets = targets.to_vec();
    targets.sort_by_key(|t| t.name);
    let phased = targets.iter().any(|t| !t.phases.is_empty());

    let mut columns = vec![Column::left("bench", targets.iter().map(|t| t.name.len()).max().unwrap_or_default())];
    if phased {
        let widest = targets.iter().flat_map(|t| t.phases).map(|p| p.len()).max().unwrap_or_default();
        columns.push(Column::left("phase", widest));
    }
    columns.extend([Column::right("n^k", &format!("{MAX_EXPONENT:.2}")), Column::left("result", "stopped".len())]);
    // Counted first: the header and the rows then print together, however long the runs take.
    let runs = counted(&targets, sizes, program);
    // The cost grows as n^k with the size n of the input. The limit is the same for every path.
    println!("k may be at most {MAX_EXPONENT}; stopped: a step was steeper than k = {STOP_EXPONENT}");
    let table = Table::new(&columns);
    let mut functions = vec![];
    for (target, runs) in targets.iter().zip(&runs) {
        functions.extend(steep_functions(target, sizes, runs).into_iter().map(|s| (target.name, s)));
        let growths = match growth(target, sizes, runs) {
            Ok(growths) => growths,
            Err(why) => {
                std::io::Write::flush(&mut std::io::stdout()).ok();
                eprintln!("{why}");
                return ExitCode::FAILURE;
            }
        };
        let names: Vec<&str> = if target.phases.is_empty() { vec![""] } else { target.phases.to_vec() };
        for (phase, growth) in names.iter().zip(growths) {
            let over = growth.steepest > MAX_EXPONENT;
            // A path stops only on a step steeper than the limit: it fails, and its larger sizes did not run.
            let result = match (over, growth.stopped) {
                (_, true) => "stopped",
                (true, false) => "FAIL",
                (false, false) => "ok",
            };
            let mut row = vec![target.name.to_owned()];
            if phased {
                row.push((*phase).to_owned());
            }
            row.extend([format!("{:.2}", growth.steepest), result.to_owned()]);
            table.row(&row);
            if over {
                steep.push(if phase.is_empty() { target.name.to_owned() } else { format!("{} {phase}", target.name) });
            }
        }
    }
    print_functions(&targets, &functions);
    if steep.is_empty() && functions.is_empty() {
        return ExitCode::SUCCESS;
    }
    // The verdict goes to stdout under the tables: a CI log reads stdout and stderr apart and can put a line of
    // stderr between two rows.
    let mut verdict = vec![];
    if !steep.is_empty() {
        verdict.push(format!("{} of the phases above", steep.len()));
    }
    if !functions.is_empty() {
        verdict.push(format!("{} of the functions above", functions.len()));
    }
    println!("cost grows faster than n^{MAX_EXPONENT}: {}", verdict.join(" and "));
    ExitCode::FAILURE
}

/// The functions of `target` whose own cost grew faster than [`MAX_EXPONENT`], over the sizes that it ran at.
fn steep_functions(target: &Target, sizes: &[usize], runs: &[Counts]) -> Vec<functions::Steep> {
    let mut bytes = vec![];
    let mut reports = vec![];
    for (&size, run) in sizes.iter().zip(runs) {
        let Some(Ok(r)) = run else { break };
        bytes.push(to_f64(u64::try_from((target.prepare)(size).0).unwrap_or(u64::MAX)));
        reports.push(r.clone());
    }
    functions::steep(&bytes, &reports, MAX_EXPONENT)
}

/// The longest function name a row shows: a generic Rust path can run to hundreds of characters.
const FUNCTION_WIDTH: usize = 90;

/// Prints the functions whose own cost grew faster than the limit, if any: their bench, their phase when the bench has
/// phases, their name, growth and share of their phase at the largest size.
fn print_functions(targets: &[&Target], steep: &[(&str, functions::Steep)]) {
    if steep.is_empty() {
        return;
    }
    let phase_name = |name: &str, i: usize| {
        targets.iter().find(|t| t.name == name).and_then(|t| t.phases.get(i)).copied().unwrap_or("")
    };
    let phased = steep.iter().any(|(n, s)| !phase_name(n, s.phase).is_empty());
    let cut = |f: &str| f.chars().take(FUNCTION_WIDTH).collect::<String>();
    println!();
    println!("functions whose own cost grows faster than n^{MAX_EXPONENT}:");
    let mut columns = vec![Column::left("bench", steep.iter().map(|(n, _)| n.len()).max().unwrap_or_default())];
    if phased {
        columns.push(Column::left(
            "phase",
            steep.iter().map(|(n, s)| phase_name(n, s.phase).len()).max().unwrap_or_default(),
        ));
    }
    columns.extend([
        Column::left("function", steep.iter().map(|(_, s)| cut(&s.function).chars().count()).max().unwrap_or_default()),
        Column::right("n^k", "9.99"),
        Column::right("share", "100.0%"),
    ]);
    let table = Table::new(&columns);
    for (name, s) in steep {
        let mut row = vec![(*name).to_owned()];
        if phased {
            row.push(phase_name(name, s.phase).to_owned());
        }
        row.extend([cut(&s.function), format!("{:.2}", s.exponent), format!("{:.1}%", s.share * 100.0)]);
        table.row(&row);
    }
}

/// What one run counted: one report, or one per phase. `None` for a run that a steep step skipped.
type Counts = Option<Result<Vec<Report>, String>>;

/// [`growth`] of every target. Each core takes the next run (a target at a size) as soon as it is free, the sizes of a
/// target side by side. The smallest sizes go first: a target's largest run starts last, and is skipped when a step
/// that already finished is steeper than [`STOP_EXPONENT`].
fn counted(targets: &[&Target], sizes: &[usize], program: &str) -> Vec<Vec<Counts>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let runs: Vec<(usize, usize)> = (0..sizes.len()).flat_map(|s| (0..targets.len()).map(move |t| (t, s))).collect();
    let next = AtomicUsize::new(0);
    let done: Mutex<Vec<Vec<Counts>>> = Mutex::new(vec![vec![None; sizes.len()]; targets.len()]);
    std::thread::scope(|scope| {
        for _ in 0..cores.min(runs.len()) {
            scope.spawn(|| {
                while let Some(&(t, s)) = runs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let steep = {
                        let d = done.lock().unwrap_or_else(PoisonError::into_inner);
                        growth(targets[t], sizes, &d[t]).is_ok_and(|g| g.iter().any(|g| g.steepest > STOP_EXPONENT))
                    };
                    if !steep {
                        let counts = instructions(program, targets[t], sizes[s]);
                        done.lock().unwrap_or_else(PoisonError::into_inner)[t][s] = Some(counts);
                    }
                }
            });
        }
    });
    done.into_inner().unwrap_or_else(PoisonError::into_inner)
}

/// How `target` grew over the runs that counted it, in the order of `sizes`, each phase apart. A run that did not
/// count (skipped, or not yet done) ends it: the target is marked stopped.
fn growth(target: &Target, sizes: &[usize], runs: &[Counts]) -> Result<Vec<Growth>, String> {
    let mut growths = vec![Growth { steepest: 0.0, stopped: false }; target.phases.len().max(1)];
    let mut before: Option<(f64, Vec<f64>)> = None;
    for (&size, run) in sizes.iter().zip(runs) {
        let Some(counts) = run else {
            for g in &mut growths {
                g.stopped = true;
            }
            break;
        };
        let counts: Vec<f64> = counts.clone()?.iter().map(|r| to_f64(r.total)).collect();
        let (bytes, _) = (target.prepare)(size);
        let bytes = to_f64(u64::try_from(bytes).map_err(|e| e.to_string())?);
        if let Some((x0, ref y0)) = before {
            for (g, (y0, y1)) in growths.iter_mut().zip(y0.iter().zip(&counts)) {
                g.steepest = g.steepest.max(exponent((x0, *y0), (bytes, *y1)));
            }
        }
        before = Some((bytes, counts));
    }
    Ok(growths)
}

/// The instructions that `target` takes at `size` as callgrind counts them in a run of this program: one count, or
/// one per phase in the order they ran. The error says which step failed.
#[expect(clippy::disallowed_methods, reason = "runs the bench under valgrind and reads its report")]
#[expect(clippy::needless_collect, reason = "the reports are read before they are removed")]
fn instructions(program: &str, target: &Target, size: usize) -> Result<Vec<Report>, String> {
    let out = std::env::temp_dir().join(format!("callgrind-{}-{size}-{}", target.name, std::process::id()));
    let mut valgrind = Command::new("valgrind");
    valgrind.args(["--tool=callgrind", "--collect-atstart=no"]);
    if target.phases.is_empty() {
        valgrind.arg("--toggle-collect=*measured*");
    } else {
        // Each phase is a call of `measured_phase`: its count is written as it ends, to `<out>.1`, `<out>.2` and on.
        // `--dump-after` takes the exact name that callgrind gives the function, and no pattern.
        valgrind.args([format!("--toggle-collect={PHASE_FN}"), format!("--dump-after={PHASE_FN}")]);
    }
    let run = valgrind
        .arg(format!("--callgrind-out-file={}", out.display()))
        .args([program, MEASURE, target.name, &size.to_string()])
        .output()
        .map_err(|e| format!("valgrind did not start ({e}): install valgrind"))?;
    let reports: Vec<std::path::PathBuf> = if target.phases.is_empty() {
        vec![out.clone()]
    } else {
        (1..=target.phases.len()).map(|i| std::path::PathBuf::from(format!("{}.{i}", out.display()))).collect()
    };
    let counts: Vec<Option<Report>> = reports.iter().map(|r| functions::read(r)).collect();
    std::fs::remove_file(&out).ok();
    for r in &reports {
        std::fs::remove_file(r).ok();
    }
    if !run.status.success() {
        let stderr = String::from_utf8_lossy(&run.stderr);
        let last: Vec<&str> = stderr.lines().rev().take(ERROR_LINES).collect();
        return Err(format!("{} at {size}: the run failed ({}): {}", target.name, run.status, last.join(" | ")));
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            c.ok_or_else(|| match target.phases.get(i) {
                Some(phase) => format!("{} at {size}: callgrind wrote no count for phase `{phase}`", target.name),
                None => format!("{} at {size}: callgrind wrote no count", target.name),
            })
        })
        .collect()
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
