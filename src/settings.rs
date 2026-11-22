//! Settings from the `Cargo.toml` of the user. Each table `[package.metadata.<name>.<module>]` sets what is generated
//! into one module with the keys of [`SETTINGS`]. `<name>` is the name of this package. See [`modules`].
//!
//! The `features` of the `honig` dependency are yours. Its type features pick the types of string formats.
//! `generate` stops when the generated code needs a feature that the dependency does not list.
//!
//! Dependencies come as cargo resolves them: a `workspace = true` entry adds the features of the workspace
//! root's `[workspace.dependencies]` entry to its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, TableLike};

use crate::error::Error;
use crate::ir::types::Format;

/// Every setting of a module as (key, default, what it does). A setting without a default is required. `--help` and
/// the book show it.
pub(crate) const SETTINGS: [(&str, Option<&str>, &str); 11] = [
    (
        "descriptions",
        None,
        "the descriptions relative to Cargo.toml, like [\"openapi.yaml\"]; several, such as [\"a.yaml\", \"api/*.yaml\"], merge",
    ),
    ("output", Some("\"src/<module>\""), "the module directory"),
    (
        "mod_rs",
        Some("the crate's clippy lints, else true"),
        "true writes the root of the module as `<output>/mod.rs`, false as `<output>.rs`",
    ),
    ("client", Some("true"), "generate the client; a table sets allow_unknown_values = true"),
    ("server", Some("true"), "generate the server; a table sets allow_unimplemented = true"),
    (
        "operations",
        Some("every one"),
        "the only operations, by operationId, that the client calls and the server serves; the module contains only the \
         types that they reach",
    ),
    ("tests", Some("false"), "generate a test that the description's examples decode"),
    ("keep_unknown_fields", Some("false"), "objects without additionalProperties keep unknown keys in a map"),
    ("overlays", Some("[]"), "Overlays (1.0 to 1.2) applied in order to the spec"),
    ("formats", Some("{}"), "your own types for formats with a check, like { uuid = \"crate::Id\" }"),
    ("derives", Some("{}"), "extra derives on every type or on one type by Rust name, like { \"*\" = [\"Hash\"] }"),
];

