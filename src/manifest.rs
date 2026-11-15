//! The runtime crate dependency in the target crate's `Cargo.toml`.
//!
//! Generated code needs the runtime at exactly the version of the generator (`=x.y.z`). `generate` adds and removes
//! its features: the halves, the adapters that the settings list and the codecs that the descriptions call for. A
//! feature that nothing needs any more goes. Only `simd` is yours to list. The `check` command fails when the
//! dependency is out of date. Everything else in the manifest is kept as written.

use toml_edit::{Array, DocumentMut, InlineTable, Item, Value};

use crate::emit::rust::RUNTIME_VERSION;
use crate::error::Error;
use crate::settings::FEATURES;

/// Returns `text` with the runtime crate dependency at the version of the generator. Of the features that `generate`
/// owns it lists exactly `needed`. It keeps the features that are yours as you list them.
///
/// A dependency with `workspace = true` takes its version from `[workspace.dependencies]`: cargo allows only
/// `features` next to it. Then the version goes in the root ([`with_pinned_workspace`]), which is this file when it
/// is the root too.
///
/// # Errors
/// The manifest is not valid TOML, `[dependencies]` is not a table or the dependency lists a feature that the
/// runtime does not have.
pub(crate) fn with_runtime(text: &str, needed: &[&str]) -> Result<String, Error> {
    let mut doc: DocumentMut = text.parse()?;
    let deps = doc
        .entry("dependencies")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| shape("[dependencies]", "table"))?;
    let version = format!("={RUNTIME_VERSION}");
    let entry = deps.entry("honig").or_insert_with(|| Item::Value(Value::InlineTable(InlineTable::new())));
    // A bare version string becomes a table so features fit.
    if entry.is_str() {
        *entry = Item::Value(Value::InlineTable(InlineTable::new()));
    }
    let table = entry.as_table_like_mut().ok_or_else(|| shape("honig", "table"))?;
    let inherited = table.get("workspace").and_then(Item::as_bool) == Some(true);
    if !inherited && table.get("version").and_then(Item::as_str) != Some(version.as_str()) {
        table.insert("version", toml_edit::value(version));
    }
    let current: Vec<String> = table
        .get("features")
        .and_then(Item::as_array)
        .map_or_default(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    if let Some(extra) = current.iter().find(|f| !FEATURES.iter().any(|(k, _)| k == f)) {
        return Err(Error::Feature {
            feature: extra.clone(),
            why: format!("the runtime has no feature `{extra}`. Remove it"),
        });
    }
    let wanted: Vec<&str> = FEATURES
        .into_iter()
        .filter(
            |(name, trigger)| {
                if trigger.generated() { needed.contains(name) } else { current.iter().any(|f| f == name) }
            },
        )
        .map(|(name, _)| name)
        .collect();
    if current != wanted {
        if wanted.is_empty() {
            table.remove("features");
        } else {
            table.insert("features", toml_edit::value(wanted.into_iter().collect::<Array>()));
        }
    }
    if inherited && doc.get("workspace").is_some() {
        return with_pinned_workspace(&doc.to_string());
    }
    Ok(doc.to_string())
}

/// Does the crate of `text` take the runtime from `[workspace.dependencies]` (`honig = { workspace = true }`)?
pub(crate) fn inherits_runtime(text: &str) -> bool {
    text.parse::<DocumentMut>().is_ok_and(|doc| {
        doc.get("dependencies").and_then(|d| d.get("honig")).and_then(|h| h.get("workspace")).and_then(Item::as_bool)
            == Some(true)
    })
}

/// Returns the workspace root `text` with `[workspace.dependencies]` holding the runtime at the version of the
/// generator. Its other keys stay as written. Each member lists the features that it needs.
///
/// # Errors
/// The manifest is not valid TOML or `[workspace.dependencies]` is not a table.
pub(crate) fn with_pinned_workspace(text: &str) -> Result<String, Error> {
    let mut doc: DocumentMut = text.parse()?;
    let deps = doc
        .entry("workspace")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| shape("[workspace]", "table"))?
        .entry("dependencies")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| shape("[workspace.dependencies]", "table"))?;
    let version = format!("={RUNTIME_VERSION}");
    let entry = deps.entry("honig").or_insert_with(|| Item::Value(Value::InlineTable(InlineTable::new())));
    if entry.is_str() {
        *entry = Item::Value(Value::InlineTable(InlineTable::new()));
    }
    let table = entry.as_table_like_mut().ok_or_else(|| shape("[workspace.dependencies] honig", "table"))?;
    if table.get("version").and_then(Item::as_str) != Some(version.as_str()) {
        table.insert("version", toml_edit::value(version));
    }
    Ok(doc.to_string())
}

/// The comment that `init` writes over the `honig` dependency: who writes its features.
const OWNER: &str = "# features: written by `cargo honig generate` from [package.metadata.honig.*], except `simd`\n";

/// Returns `text` with a `honig` dependency that lists your `features`, unless it has one already. `init` writes it
/// with [`OWNER`] over it.
///
/// # Errors
/// The manifest is not valid TOML or `[dependencies]` is not a table.
pub(crate) fn with_new_runtime(text: &str, features: &[&str]) -> Result<String, Error> {
    let mut doc: DocumentMut = text.parse()?;
    let deps = doc
        .entry("dependencies")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| shape("[dependencies]", "table"))?;
    if !deps.contains_key("honig") {
        let mut entry = InlineTable::new();
        entry.insert("version", format!("={RUNTIME_VERSION}").into());
        entry.insert("features", Value::Array(features.iter().copied().collect()));
        deps.insert("honig", Item::Value(Value::InlineTable(entry)));
        if let Some(mut key) = deps.key_mut("honig") {
            key.leaf_decor_mut().set_prefix(OWNER);
        }
    }
    Ok(doc.to_string())
}

