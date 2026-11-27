//! The committed outputs of the generator: `tests/e2e` and `examples/shop`. Each test rewrites what is stale and then
//! fails, so that the change is committed. They live in a test of their own: a change to the generator then rebuilds
//! the library and this file, not the unit tests of `src/tests`.
//!
//! With `HONIG_REGEN_CHECK_ONLY` set they fail without rewriting. The mutants job sets it: it mutates in place, and the
//! output of a mutated generator left on disk would break the build of every later mutant.

#![expect(clippy::disallowed_methods, reason = "compares and rewrites the committed outputs")]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use cargo_honig::__private::{e2e_files, fixtures};

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixtures_dir() -> PathBuf {
    root().join("tests").join("fixtures")
}

fn check_only() -> bool {
    std::env::var_os("HONIG_REGEN_CHECK_ONLY").is_some()
}

/// Puts back the files of a folder as they were, when dropped. It also removes the files added since.
struct Restore {
    dir: PathBuf,
    before: Vec<(PathBuf, String)>,
}

impl Drop for Restore {
    fn drop(&mut self) {
        for (path, _) in snapshot(&self.dir) {
            if !self.before.iter().any(|(p, _)| *p == path) {
                std::fs::remove_file(&path).unwrap();
            }
        }
        for (path, text) in &self.before {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }
}

/// `tests/e2e` contains what the fixtures generate: every description as a module. A stale file is rewritten. The
/// module of a fixture that is gone is removed.
#[test]
fn e2e_is_fresh() {
    let dir = root().join("tests").join("e2e");
    let files = e2e_files(&dir, &fixtures_dir()).unwrap();
    let modules: Vec<PathBuf> =
        fixtures(&fixtures_dir()).unwrap().into_iter().map(|(module, _)| dir.join("src").join(module)).collect();
    let mut stale: Vec<String> = std::fs::read_dir(dir.join("src"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir() && !modules.contains(path))
        .map(|path| {
            if !check_only() {
                std::fs::remove_dir_all(&path).unwrap();
            }
            path.display().to_string()
        })
        .collect();
    for (path, text) in files {
        let path = dir.join(path);
        if std::fs::read_to_string(&path).ok().as_ref() != Some(&text) {
            if !check_only() {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, text).unwrap();
            }
            stale.push(path.display().to_string());
        }
    }
    assert!(stale.is_empty(), "stale files in tests/e2e: {stale:?}");
}

/// Every file of `dir` and its subfolders as (path, contents).
fn snapshot(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = vec![];
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(snapshot(&path));
        } else {
            let text = std::fs::read_to_string(&path).unwrap();
            out.push((path, text));
        }
    }
    out.sort();
    out
}

/// `examples/shop` contains what `cargo honig generate` writes for the two module tables of its `Cargo.toml`. Its
/// `openapi.yaml` is the `catalog` fixture and its `supplier.yaml` the `xml` fixture. Then `check` of every module and
/// of one module finds nothing stale, and generating one module keeps honig features the other needs.
#[test]
fn example_is_fresh() {
    let dir = root().join("examples").join("shop");
    let before = snapshot(&dir);
    let _restore = check_only().then(|| Restore { dir: dir.clone(), before: before.clone() });
    for (fixture, spec) in [("catalog", "openapi"), ("xml", "supplier")] {
        let text = std::fs::read_to_string(fixtures_dir().join("v32").join(format!("{fixture}.yaml")));
        std::fs::write(dir.join(format!("{spec}.yaml")), text.unwrap()).unwrap();
    }
    let manifest = dir.join("Cargo.toml").display().to_string();
    let run = |args: &[&str]| {
        let args = args.iter().copied().chain(["--quiet", "--manifest", &manifest]).map(String::from);
        cargo_honig::run(args)
    };
    assert_eq!(run(&["generate"]), ExitCode::SUCCESS);
    let after = snapshot(&dir);
    let stale: Vec<String> = after
        .iter()
        .filter(|file| !before.contains(file))
        .chain(before.iter().filter(|(path, _)| !after.iter().any(|(p, _)| p == path)))
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert!(stale.is_empty(), "stale files in examples/shop: {stale:?}");
    assert_eq!(run(&["check"]), ExitCode::SUCCESS);
    assert_eq!(run(&["check", "supplier"]), ExitCode::SUCCESS);
    assert_eq!(run(&["generate", "api"]), ExitCode::SUCCESS);
    assert_eq!(snapshot(&dir), after, "generating one module keeps the features the other needs");
}