/// Returns what `init` writes for the description `spec`: the module `api` with the settings that most crates change.
pub(crate) fn init_table(spec: &str) -> String {
    format!("[{ROOT}.{DEFAULT_MODULE}]\ndescriptions = [\"{spec}\"]\nclient = true\nserver = true\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// The settings of one module.
pub(crate) struct Settings {
    /// The table as the manifest spells it, for messages: `package.metadata.honig.<module>`.
    pub table: String,
    /// Directory containing Cargo.toml; other paths are relative to it.
    pub root: PathBuf,
    /// Spec files relative to `root`: one document or several root documents to merge. A `*` in a file name
    /// matches files of that directory.
    pub spec: Vec<PathBuf>,
    /// Module directory to generate into relative to `root`.
    pub output: PathBuf,
    /// The root of the module is `<output>/mod.rs`, else `<output>.rs`.
    pub mod_rs: bool,
    /// Generate the client.
    pub client: bool,
    /// Generate the server trait and router.
    pub server: bool,
    /// Generate a test that the description's examples decode into the generated types.
    pub tests: bool,
    /// `server = { allow_unimplemented = true }`: each `Api` method defaults to `501 Not Implemented`.
    pub allow_unimplemented: bool,
    /// Extra derives as (Rust type name or `*`, derive paths).
    pub derives: Vec<(String, Vec<String>)>,
    /// `keep_unknown_fields = true`.
    pub keep_unknown: bool,
    /// `false` with `client = { allow_unknown_values = true }`: response types accept an enum value or a property that
    /// the description does not list. Requests are always strict.
    pub strict_client: bool,
    /// The crate has no library target (no `[lib]` and no `src/lib.rs`). The module is visible to the crate only.
    pub binary: bool,
    /// `operations = [...]`: the only operations that the client calls and the server serves, by `operationId`. `None`
    /// for every one.
    pub operations: Option<Vec<String>>,
    /// `overlays`: Overlay files applied in order to the spec. They are relative to `root`.
    pub overlays: Vec<PathBuf>,
}

impl Settings {
    /// Returns absolute paths of the spec files with each `*` pattern expanded in name order.
    ///
    /// # Errors
    /// A pattern matches no file or its directory cannot be listed.
    pub(crate) fn spec_paths(&self) -> Result<Vec<PathBuf>, Error> {
        let mut out = vec![];
        for spec in &self.spec {
            let path = self.root.join(spec);
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            let Some((prefix, suffix)) = name.split_once('*') else {
                out.push(path);
                continue;
            };
            let dir = path.parent().map_or_else(|| self.root.clone(), Path::to_path_buf);
            let found: Vec<PathBuf> =
                crate::disk::files_in(&dir)?.into_iter().filter(|p| matches_star(p, prefix, suffix)).collect();
            if found.is_empty() {
                return Err(setting(
                    &self.table,
                    "descriptions",
                    "patterns that match a file",
                    spec.display().to_string(),
                ));
            }
            out.extend(found);
        }
        Ok(out)
    }

    /// Returns absolute paths of the overlays in the order that they apply.
    pub(crate) fn overlay_paths(&self) -> Vec<PathBuf> {
        self.overlays.iter().map(|o| self.root.join(o)).collect()
    }

    /// Returns absolute output directory.
    pub(crate) fn output_path(&self) -> PathBuf {
        self.root.join(&self.output)
    }

    /// Returns the absolute path of the root of the module: `<output>/mod.rs` or `<output>.rs`.
    pub(crate) fn root_path(&self) -> PathBuf {
        if self.mod_rs {
            self.output_path().join(crate::disk::ROOT_FILE)
        } else {
            self.output_path().with_extension("rs")
        }
    }
}

/// A crate's `Cargo.toml` as cargo resolves it (`cargo metadata`): its name, its dependencies with the features that
/// `workspace = true` inherits, its targets and its workspace. Its `[package.metadata.honig.*]` tables are parsed as
/// written, so a message can point to the table and the key.
#[derive(Debug, Clone, Default)]
pub(crate) struct Manifest {
    doc: DocumentMut,
    /// `None` for a virtual workspace manifest.
    name: Option<String>,
    /// `[dependencies]` and the `[target.<cfg>.dependencies]` tables, not the dev or build ones.
    deps: Vec<Dependency>,
    /// The package has a library target.
    lib: bool,
    /// The manifests of the workspace members when this is a virtual workspace manifest.
    members: Option<Vec<PathBuf>>,
    /// The workspace root's manifest when it is another file than this one: its `[workspace.lints]` apply to a
    /// package with `lints.workspace = true`.
    workspace_doc: Option<DocumentMut>,
}

/// One dependency as cargo sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Dependency {
    /// The name as written: the key of its entry.
    pub name: String,
    /// The package: the name unless `package = "..."` renames it.
    pub package: String,
    /// Its `features`, with those of the workspace entry under `workspace = true`.
    pub features: Vec<String>,
    /// Its version requirement, such as `=0.0.3`. `*` without one.
    pub req: String,
}

impl Manifest {
    /// Returns the manifest at `path`, as `cargo metadata` resolves it in its workspace.
    ///
    /// # Errors
    /// The manifest cannot be opened, is not valid TOML, or cargo rejects it.
    pub(crate) fn load(path: &Path) -> Result<Self, Error> {
        let doc: DocumentMut = crate::disk::read(path)?.parse()?;
        let metadata = crate::disk::cargo_metadata(path)?;
        let here = crate::disk::canonical(path.to_path_buf());
        let manifest_of =
            |p: &cargo_metadata::Package| crate::disk::canonical(p.manifest_path.clone().into_std_path_buf());
        let root = metadata.workspace_root.join("Cargo.toml").into_std_path_buf();
        let workspace_path = (crate::disk::canonical(root.clone()) != here).then_some(root);
        let workspace_doc = match &workspace_path {
            Some(p) => Some(crate::disk::read(p)?.parse()?),
            None => None,
        };
        let Some(package) = metadata.packages.iter().find(|p| manifest_of(p) == here) else {
            let members = metadata
                .workspace_packages()
                .into_iter()
                .map(|p| p.manifest_path.clone().into_std_path_buf())
                .collect();
            return Ok(Self { members: Some(members), ..Self::default() });
        };
        let deps = package
            .dependencies
            .iter()
            .filter(|d| d.kind == cargo_metadata::DependencyKind::Normal)
            .map(|d| Dependency {
                name: d.rename.clone().unwrap_or_else(|| d.name.clone()),
                package: d.name.clone(),
                features: d.features.clone(),
                req: d.req.to_string(),
            })
            .collect();
        Ok(Self {
            doc,
            name: Some(package.name.to_string()),
            deps,
            lib: package.targets.iter().any(cargo_metadata::Target::is_lib),
            members: None,
            workspace_doc,
        })
    }

    /// Returns what the crate's clippy lints ask of the root of a module: `Some(false)` when `mod_module_files` is on,
    /// `Some(true)` when `self_named_module_files` is on. The package's `[lints.clippy]` is used, or the
    /// `[workspace.lints.clippy]` of the workspace root under `lints.workspace = true`.
    pub(crate) fn linted_mod_rs(&self) -> Option<bool> {
        let lints = self.doc.get("lints");
        let inherited = lints.and_then(|l| l.get("workspace")).and_then(Item::as_bool) == Some(true);
        let clippy = if inherited {
            self.workspace_doc.as_ref().unwrap_or(&self.doc).get("workspace")?.get("lints")?.get("clippy")?
        } else {
            lints?.get("clippy")?
        };
        // A level is `"deny"` or `{ level = "deny", priority = 1 }`.
        let on = |lint: &str| {
            let item = clippy.get(lint);
            let level = item.and_then(|i| i.as_str().or_else(|| i.get("level")?.as_str()));
            level.is_some_and(|l| l != "allow")
        };
        match (on("mod_module_files"), on("self_named_module_files")) {
            (true, false) => Some(false),
            (false, true) => Some(true),
            (true, true) | (false, false) => None,
        }
    }

    /// `text` alone: its own `[dependencies]` and target dependency tables, no workspace. Tests build manifests so.
    #[cfg(test)]
    pub(crate) fn parse(text: &str) -> Result<Self, Error> {
        let doc: DocumentMut = text.parse()?;
        let targets = doc.get("target").and_then(Item::as_table_like).into_iter().flat_map(|t| t.iter());
        let tables = std::iter::once(doc.get("dependencies")).chain(targets.map(|(_, t)| t.get("dependencies")));
        let mut deps = vec![];
        for table in tables.flatten().filter_map(Item::as_table_like) {
            for (name, item) in table.iter() {
                let entry = item.as_table_like();
                deps.push(Dependency {
                    name: name.to_owned(),
                    package: entry.and_then(|e| e.get("package")?.as_str()).unwrap_or(name).to_owned(),
                    features: string_array(entry.and_then(|e| e.get("features"))).unwrap_or_default(),
                    req: item.as_str().or_else(|| entry?.get("version")?.as_str()).unwrap_or("*").to_owned(),
                });
            }
        }
        let name = doc.get("package").and_then(|p| p.get("name")?.as_str()).map(str::to_owned);
        Ok(Self { lib: doc.contains_key("lib"), doc, name, deps, ..Self::default() })
    }

    /// Returns the name of the package. `None` for a virtual workspace manifest.
    pub(crate) fn package_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the manifests of the workspace members when this is a virtual workspace manifest, as cargo lists them.
    /// `None` for the manifest of a package.
    pub(crate) fn members(&self) -> Option<&[PathBuf]> {
        self.members.as_deref()
    }

    /// Returns whether the package has a library target.
    pub(crate) fn has_lib(&self) -> bool {
        self.lib
    }

    /// Returns `[package.metadata.honig]` when there is one.
    pub(crate) fn table(&self) -> Option<&dyn TableLike> {
        self.doc.get("package")?.get("metadata")?.get("honig")?.as_table_like()
    }

    /// Returns the dependency written as `name`.
    pub(crate) fn dependency(&self, name: &str) -> Option<&Dependency> {
        self.deps.iter().find(|d| d.name == name)
    }

    /// Returns the names of every dependency, as written and as its `package`: `x-rust-type` refers to a crate by
    /// package.
    pub(crate) fn dependency_names(&self) -> Vec<String> {
        let mut out = vec![];
        for d in &self.deps {
            for name in [&d.name, &d.package] {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
        }
        out
    }

    /// Returns the dependency on the runtime, by package so a renamed one counts too.
    pub(crate) fn runtime(&self) -> Option<&Dependency> {
        self.deps.iter().find(|d| d.package == crate::manifest::RUNTIME)
    }

    /// Does the dependency `name` take its version from `[workspace.dependencies]` (`workspace = true`)?
    pub(crate) fn inherits(&self, name: &str) -> bool {
        let entry = self.doc.get("dependencies").and_then(|d| d.get(name));
        entry.and_then(|e| e.get("workspace")).and_then(Item::as_bool) == Some(true)
    }

    /// Does the crate depend on `name`?
    pub(crate) fn has_dependency(&self, name: &str) -> bool {
        self.dependency(name).is_some()
    }
}

/// Returns whether the file name of `path` is `prefix`, then anything, then `suffix`: a name with one `*`.
fn matches_star(path: &Path, prefix: &str, suffix: &str) -> bool {
    path.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n.len() >= prefix.len() + suffix.len() && n.starts_with(prefix) && n.ends_with(suffix)
    })
}

