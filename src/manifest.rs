//! The runtime dependency in the target crate's `Cargo.toml`.
//!
//! Generated code needs the runtime at exactly the version of the generator (`=x.y.z`) with the features that it
//! calls. The dependency is yours: `generate` and `check` stop and print the `cargo add` line that fixes it.

use crate::emit::rust::RUNTIME_VERSION;
use crate::error::Error;
use crate::settings::{FEATURES, Manifest, RUNTIME};

/// Checks that the crate depends on the runtime at the version of the generator with every feature in `needed`.
///
/// # Errors
/// The crate has no runtime dependency, it requires another version or it lacks a feature in `needed`.
pub(crate) fn check_runtime(manifest: &Manifest, needed: &[&str]) -> Result<(), Error> {
    // In the order of the runtime's `[features]`, whatever order the modules found them in.
    let needed: Vec<&str> = FEATURES.iter().map(|(name, _)| *name).filter(|name| needed.contains(name)).collect();
    let version = format!("={RUNTIME_VERSION}");
    let Some(dep) = manifest.runtime() else {
        let features = if needed.is_empty() { String::new() } else { format!(" -F {}", needed.join(",")) };
        return Err(Error::Runtime(format!("is missing. Run `cargo add {RUNTIME}@{version}{features}`")));
    };
    if dep.req != version {
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
        let renamed =
            format!("rt = {{ package = \"honig\", version = \"={RUNTIME_VERSION}\", features = [\"client\"] }}");
        assert_eq!(check(&renamed, &["client"]), None, "a renamed dependency counts");
    }
}