/// Is the runtime dependency in `text` what `needed` calls for? `None` when it is. Otherwise what differs.
#[must_use]
pub(crate) fn stale(text: &str, needed: &[&str]) -> Option<String> {
    match with_runtime(text, needed) {
        Ok(wanted) if wanted == text => None,
        Ok(_) => Some(format!("Cargo.toml (honig ={RUNTIME_VERSION} or its features)")),
        Err(e) => Some(e.to_string()),
    }
}

/// Does the workspace root `text` pin the runtime at the version of the generator? `None` when it does.
#[must_use]
pub(crate) fn stale_workspace(text: &str) -> Option<String> {
    match with_pinned_workspace(text) {
        Ok(wanted) if wanted == text => None,
        Ok(_) => Some(format!("the workspace Cargo.toml ([workspace.dependencies] honig ={RUNTIME_VERSION})")),
        Err(e) => Some(e.to_string()),
    }
}

/// Returns a manifest `entry` that is not the `kind` it must be.
fn shape(entry: &str, kind: &'static str) -> Error {
    Error::Shape { entry: entry.to_owned(), kind }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::XML;

    const PINNED: &str =
        "[package]\nname = \"x\"\n\n[dependencies]\nhonig = { version = \"=0.0.1\", features = [FEATURES] }\n";

    fn features(listed: &str, needed: &[&str]) -> Result<String, Error> {
        let text = PINNED.replace("FEATURES", listed);
        let out = with_runtime(&text, needed)?;
        assert_eq!(stale(&out, needed), None, "idempotent");
        Ok(out.split_once("features = ").map_or_default(|(_, f)| f.lines().next().unwrap_or_default().to_owned()))
    }

    #[test]
    fn generate_owns_every_feature_but_simd() {
        assert_eq!(
            features("\"simd\"", &["server", "axum", XML]).unwrap(),
            "[\"server\", \"axum\", \"xml\", \"simd\"] }"
        );
        assert_eq!(
            features("\"server\", \"axum\", \"xml\", \"multipart\", \"simd\"", &["server"]).unwrap(),
            "[\"server\", \"simd\"] }",
            "an adapter and a codec that nothing needs go, simd stays"
        );
        assert!(features("\"reqwets\"", &[]).unwrap_err().to_string().contains("no feature `reqwets`"));
    }

    #[test]
    fn a_new_crate_gets_the_dependency_and_a_bare_version_becomes_a_table() {
        let out = with_runtime("[package]\nname = \"x\"\n", &["client"]).unwrap();
        assert!(
            out.contains(&format!("honig = {{ version = \"={RUNTIME_VERSION}\", features = [\"client\"] }}")),
            "{out}"
        );
        let out = with_runtime("[dependencies]\nhonig = \"0.0.1\"\n", &[]).unwrap();
        assert!(out.contains(&format!("honig = {{ version = \"={RUNTIME_VERSION}\" }}")), "{out}");
    }

    #[test]
    fn init_says_who_writes_the_features_and_generate_keeps_it() {
        let out = with_new_runtime("[dependencies]\nserde = \"1\"\n", &["simd"]).unwrap();
        let out = with_runtime(&out, &["client"]).unwrap();
        assert_eq!(
            out,
            format!(
                "[dependencies]\nserde = \"1\"\n{OWNER}honig = {{ version = \"={RUNTIME_VERSION}\", features = [\"client\", \"simd\"] }}\n"
            )
        );
    }

    #[test]
    fn an_inherited_runtime_takes_features_here_and_its_version_in_the_root() {
        let member = "[dependencies]\nhonig = { workspace = true }\n";
        assert!(inherits_runtime(member));
        assert_eq!(
            with_runtime(member, &["client"]).unwrap(),
            "[dependencies]\nhonig = { workspace = true , features = [\"client\"] }\n"
        );
        let root = "[workspace]\nmembers = [\"shop\"]\n\n[workspace.dependencies]\nhonig = \"0.0.1\"\n";
        let pinned = with_pinned_workspace(root).unwrap();
        assert!(pinned.contains(&format!("honig = {{ version = \"={RUNTIME_VERSION}\" }}")), "{pinned}");
        assert_eq!(stale_workspace(&pinned), None);
        assert!(stale_workspace(root).is_some());
        let both = format!("{pinned}\n[package]\nname = \"shop\"\n\n{member}");
        let out = with_runtime(&both, &["client"]).unwrap();
        assert_eq!(out, with_pinned_workspace(&out).unwrap(), "a root that is the member too is pinned in place");
    }

    #[test]
    fn keeps_a_path_and_fixes_the_version() {
        let text = "[dependencies]\nhonig = { version = \"=0.0.1\", path = \"../runtime\" }\n";
        let out = with_runtime(text, &[]).unwrap();
        assert!(out.contains(&format!("version = \"={RUNTIME_VERSION}\", path = \"../runtime\"")), "{out}");
    }
}