/// The table that contains one table per module.
const ROOT: &str = "package.metadata.honig";

/// The module that `init` writes and that tests use: `[package.metadata.honig.api]`, generated into `src/api`.
pub(crate) const DEFAULT_MODULE: &str = "api";

/// One module: the table `[package.metadata.honig.<name>]`.
#[derive(Clone, Copy)]
pub(crate) struct Module<'a> {
    manifest: &'a Manifest,
    name: &'a str,
    /// `None` when the manifest has no such table: every setting gets its default.
    table: Option<&'a dyn TableLike>,
}

impl<'a> Module<'a> {
    fn get(&self, key: &str) -> Option<&'a Item> {
        self.table?.get(key)
    }

    /// Returns the table as the manifest spells it, for messages.
    pub(crate) fn path(&self) -> String {
        format!("{ROOT}.{}", self.name)
    }

    /// Returns the module name: the name of its table.
    pub(crate) fn name(&self) -> &'a str {
        self.name
    }
}

impl<'a> From<&'a Manifest> for Module<'a> {
    /// Returns the default module, [`DEFAULT_MODULE`].
    fn from(manifest: &'a Manifest) -> Self {
        let table = manifest.table().and_then(|t| t.get(DEFAULT_MODULE)?.as_table_like());
        Self { manifest, name: DEFAULT_MODULE, table }
    }
}

/// Returns the modules that `manifest` sets, in the order that it writes them: one per table in
/// `[package.metadata.honig]`. A setting written there itself is an error that says where it goes.
///
/// # Errors
/// `path` has no `[package.metadata.honig]` or it has no module. A key there is not a table. A module name is not a
/// Rust identifier or is a keyword.
pub(crate) fn modules<'a>(manifest: &'a Manifest, path: &Path) -> Result<Vec<Module<'a>>, Error> {
    let table = manifest.table().ok_or_else(|| Error::NoTable(path.to_path_buf()))?;
    let mut out = vec![];
    for (name, item) in table.iter() {
        let Some(module_table) = item.as_table_like().filter(|_| !keys().any(|k| k == name)) else {
            let problem = if keys().any(|k| k == name) {
                format!("`{name}` is a setting of one module. Move it to [{ROOT}.{DEFAULT_MODULE}] or another module")
            } else if FEATURES.iter().any(|(f, _)| *f == name) {
                format!("`{name}` is a feature of the runtime. List it in the `features` of the `honig` dependency")
            } else {
                format!("`{name}` is not a module table. Write [{ROOT}.{name}] with its `descriptions`")
            };
            return Err(Error::Module { table: ROOT.to_owned(), problem });
        };
        let module = Module { manifest, name, table: Some(module_table) };
        let fix = "Name the table like a module, such as `billing`";
        let problem = if !crate::text::is_identifier(name) {
            format!("`{name}` is not a Rust identifier. {fix}")
        } else if crate::text::KEYWORDS.contains(&name) {
            format!("`{name}` is a Rust keyword. {fix}")
        } else {
            out.push(module);
            continue;
        };
        return Err(Error::Module { table: module.path(), problem });
    }
    if out.is_empty() {
        let problem = format!("has no module. Add [{ROOT}.{DEFAULT_MODULE}] with its `descriptions`");
        return Err(Error::Module { table: ROOT.to_owned(), problem });
    }
    Ok(out)
}

