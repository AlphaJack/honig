//! What a bench measured on `main`, beside what it measures now.
//!
//! Each bench keeps its numbers in
//! `.bench-results/<bench>.tsv` at the root of the repository, one `key<TAB>value` per line. CI restores the file that
//! the last run on `main` left and saves it again only on `main`: a pull request loads `main` and writes nothing back.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The numbers of one bench: those that `main` left, and those of this run.
#[derive(Debug)]
pub struct Results {
    path: PathBuf,
    main: BTreeMap<String, f64>,
    now: BTreeMap<String, f64>,
}

impl Results {
    /// The results of the bench `name`: what `main` left there, if any.
    #[must_use]
    #[expect(clippy::disallowed_methods, reason = "loads the results that main left")]
    pub fn open(name: &str) -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join(".bench-results")
            .join(format!("{name}.tsv"));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let main = text
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .filter_map(|(k, v)| Some((k.to_owned(), v.parse().ok()?)))
            .collect();
        Self { path, main, now: BTreeMap::new() }
    }

    /// What `main` measured for `key`.
    #[must_use]
    pub fn main(&self, key: &str) -> Option<f64> {
        self.main.get(key).copied()
    }

    /// Keeps what this run measured for `key`.
    pub fn record(&mut self, key: &str, value: f64) {
        self.now.insert(key.to_owned(), value);
    }

    /// Writes what this run measured, for the next run to load as `main` when CI saves it.
    #[expect(clippy::disallowed_methods, reason = "writes the results of this run")]
    pub fn save(&self) {
        let text: String = self.now.iter().map(|(k, v)| format!("{k}\t{v}\n")).collect();
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(&self.path, text).ok();
    }
}

/// Returns `main` as a table shows it: `-` when `main` has no number for it yet.
#[must_use]
pub fn shown(main: Option<f64>, decimals: usize) -> String {
    main.map_or_else(|| "-".to_owned(), |m| format!("{m:.decimals$}"))
}

/// Returns how much `now` changed against `main` in percent, `-` without `main`.
#[must_use]
pub fn change(now: f64, main: Option<f64>) -> String {
    match main {
        Some(m) if m > 0.0 => format!("{:+.1}", (now / m - 1.0) * 100.0),
        _ => "-".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_signed_and_needs_main() {
        assert_eq!(change(110.0, Some(100.0)), "+10.0");
        assert_eq!(change(90.0, Some(100.0)), "-10.0");
        assert_eq!(change(90.0, None), "-");
        assert_eq!(shown(Some(1.234), 2), "1.23");
        assert_eq!(shown(None, 2), "-");
    }
}
