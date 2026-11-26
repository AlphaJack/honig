//! the honig dependency in the target crate's `Cargo.toml`.
//!
//! Generated code needs honig at exactly the version of the generator (`=x.y.z`) with the features that it
//! calls. The dependency is yours: `generate` and `check` stop and print the `cargo add` line that fixes it.

use crate::emit::rust::RUNTIME_VERSION;
use crate::error::Error;
use crate::settings::{FEATURES, Manifest, RUNTIME};

/// Checks that the crate depends on honig at the version of the generator with every feature in `needed`.
///
/// # Errors
/// The crate has no honig dependency, it requires another version or it lacks a feature in `needed`.
pub(crate) fn check_runtime(manifest: &Manifest, needed: &[&str]) -> Result<(), Error> {
    let needed = in_order(needed);
    let version = format!("={RUNTIME_VERSION}");
    let Some(dep) = manifest.runtime() else {
        return Err(Error::Runtime(format!("is missing. Run `{}`", add_line(&needed))));
    };
    if !pins(&dep.req) {
        let fix = if manifest.inherits(&dep.name) {
            format!("Set `{RUNTIME} = \"{version}\"` in [workspace.dependencies]")
        } else {
            format!("Run `cargo add {RUNTIME}@{version}`")
        };
        return Err(Error::Runtime(format!("requires `{}`, generated code needs `{version}`. {fix}", dep.req)));
    }
    let missing: Vec<&str> = needed.iter().copied().filter(|f| !dep.features.iter().any(|d| d == f)).collect();
    if !missing.is_empty() {
        return Err(Error::Runtime(format!(
            "lacks the features that generated code needs. Run `cargo add {RUNTIME} -F {}`",
            missing.join(",")
        )));
    }
    Ok(())
}

/// Does the requirement `req` admit the version of the generator and no other? `=x.y.z` does. So does a caret on a
/// `0.0.z` version, which Cargo reads as that version alone: `0.0.3` and `^0.0.3`, as `cargo add` writes them.
fn pins(req: &str) -> bool {
    let req = req.trim();
    let exact = req.strip_prefix('=').map(str::trim) == Some(RUNTIME_VERSION);
    let caret = RUNTIME_VERSION.starts_with("0.0.") && req.strip_prefix('^').unwrap_or(req).trim() == RUNTIME_VERSION;
    exact || caret
}

/// Returns `needed` in the order of honig's `[features]`, whatever order the modules found them in.
fn in_order<'f>(needed: &[&'f str]) -> Vec<&'f str> {
    FEATURES.iter().map(|(name, _)| *name).filter(|name| needed.contains(name)).collect()
}

/// Returns the `cargo add` line of honig at the version of the generator with the features in `needed`.
pub(crate) fn add_line(needed: &[&str]) -> String {
    let needed = in_order(needed);
    let features = if needed.is_empty() { String::new() } else { format!(" -F {}", needed.join(",")) };
    format!("cargo add {RUNTIME}@={RUNTIME_VERSION}{features}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error of [`check_runtime`] as text. `None` when the dependency fits.
    fn check(deps: &str, needed: &[&str]) -> Option<String> {
        let manifest = Manifest::parse(&format!("[package]\nname = \"x\"\n\n[dependencies]\n{deps}")).unwrap();
        check_runtime(&manifest, needed).err().map(|e| e.to_string())
    }

    #[test]
    fn the_runtime_needs_the_version_of_the_generator_and_the_needed_features() {
        let pinned =
            |features: &str| format!("honig = {{ version = \"={RUNTIME_VERSION}\", features = [{features}] }}");
        assert_eq!(check(&pinned("\"client\", \"simd\""), &["client"]), None, "more features are yours");
        assert_eq!(
            check(&pinned("\"client\""), &["xml", "server", "client"]).unwrap(),
            "Cargo.toml: the honig dependency lacks the features that generated code needs. Run `cargo add honig -F \
             server,xml`"
        );
        assert_eq!(
            check("honig = \"0.0.1\"", &[]).unwrap(),
            format!(
                "Cargo.toml: the honig dependency requires `0.0.1`, generated code needs `={RUNTIME_VERSION}`. Run \
                 `cargo add honig@={RUNTIME_VERSION}`"
            )
        );
        assert_eq!(
            check("", &["client", "server"]).unwrap(),
            format!(
                "Cargo.toml: the honig dependency is missing. Run `cargo add honig@={RUNTIME_VERSION} -F client,server`"
            )
        );
        assert!(check("honig = { workspace = true }", &[]).unwrap().contains("in [workspace.dependencies]"));
        // `cargo add` writes a caret: on a `0.0.z` version Cargo reads it as that version alone.
        assert_eq!(check(&format!("honig = \"{RUNTIME_VERSION}\""), &[]), None);
        assert_eq!(check(&format!("honig = \"^{RUNTIME_VERSION}\""), &[]), None);
        assert!(check(&format!("honig = \"~{RUNTIME_VERSION}\""), &[]).is_some(), "a tilde admits later patches");
        let renamed =
            format!("rt = {{ package = \"honig\", version = \"={RUNTIME_VERSION}\", features = [\"client\"] }}");
        assert_eq!(check(&renamed, &["client"]), None, "a renamed dependency counts");
    }
}