/// Fails unless `only` is one of `modules`. `None` selects every module.
///
/// # Errors
/// `only` is no module.
pub(crate) fn ensure_module(modules: &[Module<'_>], only: Option<&str>) -> Result<(), Error> {
    let Some(only) = only else { return Ok(()) };
    if modules.iter().any(|m| m.name == only) {
        return Ok(());
    }
    let names: Vec<String> = modules.iter().map(|m| format!("`{}`", m.name)).collect();
    let problem = format!("has no module `{only}`. The modules are {}", names.join(", "));
    Err(Error::Module { table: ROOT.to_owned(), problem })
}

/// Returns `item` as the manifest writes it for an error message.
fn shown(item: &Item) -> String {
    item.to_string().trim().to_owned()
}

/// Returns the strings of a TOML array. `None` when `item` is absent, not an array or contains anything but strings.
fn string_array(item: Option<&Item>) -> Option<Vec<String>> {
    item?.as_array()?.iter().map(|v| v.as_str().map(str::to_owned)).collect()
}

/// Returns the settings of `module` in the manifest at `path`.
///
/// # Errors
/// It has no table or a setting has the wrong kind of value.
pub(crate) fn settings<'a>(module: impl Into<Module<'a>>, path: &Path) -> Result<Settings, Error> {
    let module = module.into();
    let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let table = module.table.ok_or_else(|| Error::NoTable(path.to_path_buf()))?;
    let name = module.path();
    let string = |k: &str| match table.get(k) {
        None => Ok(None),
        Some(item) => {
            item.as_str().map(|s| Some(s.to_owned())).ok_or_else(|| setting(&name, k, "a string", shown(item)))
        }
    };
    let flag = |k: &str, default: bool| match table.get(k) {
        None => Ok(default),
        Some(item) => item.as_bool().ok_or_else(|| setting(&name, k, "true or false", shown(item))),
    };
    if let Some((key, _)) = table.iter().find(|(k, _)| !keys().any(|known| known == *k)) {
        if FEATURES.iter().any(|(f, _)| *f == key) {
            let problem =
                format!("`{key}` is a feature of the runtime. List it in the `features` of the `honig` dependency");
            return Err(Error::Module { table: name, problem });
        }
        return Err(Error::UnknownSetting { table: name, key: key.to_owned(), known: keys().collect() });
    }
    let manifest = module.manifest;
    let binary = !manifest.has_lib();
    let wanted = "a list of paths like [\"openapi.yaml\"]";
    let spec: Vec<PathBuf> = match table.get("descriptions") {
        None => return Err(setting(&name, "descriptions", wanted, "nothing".to_owned())),
        Some(item) => match string_array(Some(item)) {
            Some(many) if !many.is_empty() => many.into_iter().map(PathBuf::from).collect(),
            _ => return Err(setting(&name, "descriptions", wanted, shown(item))),
        },
    };
    named_types(module)?;
    let output = string("output")?.unwrap_or_else(|| format!("src/{}", module.name));
    let mod_rs = match table.get("mod_rs") {
        None => manifest.linted_mod_rs().unwrap_or(true),
        Some(item) => item.as_bool().ok_or_else(|| setting(&name, "mod_rs", "true or false", shown(item)))?,
    };
    let Halves { client, server, allow_unimplemented, strict_client } = halves(table, &name)?;
    let operations = table
        .get("operations")
        .map(|item| {
            string_array(Some(item)).ok_or_else(|| setting(&name, "operations", "a list of operationIds", shown(item)))
        })
        .transpose()?;
    Ok(Settings {
        table: name.clone(),
        root,
        spec,
        output: output.into(),
        mod_rs,
        client,
        server,
        tests: flag("tests", false)?,
        allow_unimplemented,
        derives: derives(table, &name)?,
        binary,
        operations,
        strict_client,
        overlays: match table.get("overlays") {
            None => vec![],
            Some(item) => string_array(Some(item))
                .ok_or_else(|| setting(&name, "overlays", "a list of paths like [\"fix.overlay.yaml\"]", shown(item)))?
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        },
        keep_unknown: flag("keep_unknown_fields", false)?,
    })
}

/// What `client` and `server` set.
struct Halves {
    client: bool,
    server: bool,
    allow_unimplemented: bool,
    strict_client: bool,
}

/// Returns `client` and `server` of the module table `name`.
fn halves(table: &dyn TableLike, name: &str) -> Result<Halves, Error> {
    let (client, client_table) = half(table, name, "client", &["allow_unknown_values"])?;
    let (server, server_table) = half(table, name, "server", &["allow_unimplemented"])?;
    let allow_unimplemented = match server_table.and_then(|t| t.get("allow_unimplemented")) {
        None => false,
        Some(item) => item
            .as_bool()
            .ok_or_else(|| setting(&format!("{name}.server"), "allow_unimplemented", "true or false", shown(item)))?,
    };
    let strict_client = match client_table.and_then(|t| t.get("allow_unknown_values")) {
        None => true,
        Some(item) => !item
            .as_bool()
            .ok_or_else(|| setting(&format!("{name}.client"), "allow_unknown_values", "true or false", shown(item)))?,
    };
    Ok(Halves { client, server, allow_unimplemented, strict_client })
}

/// Returns the half `key` (`client` or `server`): whether it is generated and its table of settings. It is `true`,
/// `false` or a table with some of `keys`, which generates it.
fn half<'a>(
    table: &'a dyn TableLike,
    module: &str,
    key: &str,
    keys: &[&'static str],
) -> Result<(bool, Option<&'a dyn TableLike>), Error> {
    let Some(item) = table.get(key) else { return Ok((true, None)) };
    if let Some(on) = item.as_bool() {
        return Ok((on, None));
    }
    let wanted = format!("true, false or a table with {}", keys.join(", "));
    let settings = item.as_table_like().ok_or_else(|| setting(module, key, wanted, shown(item)))?;
    if let Some((unknown, _)) = settings.iter().find(|(k, _)| !keys.contains(k)) {
        return Err(Error::UnknownSetting {
            table: format!("{module}.{key}"),
            key: unknown.to_owned(),
            known: keys.to_vec(),
        });
    }
    Ok((true, Some(settings)))
}

/// Returns the `derives` table: `Type = ["Path", ...]` with `"*"` for all.
fn derives(table: &dyn TableLike, module: &str) -> Result<Vec<(String, Vec<String>)>, Error> {
    let name = &format!("{module}.derives");
    let Some(derives) = table.get("derives").and_then(Item::as_table_like) else { return Ok(vec![]) };
    let mut out = vec![];
    for (k, v) in derives.iter() {
        let list = string_array(Some(v))
            .ok_or_else(|| setting(name, k, "a list of derive paths, like [\"Hash\"]", shown(v)))?;
        for path in &list {
            let ok = !path.is_empty() && path.split("::").all(crate::text::is_identifier);
            if !ok {
                return Err(setting(name, k, "derive paths like `sqlx::FromRow`", format!("`{path}`")));
            }
        }
        out.push((k.to_owned(), list));
    }
    Ok(out)
}

/// Returns a setting of `table` containing something other than what it must.
fn setting(table: &str, key: &str, expected: impl Into<String>, found: String) -> Error {
    Error::Setting { table: table.to_owned(), key: key.to_owned(), expected: expected.into(), found }
}

