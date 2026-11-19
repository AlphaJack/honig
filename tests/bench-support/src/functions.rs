//! The cost of each function in a callgrind report, and the functions whose own cost grows faster than their input
//! allows. A phase adds up every function, so a function that grows as n^2 hides there until it outweighs the linear
//! work around it. Its own cost shows the growth at once, whatever its share.

use std::collections::BTreeMap;
use std::path::Path;

/// The crates whose functions are judged. The standard library and libc are not: their algorithms are known, and their
/// counts jump at sizes where a copy or an allocation changes strategy. A quadratic call of them still shows in the
/// phase.
const OWN: [&str; 4] = ["honig::", "cargo_honig::", "e2e::", "bench_support::"];

/// Does `function` belong to one of the [`OWN`] crates? A trait method of one of their types reads `<honig::...>`.
fn own(function: &str) -> bool {
    let name = function.strip_prefix('<').unwrap_or(function);
    OWN.iter().any(|c| name.starts_with(c))
}

/// The smallest share of its phase, at the largest size, that a function must have to be judged. A function of a few
/// hundred instructions can jump between sizes for reasons that do not grow with the input.
const FLOOR: f64 = 0.005;

/// What one report counted: the instructions of the phase, and of each function in it. A function counts its own
/// instructions and not those of the functions it calls.
#[derive(Debug, Clone, Default)]
pub(crate) struct Report {
    pub total: u64,
    pub functions: BTreeMap<String, u64>,
}

/// A function whose own cost grew faster than the limit: its phase by index, its name, its steepest step and its
/// share of the phase at the largest size.
#[derive(Debug)]
pub(crate) struct Steep {
    pub phase: usize,
    pub function: String,
    pub exponent: f64,
    pub share: f64,
}

/// Reads the report at `path`: its total, and each function's own count.
#[expect(clippy::disallowed_methods, reason = "reads a callgrind report")]
pub(crate) fn read(path: &Path) -> Option<Report> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(Report { total: totals(&text)?, functions: own_costs(&text) })
}

/// Each function's own count in a callgrind report. `fn=` starts the lines of a function and `cfn=` names the one
/// it calls: both may give a name to an id, `fn=(7) name`, which later lines use alone, `fn=(7)`. A line of cost is a
/// position and then the count. The line after `calls=` is the cost of that call, the callee's, and is left out.
fn own_costs(text: &str) -> BTreeMap<String, u64> {
    let mut names = BTreeMap::new();
    let mut costs = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut callee_next = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("fn=") {
            current = Some(named(&mut names, rest));
        } else if let Some(rest) = line.strip_prefix("cfn=") {
            named(&mut names, rest);
        } else if line.starts_with("calls=") {
            callee_next = true;
        } else if line.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | '*')) {
            let count = line.split_whitespace().nth(1).and_then(|c| c.parse::<u64>().ok());
            if std::mem::take(&mut callee_next) {
                continue;
            }
            if let (Some(f), Some(count)) = (&current, count) {
                *costs.entry(f.clone()).or_insert(0) += count;
            }
        }
    }
    costs
}

/// The name of `(id) name` or `(id)`: a name given here is kept for its id.
fn named(names: &mut BTreeMap<String, String>, compressed: &str) -> String {
    let Some((id, rest)) = compressed.strip_prefix('(').and_then(|r| r.split_once(')')) else {
        return compressed.to_owned();
    };
    let rest = rest.trim();
    if !rest.is_empty() {
        names.insert(id.to_owned(), rest.to_owned());
    }
    names.get(id).cloned().unwrap_or_else(|| compressed.to_owned())
}

/// The `totals:` line of a callgrind report: the instructions of every counted call.
pub(crate) fn totals(report: &str) -> Option<u64> {
    report.lines().find_map(|l| l.strip_prefix("totals:"))?.split_whitespace().next()?.parse().ok()
}