/// Which generated half an adapter serves. The book says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// HTTP transports the client sends through.
    Client,
    /// Servers the generated router plugs into.
    Server,
    /// Logging both halves use.
    Both,
}

/// When the generated code needs a runtime feature. Every feature is yours to list. `generate` stops when one that the
/// code needs is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// Needed: the crate generates the client or the server.
    Half(Side),
    /// Optional: an adapter for the library of the same name.
    Library(Side),
    /// Needed: a codec or a check. The description has a body, parameter or scheme that it parses. Or it has a format
    /// that it checks and that has no type of its own.
    Description,
    /// Optional: the types of string formats.
    Types,
    /// Optional.
    Chosen,
}

/// Every runtime feature and when the generated code needs it.
///
/// The book's table of features shows this one.
pub(crate) const FEATURES: [(&str, Trigger); 22] = [
    (CLIENT, Trigger::Half(Side::Client)),
    (SERVER, Trigger::Half(Side::Server)),
    ("reqwest", Trigger::Library(Side::Client)),
    ("reqwest-middleware", Trigger::Library(Side::Client)),
    ("ureq", Trigger::Library(Side::Client)),
    ("gloo-net", Trigger::Library(Side::Client)),
    ("axum", Trigger::Library(Side::Server)),
    ("tracing", Trigger::Library(Side::Server)),
    ("tracing-opentelemetry", Trigger::Library(Side::Both)),
    (BASE64, Trigger::Description),
    (COOKIE, Trigger::Description),
    (IDN, Trigger::Description),
    (JIFF, Trigger::Types),
    (UUID, Trigger::Types),
    (URL, Trigger::Types),
    (IPNET, Trigger::Types),
    (MULTIPART, Trigger::Description),
    (OAUTH, Trigger::Description),
    (PKCE, Trigger::Description),
    (REGRESS, Trigger::Description),
    (XML, Trigger::Description),
    ("simd", Trigger::Chosen),
];

/// The generated client's half of the runtime.
pub(crate) const CLIENT: &str = "client";
/// The generated server's half of the runtime.
pub(crate) const SERVER: &str = "server";
/// `Cookie` parameters and API keys.
pub(crate) const COOKIE: &str = "cookie";
/// `email`, `idn-email`, `hostname` and `idn-hostname` checks (IDNA2008 and punycode).
pub(crate) const IDN: &str = "idn";
/// `ipv4-cidr` and `ipv6-cidr` as `ipnet` networks.
pub(crate) const IPNET: &str = "ipnet";
/// Dates and times as `jiff` types.
pub(crate) const JIFF: &str = "jiff";
/// `multipart/*` bodies.
pub(crate) const MULTIPART: &str = "multipart";
/// Binary data in JSON and `http basic` credentials: base64.
pub(crate) const BASE64: &str = "base64";
/// The `OAuth` 2.0 flows of the client.
pub(crate) const OAUTH: &str = "oauth";
/// The OAuth 2.0 authorization code flow. The client starts it with a PKCE challenge.
pub(crate) const PKCE: &str = "pkce";
/// `pattern` and `patternProperties` checks.
pub(crate) const REGRESS: &str = "regress";
/// `format: uri` as `url::Url`.
pub(crate) const URL: &str = "url";
/// `format: uuid` as `uuid::Uuid`.
pub(crate) const UUID: &str = "uuid";
/// XML bodies and parts.
pub(crate) const XML: &str = "xml";

/// Returns the keys of the table of a module.
fn keys() -> impl Iterator<Item = &'static str> {
    SETTINGS.into_iter().map(|(key, _, _)| key)
}

/// A type that the `formats` table sets for a format: a path such as `crate::Stamp`. Only a path of Rust identifiers is
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NamedType(String);

impl NamedType {
    /// Returns the type that `path` refers to. `None` when it is not a path of identifiers.
    pub(crate) fn new(path: &str) -> Option<Self> {
        let segments = path.strip_prefix("::").unwrap_or(path);
        segments.split("::").all(crate::text::is_identifier).then(|| Self(path.to_owned()))
    }

    /// Returns the path as the table writes it.
    pub(crate) fn path(&self) -> &str {
        &self.0
    }
}

/// The types of string formats: the type features of the runtime and the `formats` table. A format with neither is its
/// checked text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FormatLibs {
    /// `uuid`: `format: uuid` as `uuid::Uuid`.
    pub uuid: bool,
    /// `url`: `format: uri` as `url::Url`.
    pub url: bool,
    /// `ipnet`: `format: ipv4-cidr` and `ipv6-cidr` as networks.
    pub ipnet: bool,
    /// `jiff`: dates and times.
    pub jiff: bool,
    /// The types that the table sets for formats. Each wins over a feature for its format.
    pub named: BTreeMap<Format, NamedType>,
}

/// Returns the types of string formats: the type features that the `honig` dependency lists and the `formats` table.
///
/// # Errors
/// The table has a value of the wrong kind or a key that is no format with a check.
pub(crate) fn format_libs<'a>(module: impl Into<Module<'a>>) -> Result<FormatLibs, Error> {
    let module = module.into();
    Ok(FormatLibs { named: named_types(module)?, ..feature_libs(module.manifest) })
}

/// Returns the types that the type features of the `honig` dependency give string formats.
pub(crate) fn feature_libs(manifest: &Manifest) -> FormatLibs {
    let on = |feature: &str| manifest.runtime().is_some_and(|d| d.features.iter().any(|f| f == feature));
    FormatLibs { uuid: on(UUID), url: on(URL), ipnet: on(IPNET), jiff: on(JIFF), named: BTreeMap::new() }
}

/// Returns the types that the `formats` table sets, by format.
fn named_types(module: Module<'_>) -> Result<BTreeMap<Format, NamedType>, Error> {
    let mut named = BTreeMap::new();
    let Some(item) = module.get("formats") else { return Ok(named) };
    let table = item.as_table_like().ok_or_else(|| setting(&module.path(), "formats", "a table", shown(item)))?;
    let formats = &format!("{}.formats", module.path());
    for (key, value) in table.iter() {
        let Some(format) = Format::ALL.into_iter().find(|f| f.name() == key && f.marker().is_some()) else {
            let found = if FEATURES.iter().any(|(f, _)| *f == key) {
                format!("`{key}`, a feature of the runtime: list it in the `features` of the `honig` dependency")
            } else {
                "an unknown setting".to_owned()
            };
            return Err(setting(formats, key, "a format with a check, such as date-time", found));
        };
        let text = value
            .as_str()
            .ok_or_else(|| setting(formats, key, "the path of a type such as \"crate::Stamp\"", shown(value)))?;
        let ty =
            NamedType::new(text).ok_or_else(|| setting(formats, key, "the path of a type", format!("`{text}`")))?;
        named.insert(format, ty);
    }
    Ok(named)
}

#[cfg(test)]
#[expect(clippy::disallowed_methods, reason = "tests set up files of their own")]
mod tests {
    use super::*;

    fn parse(text: &str) -> Manifest {
        Manifest::parse(text).unwrap()
    }

    #[test]
    fn init_writes_the_defaults() {
        assert_eq!(
            init_table("openapi.yaml"),
            "[package.metadata.honig.api]\ndescriptions = [\"openapi.yaml\"]\nclient = true\nserver = true\n"
        );
        let read = |tables: &str| {
            let path = Path::new("Cargo.toml");
            settings(modules(&parse(tables), path).unwrap()[0], path).unwrap()
        };
        let (example, defaults) = (
            read(&init_table("openapi.yaml")),
            read("[package.metadata.honig.api]\ndescriptions = [\"openapi.yaml\"]"),
        );
        let shown =
            |c: Settings| (c.spec, c.output, c.client, c.server, c.tests, c.allow_unimplemented, c.keep_unknown);
        assert_eq!(shown(example), shown(defaults));
    }

    fn libs(formats: &str, features: &str) -> Result<FormatLibs, Error> {
        format_libs(&parse(&format!(
            "[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\n{formats}\n[dependencies]\n\
             honig = {{ version = \"=0.0.1\", features = [{features}] }}\n"
        )))
    }

    #[test]
    fn the_type_features_of_the_runtime_pick_the_types() {
        assert_eq!(libs("", "").unwrap(), FormatLibs::default(), "without a type feature a format is its text");
        assert_eq!(
            libs("", "\"client\", \"uuid\", \"jiff\"").unwrap(),
            FormatLibs { uuid: true, jiff: true, ..FormatLibs::default() }
        );
        let all = libs("", "\"uuid\", \"url\", \"ipnet\", \"jiff\"").unwrap();
        assert_eq!(all, FormatLibs { uuid: true, url: true, ipnet: true, jiff: true, ..FormatLibs::default() });
        let renamed = parse("[dependencies]\nrt = { package = \"honig\", version = \"1\", features = [\"uuid\"] }\n");
        assert!(feature_libs(&renamed).uuid, "a renamed dependency counts");
    }

    #[test]
    fn the_formats_table_sets_your_own_types() {
        let named = |libs: &FormatLibs, f: Format| libs.named.get(&f).map(|t| t.path().to_owned());
        let stamp = libs("formats = { date-time = \"crate::Stamp\" }", "").unwrap();
        assert_eq!(named(&stamp, Format::DateTime).as_deref(), Some("crate::Stamp"));
        // A type of yours wins over a feature for its format only.
        let mixed = libs("formats = { date-time = \"crate::Stamp\" }", "\"jiff\"").unwrap();
        assert_eq!(named(&mixed, Format::DateTime).as_deref(), Some("crate::Stamp"));
        assert_eq!((named(&mixed, Format::Date), mixed.jiff), (None, true));
        let ids = libs("formats = { uuid = \"crate::ids::OrderId\" }", "").unwrap();
        assert_eq!((named(&ids, Format::Uuid).as_deref(), ids.uuid), (Some("crate::ids::OrderId"), false));
        let error = |formats: &str| libs(formats, "").unwrap_err().to_string();
        assert!(error("formats = { uuid = \"not a type(\" }").contains("`uuid`"), "the error points to the key");
        assert!(error("formats = { date = 1 }").contains("the path of a type"));
        assert!(error("formats = { date = \"jiff::Zoned<jiff::Tz>\" }").contains("the path of a type"));
        assert!(error("formats = [\"uuid\"]").contains("a table"));
        // `time` accepts only valid values: no check to keep, so no type to set.
        assert!(error("formats = { time = \"crate::T\" }").contains("an unknown setting"));
        assert!(error("formats = { uuid = true }").contains("the path of a type"), "a type feature is no setting here");
        assert!(error("formats = { jiff = true }").contains("a feature of the runtime: list it in the `features`"));
    }

    #[test]
    fn keep_unknown_fields_is_a_flag() {
        let one =
            |module: &str| modules_of(&format!("[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\n{module}"));
        assert!(!one("").unwrap()[0].keep_unknown, "dropped unless set");
        assert!(one("keep_unknown_fields = true\n").unwrap()[0].keep_unknown);
        assert!(one("keep_unknown_fields = \"keep\"\n").unwrap_err().to_string().contains("true or false"));
        assert!(one("unknown_fields = \"keep\"\n").unwrap_err().to_string().contains("no setting `unknown_fields`"));
    }

    #[test]
    fn a_typo_is_an_error() {
        let err = |module: &str| {
            modules_of(&format!("[package.metadata.honig.api]\n{module}"))
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        };
        assert!(
            err("descriptions = [\"a.yaml\"]\nstrcit = false").contains("no setting `strcit`"),
            "a typo is not ignored"
        );
        assert_eq!(err("descriptions = [\"a.yaml\"]"), "");
    }