/// The functions whose own cost grew faster than `limit` between two sizes in a row. `bytes` holds the input size of
/// each run and `reports` its reports, one per phase. Only a function with [`FLOOR`] of its phase at the largest size
/// is judged, and only between sizes where it ran at all.
pub(crate) fn steep(bytes: &[f64], reports: &[Vec<Report>], limit: f64) -> Vec<Steep> {
    let Some(last) = reports.last() else { return vec![] };
    let mut out = vec![];
    for (phase, at_last) in last.iter().enumerate() {
        for (name, &count) in &at_last.functions {
            let share = to_f64(count) / to_f64(at_last.total.max(1));
            if share < FLOOR || !own(name) {
                continue;
            }
            let counts: Vec<f64> = reports
                .iter()
                .map(|r| to_f64(r.get(phase).and_then(|p| p.functions.get(name)).copied().unwrap_or(0)))
                .collect();
            let exponent = bytes
                .windows(2)
                .zip(counts.windows(2))
                .filter(|(_, c)| c[0] > 0.0 && c[1] > 0.0)
                .map(|(b, c)| (c[1] / c[0]).log(b[1] / b[0]))
                .fold(0.0, f64::max);
            if exponent > limit {
                out.push(Steep { phase, function: name.clone(), exponent, share });
            }
        }
    }
    out
}

/// A count as a float: the counts here are far below 2^52.
#[expect(clippy::cast_precision_loss, reason = "counts here are far below 2^52, where f64 is exact")]
fn to_f64(count: u64) -> f64 {
    count as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report with names given once and used by id, a call whose cost is the callee's, and positions written
    /// relative to the line before.
    #[test]
    fn a_report_gives_each_function_its_own_count() {
        let text =
            "fn=(1) honig::outer\n10 5\ncfn=(2) honig::inner\ncalls=1 0\n11 100\n+1 3\n\nfn=(2)\n0 100\nfn=(1)\n-2 2\n";
        let costs = own_costs(text);
        assert_eq!(costs.get("honig::outer"), Some(&10), "5 + 3 + 2, without the call's 100: {costs:?}");
        assert_eq!(costs.get("honig::inner"), Some(&100), "{costs:?}");
    }

    /// Reports at size `n`: a linear function and a quadratic one with `square` of the phase's cost per n^2.
    fn reports(n: u64, square: u64) -> Vec<Report> {
        let functions = [("honig::linear".to_owned(), 1000 * n), ("honig::square".to_owned(), square * n * n)];
        vec![Report { total: 1000 * n + square * n * n, functions: functions.into_iter().collect() }]
    }

    /// A function that grows as n^2 shows at once, at a share of a few percent: the linear rest does not hide it.
    #[test]
    fn a_quadratic_function_shows_at_any_share() {
        let found = steep(&[16.0, 64.0], &[reports(16, 1), reports(64, 1)], 1.3);
        let names: Vec<&str> = found.iter().map(|s| s.function.as_str()).collect();
        assert_eq!(names, ["honig::square"], "{found:?}");
        assert!(found[0].share < 0.1 && (found[0].exponent - 2.0).abs() < 1e-9, "{found:?}");
    }

    /// A function of the standard library or libc is not judged: its count jumps where a copy changes strategy.
    #[test]
    fn a_function_of_another_crate_is_not_judged() {
        let functions = [
            ("__memcpy_avx_unaligned_erms".to_owned(), 1u64),
            ("<honig::Content as serde::Deserialize>::deserialize".to_owned(), 1),
        ];
        let at = |n: u64| {
            vec![Report { total: n * n, functions: functions.iter().map(|(f, _)| (f.clone(), n * n / 2)).collect() }]
        };
        let found: Vec<String> = steep(&[16.0, 64.0], &[at(16), at(64)], 1.3).into_iter().map(|s| s.function).collect();
        assert_eq!(found, ["<honig::Content as serde::Deserialize>::deserialize"]);
    }

    /// A function below the floor of its phase is not judged: it is too small to grow for a reason.
    #[test]
    fn a_function_below_the_floor_is_not_judged() {
        let tiny = |n: u64| {
            let mut r = reports(n, 1);
            r[0].total = 100_000_000 * n;
            r
        };
        assert!(steep(&[16.0, 64.0], &[tiny(16), tiny(64)], 1.3).is_empty());
    }
}