    /// A workspace as cargo resolves it: a virtual root with `crates/*` and an excluded member. `shop` inherits `uuid`
    /// with `serde` from the root, renames `jiff` and depends on `url` for one target only.
    #[test]
    fn cargo_resolves_the_workspace_and_its_dependencies() {
        let dir = std::env::temp_dir().join(format!("honig-metadata-{}", std::process::id()));
        let member = |name: &str, deps: &str| {
            let at = dir.join("crates").join(name);
            std::fs::create_dir_all(at.join("src")).unwrap();
            std::fs::write(at.join("src").join("lib.rs"), "").unwrap();
            let text = format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n{deps}");
            std::fs::write(at.join("Cargo.toml"), text).unwrap();
            at.join("Cargo.toml")
        };
        let shop = member(
            "shop",
            "[dependencies]\nuuid = { workspace = true }\ndates = { package = \"jiff\", version = \"0.2\" }\n\
             [target.'cfg(unix)'.dependencies]\nurl = \"2\"\n[dev-dependencies]\nregress = \"0.10\"\n",
        );
        member("old", "");
        let root = dir.join("Cargo.toml");
        std::fs::write(
            &root,
            "[workspace]\nmembers = [\"crates/*\"]\nexclude = [\"crates/old\"]\nresolver = \"3\"\n\n\
             [workspace.dependencies]\nuuid = { version = \"1\", features = [\"v7\", \"serde\"] }\n",
        )
        .unwrap();
        let workspace = Manifest::load(&root).unwrap();
        assert_eq!(workspace.package_name(), None);
        assert_eq!(workspace.members().map(<[PathBuf]>::len), Some(1), "`exclude` drops `old`");
        let m = Manifest::load(&shop).unwrap();
        assert_eq!(m.package_name(), Some("shop"));
        let uuid = m.dependency("uuid").unwrap();
        assert_eq!(uuid.features, ["v7", "serde"], "`workspace = true` brings the root's features");
        assert_eq!(uuid.req, "^1", "and its version");
        assert!(m.has_dependency("url") && !m.has_dependency("regress"), "target dependencies count, dev ones not");
        assert_eq!(m.dependency_names(), ["dates", "jiff", "uuid", "url"], "in the order cargo lists them");
        assert!(m.has_lib());
        std::fs::remove_dir_all(dir).ok();
    }

    /// The settings of `tables` under `[package.metadata.honig]`, or the error.
    fn modules_of(tables: &str) -> Result<Vec<Settings>, Error> {
        let text = format!("[package]\nname = \"x\"\n\n{tables}\n[dependencies]\naxum = \"0.8\"\nreqwest = \"0.13\"\n");
        let m = parse(&text);
        let path = Path::new("Cargo.toml");
        modules(&m, path).and_then(|ms| ms.into_iter().map(|module| settings(module, path)).collect())
    }

    #[test]
    fn derives_and_overlays_are_read_in_order() {
        let configs = modules_of(
            "[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\noverlays = [\"one.yaml\", \"fix/two.yaml\"]\n\
             derives = { \"*\" = [\"Hash\"], Pet = [\"sqlx::FromRow\", \"Eq\"] }\n",
        )
        .unwrap();
        let c = &configs[0];
        let derive = |k: &str, v: &[&str]| (k.to_owned(), v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(c.derives, [derive("*", &["Hash"]), derive("Pet", &["sqlx::FromRow", "Eq"])]);
        assert_eq!(c.overlay_paths(), [c.root.join("one.yaml"), c.root.join("fix").join("two.yaml")]);
    }

    #[test]
    fn a_derive_must_be_a_path() {
        for bad in ["\"\"", "\"not a path\"", "\"a::\"", "\"Hash\", 1"] {
            let tables =
                format!("[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\nderives = {{ Pet = [{bad}] }}\n");
            let err = modules_of(&tables).unwrap_err().to_string();
            assert!(err.contains("derives") && err.contains("Pet"), "{bad}: {err}");
        }
    }

    #[test]
    fn a_star_matches_names_long_enough_for_both_ends() {
        assert!(matches_star(Path::new("billing.yaml"), "bill", ".yaml"));
        assert!(matches_star(Path::new(".yaml"), "", ".yaml"));
        assert!(!matches_star(Path::new("billing.yml"), "bill", ".yaml"));
        assert!(!matches_star(Path::new("a"), "a", "a"), "the ends may not overlap");
        assert!(!matches_star(Path::new("aba"), "ab", "ba"));
    }

    #[test]
    fn each_table_is_one_module_with_its_own_output() {
        let configs = modules_of(
            "[package.metadata.honig.api]\ndescriptions = [\"own.yaml\"]\nclient = false\n\n\
             [package.metadata.honig.billing]\ndescriptions = [\"vendor/*.yaml\"]\nserver = false\n\n\
             [package.metadata.honig.identity]\ndescriptions = [\"id.yaml\"]\noutput = \"src/id\"\n",
        )
        .unwrap();
        let shown: Vec<_> = configs.iter().map(|c| (c.table.as_str(), c.output.clone(), c.client, c.server)).collect();
        assert_eq!(
            shown,
            [
                ("package.metadata.honig.api", Path::new("src").join("api"), false, true),
                ("package.metadata.honig.billing", Path::new("src").join("billing"), true, false),
                ("package.metadata.honig.identity", Path::new("src").join("id"), true, true),
            ]
        );
    }

    #[test]
    fn client_and_server_take_a_table_of_their_own() {
        let one =
            |module: &str| modules_of(&format!("[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\n{module}"));
        let s = one("server = { allow_unimplemented = true }\n").unwrap();
        assert_eq!((s[0].client, s[0].server, s[0].allow_unimplemented), (true, true, true));
        let s = one("client = false\n").unwrap();
        assert_eq!((s[0].client, s[0].server, s[0].allow_unimplemented), (false, true, false));
        assert_eq!(
            one("server = { allow_unknown_values = true }\n").unwrap_err().to_string(),
            "[package.metadata.honig.api.server] has no setting `allow_unknown_values`. The settings are \
             allow_unimplemented"
        );
        assert_eq!(
            one("client = \"yes\"\n").unwrap_err().to_string(),
            "[package.metadata.honig.api] `client` must be true, false or a table with allow_unknown_values, found \"yes\""
        );
        let strict = |client: &str| one(&format!("client = {client}\n")).unwrap()[0].strict_client;
        assert!(strict("true"), "strict unless set");
        assert!(!strict("{ allow_unknown_values = true }"));
        assert!(
            one("client = { strict = false }\n").unwrap_err().to_string().contains("no setting `strict`"),
            "the old name is an error"
        );
    }

    /// One list of operations serves both halves. A list on a half is a setting that the half does not have.
    #[test]
    fn operations_are_one_list_for_both_halves() {
        let one =
            |module: &str| modules_of(&format!("[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\n{module}"));
        let s = one("operations = [\"getOrder\"]\n").unwrap();
        assert_eq!(s[0].operations, Some(vec!["getOrder".to_owned()]));
        assert_eq!(one("client = true\n").unwrap()[0].operations, None);
        assert_eq!(
            one("client = { operations = [\"getOrder\"] }\n").unwrap_err().to_string(),
            "[package.metadata.honig.api.client] has no setting `operations`. The settings are allow_unknown_values"
        );
    }

    #[test]
    fn a_setting_in_the_wrong_table_says_where_it_goes() {
        assert_eq!(
            modules_of("[package.metadata.honig]\ndescriptions = [\"a.yaml\"]\n").unwrap_err().to_string(),
            "[package.metadata.honig] `descriptions` is a setting of one module. Move it to [package.metadata.honig.api] or \
             another module"
        );
        assert_eq!(
            modules_of(
                "[package.metadata.honig]\nsimd = false\n[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\n"
            )
            .unwrap_err()
            .to_string(),
            "[package.metadata.honig] `simd` is a feature of the runtime. List it in the `features` of the `honig` \
             dependency"
        );
        assert_eq!(
            modules_of("[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\naxum = true\n")
                .unwrap_err()
                .to_string(),
            "[package.metadata.honig.api] `axum` is a feature of the runtime. List it in the `features` of the \
             `honig` dependency"
        );
        assert_eq!(
            modules_of("[package.metadata.honig]\n").unwrap_err().to_string(),
            "[package.metadata.honig] has no module. Add [package.metadata.honig.api] with its `descriptions`"
        );
    }

    #[test]
    fn a_wrong_module_table_is_named_in_its_error() {
        assert_eq!(
            modules_of("[package.metadata.honig.billing]\ndescriptions = [\"b.yaml\"]\nstrcit = false\n")
                .unwrap_err()
                .to_string(),
            format!(
                "[package.metadata.honig.billing] has no setting `strcit`. The settings are {}",
                keys().collect::<Vec<_>>().join(", ")
            )
        );
        assert_eq!(
            modules_of("[package.metadata.honig.billing]\ndescriptions = 3\n").unwrap_err().to_string(),
            "[package.metadata.honig.billing] `descriptions` must be a list of paths like [\"openapi.yaml\"], found 3"
        );
        assert_eq!(
            modules_of("[package.metadata.honig.billing]\ndescriptions = \"b.yaml\"\n").unwrap_err().to_string(),
            "[package.metadata.honig.billing] `descriptions` must be a list of paths like [\"openapi.yaml\"], found \
             \"b.yaml\""
        );
        assert_eq!(
            modules_of("[package.metadata.honig.billing]\ndescriptions = [\"b.yaml\"]\nformats = { uuid = 1 }\n")
                .unwrap_err()
                .to_string(),
            "[package.metadata.honig.billing.formats] `uuid` must be the path of a type such as \"crate::Stamp\", found 1"
        );
        assert_eq!(
            modules_of("[package.metadata.honig.type]\ndescriptions = [\"t.yaml\"]\n").unwrap_err().to_string(),
            "[package.metadata.honig.type] `type` is a Rust keyword. Name the table like a module, such as `billing`"
        );
        assert_eq!(
            modules_of("[package.metadata.honig.ext-a]\ndescriptions = [\"t.yaml\"]\n").unwrap_err().to_string(),
            "[package.metadata.honig.ext-a] `ext-a` is not a Rust identifier. Name the table like a module, such as \
             `billing`"
        );
    }

    #[test]
    fn a_module_name_on_the_command_line_must_be_a_table() {
        let m = parse(
            "[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\n[package.metadata.honig.billing]\ndescriptions = [\"b.yaml\"]\n",
        );
        let check =
            |only| ensure_module(&modules(&m, Path::new("Cargo.toml")).unwrap(), only).map_err(|e| e.to_string());
        assert_eq!(check(Some("billing")), Ok(()));
        assert_eq!(check(None), Ok(()));
        assert_eq!(
            check(Some("bilinig")),
            Err("[package.metadata.honig] has no module `bilinig`. The modules are `api`, `billing`".to_owned())
        );
    }

    #[test]
    fn the_root_of_a_module_follows_the_lints_of_the_crate_unless_set() {
        let root = |extra: &str| {
            let table = format!("[package.metadata.honig.api]\ndescriptions = [\"a.yaml\"]\n{extra}");
            modules_of(&table).ok().map(|ms| ms[0].root_path())
        };
        let file = Some(Path::new("src").join("api.rs"));
        let mod_rs = Some(Path::new("src").join("api").join("mod.rs"));
        assert_eq!(root(""), mod_rs);
        assert_eq!(root("\n[lints.clippy]\nmod_module_files = \"deny\""), file);
        assert_eq!(root("\n[lints.clippy]\nmod_module_files = { level = \"warn\", priority = 1 }"), file);
        assert_eq!(root("\n[lints.clippy]\nmod_module_files = \"allow\""), mod_rs);
        assert_eq!(root("\n[lints.clippy]\nself_named_module_files = \"deny\""), mod_rs);
        // A package that inherits the lints of the workspace it is the root of.
        let inherited = "\n[lints]\nworkspace = true\n[workspace.lints.clippy]\nmod_module_files = \"deny\"";
        assert_eq!(root(inherited), file);
        assert_eq!(root("mod_rs = false"), file);
        assert_eq!(root("mod_rs = true\n[lints.clippy]\nmod_module_files = \"deny\""), mod_rs);
        assert_eq!(root("mod_rs = \"no\""), None);
    }
}
