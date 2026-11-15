//! `$ref` resolution.
//!
//! Two jobs:
//! 1. **Bundling.** External file refs (`other.yaml#/X`) are pulled into the
//!    root document. So every later pass only sees local refs. Schemas become
//!    `#/components/schemas/<name>` (2.0: `#/definitions/<name>`). Everything else is inlined.
//! 2. **Lookup.** `Doc` resolves local refs: JSON pointers (`#/a/b`), plain
//!    name anchors (`#name` via `$anchor`) and absolute URIs that match `$self`
//!    or a schema `$id`.
//!
//! Remote (http) refs are reported and not fetched. Fetching would add an HTTP
//! dependency to the generator.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::diag::Diagnostics;
use crate::error::Error;
use crate::json::{escape_token, is_data, pointer};
use crate::walk::{self, Obj, Visit};

/// Is the generator built with the `remote-refs` feature (fetches `http(s)` refs)?
pub(crate) const REMOTE_REFS: bool = cfg!(feature = "remote-refs");

/// Where relative refs resolve and whether `http(s)` refs may be fetched.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Base<'a> {
    /// Directory of the root file (`None` for documents loaded from a string).
    pub dir: Option<&'a Path>,
    /// Fetch `http(s)://` refs. [`Base::new`] sets it with `remote-refs`.
    pub remote: bool,
}

impl<'a> Base<'a> {
    /// Refs relative to `dir`; remote refs fetched when the feature is built in.
    pub(crate) const fn new(dir: Option<&'a Path>) -> Self {
        Self { dir, remote: REMOTE_REFS }
    }

    /// Refs relative to `dir`. `http(s)` refs are reported and never fetched whatever the build.
    #[cfg(test)]
    pub(crate) const fn offline(dir: Option<&'a Path>) -> Self {
        Self { dir, remote: false }
    }
}

impl Base<'_> {
    fn path(self, file: &str) -> PathBuf {
        if is_http(file) { PathBuf::from(file) } else { self.dir.unwrap_or_else(|| Path::new(".")).join(file) }
    }

    /// Returns a ref to bundle. That is a relative file or a URL when remote refs are on.
    fn is_external(self, r: &str) -> bool {
        is_relative(r) || (self.remote && is_http(r))
    }
}

/// Bundles external file refs into the root document. `base` is the directory
/// of the root file (`None` for documents loaded from a string).
pub(crate) fn bundle(root: Value, base: Option<&Path>, diags: &mut Diagnostics) -> Value {
    bundle_with(root, Base::new(base), diags)
}

/// Rounds of adopting component files: a handful reaches a fixpoint for any realistic nesting.
const MAX_ADOPTION_PASSES: usize = 16;

/// How long a remote `$ref` may take to download.
#[cfg(feature = "remote-refs")]
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Returns [`bundle`] with explicit options.
pub(crate) fn bundle_with(mut root: Value, base: Base<'_>, diags: &mut Diagnostics) -> Value {
    crate::schema_ids::rebase(&mut root);
    let mut cache: BTreeMap<PathBuf, Value> = BTreeMap::new();
    security_scheme_uris(&mut root, base, &mut cache, diags);
    let mut names: BTreeMap<String, String> = BTreeMap::new(); // canonical "file#frag" to component name
    let mut hoisted_names: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    let layout = Layout::of(&root);
    // A handful of passes reaches a fixpoint for any realistic nesting depth.
    for _ in 0..MAX_ADOPTION_PASSES {
        let adopted = adopt_component_files(&mut root, layout, base, &mut cache, &mut names, &mut hoisted_names, diags);
        let mut b =
            Bundler { layout, base, cache: &mut cache, names: &mut names, diags, new_schemas: vec![], changed: false };
        walk::walk(&mut root, &mut b);
        let new_schemas = std::mem::take(&mut b.new_schemas);
        let mut changed = b.changed || adopted;
        let existing: Vec<(String, String)> = ["parameters", "requestBodies", "responses", "headers"]
            .iter()
            .flat_map(|k| {
                (layout.section(k).and_then(|s| root.pointer(&s)).and_then(Value::as_object).into_iter())
                    .flat_map(|m| m.keys().map(|n| ((*k).to_owned(), n.clone())))
            })
            .collect();
        let mut hoist =
            Hoist { layout, base, cache: b.cache, diags: b.diags, names: &mut hoisted_names, existing, new: vec![] };
        changed |= hoist.run(&mut root, "", "");
        let hoisted = std::mem::take(&mut hoist.new);
        for (kind, name, value) in hoisted {
            if let Some(map) = layout.section_mut(&mut root, kind) {
                map.insert(name, value);
                changed = true;
            }
        }
        if !new_schemas.is_empty()
            && let Some(schemas) = layout.section_mut(&mut root, "schemas")
        {
            schemas.extend(new_schemas);
            changed = true;
        }
        if !changed {
            break;
        }
    }
    security_scheme_uris(&mut root, base, &mut cache, diags);
    root
}

/// Renames 3.2 Security Requirement keys that are URIs of Security Scheme Objects
/// instead of component names. Each one becomes a component name. Local
/// URIs become the component that they point at. External ones become a copy added to
/// `components.securitySchemes`. A component name wins over a URI (3.2
/// "Security Considerations"). Keys that resolve nowhere are left alone for
/// validation to report.
fn security_scheme_uris(
    root: &mut Value,
    base: Base<'_>,
    cache: &mut BTreeMap<PathBuf, Value>,
    diags: &mut Diagnostics,
) {
    // Swagger 2.0 (`securityDefinitions`) has names only and never URIs.
    if root.get("swagger").is_some() {
        return;
    }
    let self_uri = root.get("$self").and_then(Value::as_str).map(|s| s.trim_end_matches('#').to_owned());
    let mut keys = vec![];
    security_keys(root, "", &[], &mut keys);
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for (ptr, key) in keys {
        let known =
            |root: &Value, n: &str| root.pointer("/components/securitySchemes").and_then(|m| m.get(n)).is_some();
        // A name with no fragment is a (missing) component name and not a file.
        // A Security Scheme URI must point into a document.
        if known(root, &key) || !key.contains('#') {
            continue;
        }
        let name = if let Some(n) = renamed.get(&key) {
            Some(n.clone())
        } else {
            let (file, frag) = key.split_once('#').unwrap_or((&key, ""));
            let local = file.is_empty() || self_uri.as_deref() == Some(file);
            let prefix = "/components/securitySchemes/";
            let name = if local {
                let frag = crate::json::percent_decode(frag);
                frag.strip_prefix(prefix)
                    .filter(|n| !n.contains('/'))
                    .map(crate::json::unescape_token)
                    .filter(|n| known(root, n))
            } else if base.is_external(&key) {
                // A document that names a scheme in another document that it does not
                // ship is still valid (OAI 3.2 suite). It is a warning here and a
                // refusal when generating a server.
                let mut fetch_diags = Diagnostics::default();
                let scheme = fetch(&key, base, cache, &mut fetch_diags, &ptr);
                for d in fetch_diags.iter() {
                    diags.warning(d.pointer.clone(), d.message.clone());
                }
                scheme.map(|scheme| {
                    let stem = frag.rsplit('/').find(|t| !t.is_empty()).map_or_else(
                        || "external".to_owned(),
                        |t| crate::json::unescape_token(&crate::json::percent_decode(t)),
                    );
                    let mut name = stem.clone();
                    let mut i = 2;
                    while known(root, &name) {
                        name = format!("{stem}{i}");
                        i += 1;
                    }
                    if let Some(map) = components_mut(root, "securitySchemes") {
                        map.insert(name.clone(), scheme);
                    }
                    name
                })
            } else {
                None
            };
            if let Some(n) = &name {
                renamed.insert(key.clone(), n.clone());
            }
            name
        };
        let Some(name) = name else { continue };
        if let Some(Value::Object(req)) = root.pointer_mut(&ptr)
            && let Some(scopes) = req.shift_remove(&key)
        {
            req.insert(name, scopes);
        }
    }
}

/// Returns the Security Requirement keys that name no `components.securitySchemes` entry
/// (after bundling) as (pointer of the requirement object, key).
pub(crate) fn unresolved_security(root: &Value) -> Vec<(String, String)> {
    let mut keys = vec![];
    security_keys(root, "", &[], &mut keys);
    let schemes = root.pointer("/components/securitySchemes").and_then(Value::as_object);
    keys.retain(|(_, k)| schemes.is_none_or(|m| !m.contains_key(k)));
    keys
}

/// Visits every Security Requirement key as (pointer of its requirement object, key).
/// Requirements sit at the root and on operations (any method key or an
/// entry of `additionalOperations`) wherever path items nest.
fn security_keys<'a>(v: &'a Value, ptr: &str, path: &[&'a str], out: &mut Vec<(String, String)>) {
    const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace", "query"];
    let Value::Object(o) = v else { return };
    let at_op = match path {
        [] | [.., "additionalOperations", _] => true,
        [.., m] => METHODS.contains(m) && path.len() >= 2,
    };
    if at_op && let Some(Value::Array(reqs)) = o.get("security") {
        for (i, r) in reqs.iter().enumerate() {
            let rptr = format!("{}/{i}", crate::json::join(ptr, "security"));
            for k in r.as_object().into_iter().flat_map(|r| r.keys()) {
                out.push((rptr.clone(), k.clone()));
            }
        }
    }
    for (k, c) in o {
        if k == "security" || is_data(path.last().copied().unwrap_or(""), k) || k == "schemas" || k == "schema" {
            continue;
        }
        let mut next = path.to_vec();
        next.push(k);
        security_keys(c, &crate::json::join(ptr, k), &next, out);
    }
}

/// Returns a ref to another local file (not a fragment and not a URL).
fn is_relative(r: &str) -> bool {
    !r.starts_with('#') && !r.contains("://")
}

fn is_http(r: &str) -> bool {
    r.starts_with("https://") || r.starts_with("http://")
}

/// Returns an HTTP client that gives up after `timeout`. It checks certificates against the store of the OS with `ring`
/// as its cryptography. There are no bundled root certificates.
#[cfg(feature = "remote-refs")]
fn agent(timeout: std::time::Duration) -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder()
        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
        .unversioned_rustls_crypto_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .build();
    ureq::Agent::config_builder().timeout_global(Some(timeout)).tls_config(tls).build().into()
}

/// Downloads a URL with the built-in client: called only when [`Base::remote`] is set.
#[cfg(feature = "remote-refs")]
fn download(url: &str) -> Result<String, Error> {
    let fetch = |e: ureq::Error| Error::Fetch { url: url.to_owned(), problem: e.to_string() };
    agent(FETCH_TIMEOUT).get(url).call().map_err(fetch)?.body_mut().read_to_string().map_err(fetch)
}

#[cfg(not(feature = "remote-refs"))]
fn download(url: &str) -> Result<String, Error> {
    Err(Error::Fetch {
        url: url.to_owned(),
        problem:
            "this build cannot fetch remote references: it was installed without the default `remote-refs` feature"
                .to_owned(),
    })
}

/// Returns the text of every `externalValue` in `root` read relative to `dir` or why it could not be read. The IR
/// checks examples against it without touching files.
pub(crate) fn read_externals(root: &Value, dir: Option<&Path>) -> crate::json::Externals {
    let mut uris = BTreeSet::new();
    external_values(root, &mut uris);
    uris.into_iter().map(|uri| (uri.clone(), read_external(dir, &uri).map_err(|e| e.to_string()))).collect()
}

fn external_values(v: &Value, out: &mut BTreeSet<String>) {
    match v {
        Value::Object(o) => {
            if let Some(Value::String(uri)) = o.get("externalValue") {
                out.insert(uri.clone());
            }
            o.values().for_each(|v| external_values(v, out));
        }
        Value::Array(a) => a.iter().for_each(|v| external_values(v, out)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Returns the text of an `externalValue`. That is a file relative to `dir` or a URL when
/// the generator is built with `remote-refs`.
///
/// # Errors
/// The file cannot be read or the URL fetched.
fn read_external(dir: Option<&Path>, uri: &str) -> Result<String, Error> {
    if is_http(uri) {
        return download(uri);
    }
    if uri.contains("://") {
        return Err(Error::Scheme(uri.to_owned()));
    }
    let path = Base::new(dir).path(&crate::json::percent_decode(uri));
    crate::disk::read(&path)
}

/// Resolves `rel` against the document at `file` (a path or a URL),
/// collapsing `.` and `..` segments.
fn join_ref(file: &str, rel: &str) -> String {
    if !is_http(file) {
        let dir = Path::new(file).parent().unwrap_or_else(|| Path::new(""));
        return dir.join(rel).to_string_lossy().into_owned();
    }
    let scheme_end = file.find("://").map_or_default(|i| i + "://".len());
    let host_end = file.get(scheme_end..).and_then(|t| t.find('/')).map_or(file.len(), |i| scheme_end + i);
    let origin = file.get(..host_end).unwrap_or(file);
    let path = file.get(host_end..).unwrap_or("");
    let mut segs: Vec<&str> = if rel.starts_with('/') {
        vec![]
    } else {
        let dir = path.rsplit_once('/').map_or_default(|(d, _)| d);
        dir.split('/').filter(|x| !x.is_empty()).collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            other => segs.push(other),
        }
    }
    format!("{origin}/{}", segs.join("/"))
}

fn load(path: &Path, cache: &mut BTreeMap<PathBuf, Value>, diags: &mut Diagnostics, at: &str) -> Option<Value> {
    if let Some(v) = cache.get(path) {
        return Some(v.clone());
    }
    let url = path.to_string_lossy();
    if is_http(&url) {
        let parsed = download(&url)
            .map_err(|e| e.to_string())
            .and_then(|t| crate::parse::parse_str(&t).map(|(v, _)| v).map_err(|d| d.message));
        return match parsed {
            Ok(mut v) => {
                crate::schema_ids::rebase_fetched(&mut v, &url);
                cache.insert(path.to_path_buf(), v.clone());
                Some(v)
            }
            Err(e) => {
                diags.error(at, format!("cannot fetch remote reference `{url}`: {e}"));
                None
            }
        };
    }
    match crate::parse::load_file(path) {
        Ok((mut v, _)) => {
            crate::schema_ids::rebase_fetched(
                &mut v,
                &format!("file://{}", crate::disk::canonical(path.to_path_buf()).display()),
            );
            cache.insert(path.to_path_buf(), v.clone());
            Some(v)
        }
        Err(d) => {
            diags.error(at, format!("cannot load external reference: {}", d.message));
            None
        }
    }
}

/// Rewrites relative refs inside a value copied from `file` so they stay
/// correct once the value lives in the root document.
fn rebase(v: &mut Value, file: &str, parent: &str) {
    match v {
        Value::Object(o) => {
            // `$dynamicRef` too: in a fetched document it names that document as `$ref` does.
            for key in ["$ref", "$dynamicRef"] {
                if let Some(Value::String(r)) = o.get_mut(key) {
                    if let Some(frag) = r.strip_prefix('#') {
                        *r = format!("{file}#{frag}");
                    } else if is_relative(r) {
                        *r = join_ref(file, r);
                    }
                }
            }
            for (k, c) in o.iter_mut() {
                if !is_data(parent, k) {
                    rebase(c, file, k);
                }
            }
        }
        Value::Array(a) => {
            for c in a {
                rebase(c, file, parent);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn fetch(
    r: &str,
    base: Base<'_>,
    cache: &mut BTreeMap<PathBuf, Value>,
    diags: &mut Diagnostics,
    at: &str,
) -> Option<Value> {
    let (file, frag) = r.split_once('#').unwrap_or((r, ""));
    let path = base.path(file);
    let doc = load(&path, cache, diags, at)?;
    // A fragment that is no pointer names an `$anchor` of the document.
    let anchored =
        (!frag.is_empty() && !frag.starts_with('/')).then(|| crate::schema_ids::anchor(&doc, frag)).flatten();
    let Some(target) = pointer(&doc, anchored.as_deref().unwrap_or(frag)) else {
        diags.error(at, format!("`{r}` does not resolve"));
        return None;
    };
    let mut target = target.clone();
    rebase(&mut target, file, "");
    Some(target)
}

struct Bundler<'a> {
    layout: Layout,
    base: Base<'a>,
    cache: &'a mut BTreeMap<PathBuf, Value>,
    names: &'a mut BTreeMap<String, String>,
    diags: &'a mut Diagnostics,
    new_schemas: Vec<(String, Value)>,
    changed: bool,
}

impl Visit for Bundler<'_> {
    fn schema(&mut self, s: &mut Obj, ptr: &str) {
        // An external `$dynamicRef` is bundled like a `$ref`: its target becomes a component and keeps its
        // `$dynamicAnchor`, so the reference stays bookended.
        let keyword = if s.get("$ref").is_some() { "$ref" } else { "$dynamicRef" };
        let Some(r) = s.get(keyword).and_then(Value::as_str).map(String::from) else { return };
        if r.contains("://") && !self.base.is_external(&r) {
            self.diags.error(
                ptr,
                format!(
                    "remote reference `{r}` needs honig built with its default `remote-refs` feature \
                     (cargo install {})",
                    env!("CARGO_PKG_NAME")
                ),
            );
            s.remove(keyword);
            self.changed = true;
            return;
        }
        if !self.base.is_external(&r) {
            return;
        }
        let key = canonical_key(&r, self.base);
        let name = match self.names.get(&key) {
            Some(n) => n.clone(),
            None => {
                let Some(target) = fetch(&r, self.base, self.cache, self.diags, ptr) else {
                    s.remove(keyword);
                    return;
                };
                let stem = r
                    .rsplit(['/', '#'])
                    .find(|t| !t.is_empty())
                    .map_or("External", |t| {
                        t.trim_end_matches(".yaml").trim_end_matches(".yml").trim_end_matches(".json")
                    })
                    .to_owned();
                let mut name = stem.clone();
                let mut i = 2;
                while self.names.values().any(|n| n == &name) {
                    name = format!("{stem}{i}");
                    i += 1;
                }
                self.names.insert(key, name.clone());
                self.new_schemas.push((name.clone(), target));
                name
            }
        };
        s.insert(keyword.into(), json!(self.layout.reference("schemas", &name)));
        self.changed = true;
    }
}

/// Returns the identity of an external target: canonical file path plus fragment. So
/// `../schemas/A.yml` and `./A.yml` that name the same file are one schema.
fn canonical_key(r: &str, base: Base<'_>) -> String {
    let (file, frag) = r.split_once('#').unwrap_or((r, ""));
    let path = base.path(file);
    let path = crate::disk::canonical(path);
    format!("{}#{frag}", path.display())
}

/// Component sections whose external entries are adopted as themselves.
const ADOPTED: [&str; 5] = ["schemas", "parameters", "requestBodies", "responses", "headers"];

/// Returns `components.schemas.X: {$ref: other.yaml}`: the schema of the file becomes X
/// itself (not a new component plus an alias). The same goes for parameters, request
/// bodies, responses and headers. It also goes for a whole section given as one file
/// (`components.parameters: {$ref: parameters/_index.yml}`).
fn adopt_component_files(
    root: &mut Value,
    layout: Layout,
    base: Base<'_>,
    cache: &mut BTreeMap<PathBuf, Value>,
    names: &mut BTreeMap<String, String>,
    hoisted: &mut BTreeMap<String, (&'static str, String)>,
    diags: &mut Diagnostics,
) -> bool {
    let mut changed = false;
    for kind in ADOPTED {
        let Some(at) = layout.section(kind) else { continue };
        let Some(section) = root.pointer_mut(&at) else { continue };
        if let Some(r) = single_external_ref(section, base) {
            *section = fetch(&r, base, cache, diags, &at).unwrap_or_else(|| json!({}));
            changed = true;
        }
        let Some(entries) = section.as_object_mut() else { continue };
        for (name, entry) in entries.iter_mut() {
            let Some(r) = single_external_ref(entry, base) else { continue };
            let at = format!("{at}/{}", escape_token(name));
            if let Some(target) = fetch(&r, base, cache, diags, &at) {
                let key = canonical_key(&r, base);
                if kind == "schemas" {
                    names.insert(key, name.clone());
                } else {
                    hoisted.insert(key, (kind, name.clone()));
                }
                *entry = target;
                changed = true;
            }
        }
    }
    changed
}

/// Returns the target of `{ "$ref": "<external file>" }` with no sibling keys.
fn single_external_ref(v: &Value, base: Base<'_>) -> Option<String> {
    let o = v.as_object()?;
    (o.len() == 1).then_some(())?;
    o.get("$ref").and_then(Value::as_str).filter(|r| base.is_external(r)).map(String::from)
}

/// Returns the component section for an external ref found at `key` inside `container`,
/// or `None` to inline it.
fn component_kind(container: &str, key: &str) -> Option<&'static str> {
    match (container, key) {
        ("parameters", _) => Some("parameters"),
        (_, "requestBody") => Some("requestBodies"),
        ("responses", _) => Some("responses"),
        ("headers", _) => Some("headers"),
        _ => None,
    }
}

/// Every remaining external ref (non-schema contexts). Parameters, request
/// bodies, responses and headers become components once per target. So
/// operations that share a file share its types. Anything else is inlined.
struct Hoist<'a> {
    layout: Layout,
    base: Base<'a>,
    cache: &'a mut BTreeMap<PathBuf, Value>,
    diags: &'a mut Diagnostics,
    /// canonical "file#frag" to (component section, name) across passes.
    names: &'a mut BTreeMap<String, (&'static str, String)>,
    /// (section, name) of components already in the document.
    existing: Vec<(String, String)>,
    new: Vec<(&'static str, String, Value)>,
}

impl Hoist<'_> {
    fn run(&mut self, v: &mut Value, container: &str, key: &str) -> bool {
        let mut changed = false;
        match v {
            Value::Object(o) => {
                if let Some(r) =
                    o.get("$ref").and_then(Value::as_str).filter(|r| self.base.is_external(r)).map(String::from)
                {
                    // A kind the version has no section for (2.0 headers) is inlined.
                    let kind = component_kind(container, key).filter(|k| self.layout.section(k).is_some());
                    *v = match kind {
                        Some(kind) => self
                            .component(&r, kind)
                            .map_or_else(|| json!({}), |name| json!({ "$ref": self.layout.reference(kind, &name) })),
                        None => fetch(&r, self.base, self.cache, self.diags, "").unwrap_or_else(|| json!({})),
                    };
                    return true;
                }
                for (k, c) in o.iter_mut() {
                    if !is_data(key, k) {
                        changed |= self.run(c, key, k);
                    }
                }
            }
            Value::Array(a) => {
                // Elements live in the array named `key` (`parameters: [...]`).
                for c in a {
                    changed |= self.run(c, key, key);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
        changed
    }

    /// Names of the component that holds the target of `r`. It is fetched once.
    fn component(&mut self, r: &str, kind: &'static str) -> Option<String> {
        let key = canonical_key(r, self.base);
        if let Some((k, name)) = self.names.get(&key)
            && *k == kind
        {
            return Some(name.clone());
        }
        let target = fetch(r, self.base, self.cache, self.diags, "")?;
        let stem = r
            .rsplit(['/', '#'])
            .find(|t| !t.is_empty())
            .map_or("External", |t| t.trim_end_matches(".yaml").trim_end_matches(".yml").trim_end_matches(".json"))
            .to_owned();
        let taken = |n: &str, h: &Self| {
            h.names.values().any(|(k, x)| *k == kind && x == n) || h.existing.iter().any(|(k, x)| k == kind && x == n)
        };
        let mut name = stem.clone();
        let mut i = 2;
        while taken(&name, self) {
            name = format!("{stem}{i}");
            i += 1;
        }
        self.names.insert(key, (kind, name.clone()));
        self.new.push((kind, name.clone(), target));
        Some(name)
    }
}

/// Where bundled components go. That is `components.<kind>` from 3.0 on and the own top-level sections of 2.0 before.
/// The 2.0 to 3.0 upgrade moves those. So a ref written here still resolves after it.
#[derive(Debug, Clone, Copy)]
struct Layout {
    v20: bool,
}

impl Layout {
    fn of(root: &Value) -> Self {
        Self { v20: root.get("swagger").is_some() }
    }

    /// Returns JSON pointer of the section holding `kind`; `None` when the version has none.
    fn section(self, kind: &str) -> Option<String> {
        match (self.v20, kind) {
            (false, _) => Some(format!("/components/{kind}")),
            (true, "schemas") => Some("/definitions".to_owned()),
            (true, "parameters" | "responses") => Some(format!("/{kind}")),
            (true, _) => None,
        }
    }

    /// Returns `$ref` to component `name` of `kind`.
    fn reference(self, kind: &str, name: &str) -> String {
        let section = self.section(kind).unwrap_or_else(|| format!("/components/{kind}"));
        format!("#{section}/{}", escape_token(name))
    }

    /// Returns the section that holds `kind` in the document at `root`. It is created when missing.
    fn section_mut<'d>(self, root: &'d mut Value, kind: &str) -> Option<&'d mut Map<String, Value>> {
        let section = self.section(kind)?;
        let mut cur = root;
        for token in section.split('/').skip(1) {
            cur = cur.as_object_mut()?.entry(token).or_insert(json!({}));
        }
        cur.as_object_mut()
    }
}

/// Returns `components.<kind>` of the document at `root`. It is created when missing.
fn components_mut<'d>(root: &'d mut Value, kind: &str) -> Option<&'d mut Map<String, Value>> {
    let components = root.as_object_mut()?.entry("components").or_insert(json!({}));
    components.as_object_mut()?.entry(kind).or_insert(json!({})).as_object_mut()
}

#[cfg(test)]
#[expect(clippy::disallowed_methods, reason = "tests set up files of their own")]
mod tests {
    use super::*;

    #[test]
    fn bundles_external_files() {
        let dir = std::env::temp_dir().join(format!("honig-bundle-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(
            dir.join("sub").join("order.yaml"),
            "Order:\n  type: object\n  properties:\n    tag: {$ref: '#/Tag'}\nTag: {type: string}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("resp.yaml"),
            "description: ok\ncontent:\n  application/json:\n    schema: {$ref: 'sub/order.yaml#/Order'}\n",
        )
        .unwrap();
        let root = json!({
            "openapi": "3.2.0",
            "paths": {"/p": {"get": {"responses": {"200": {"$ref": "resp.yaml"}}}}}
        });
        let mut d = Diagnostics::default();
        let out = bundle(root, Some(&dir), &mut d);
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(out["paths"]["/p"]["get"]["responses"]["200"]["$ref"], "#/components/responses/resp");
        let schema = &out["components"]["responses"]["resp"]["content"]["application/json"]["schema"];
        assert_eq!(schema["$ref"], "#/components/schemas/Order");
        assert_eq!(out["components"]["schemas"]["Order"]["properties"]["tag"]["$ref"], "#/components/schemas/Tag");
        assert_eq!(out["components"]["schemas"]["Tag"]["type"], "string");
        std::fs::remove_dir_all(dir).ok();
    }

    /// One file per resource and per schema as a hand-written description grows. The bundle is the single file that
    /// it was split from with the version and component names.
    #[test]
    fn a_v20_description_bundles_into_its_own_sections() {
        let dir = std::env::temp_dir().join(format!("honig-bundle-v20-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Order.yaml"), "type: object\nproperties:\n  name: {type: string}\n").unwrap();
        std::fs::write(dir.join("parameters.yaml"), "limit: {name: limit, in: query, type: integer}\n").unwrap();
        let root = json!({
            "swagger": "2.0",
            "paths": {"/orders": {"get": {
                "parameters": [{"$ref": "parameters.yaml#/limit"}],
                "responses": {"200": {"description": "ok", "schema": {"$ref": "Order.yaml"}}}
            }}}
        });
        let mut d = Diagnostics::default();
        let out = bundle(root, Some(&dir), &mut d);
        assert!(d.is_empty(), "{d:?}");
        let get = &out["paths"]["/orders"]["get"];
        // The own sections of 2.0. The 2.0 to 3.0 upgrade moves them with their refs.
        assert_eq!(get["parameters"][0]["$ref"], "#/parameters/limit");
        assert_eq!(get["responses"]["200"]["schema"]["$ref"], "#/definitions/Order");
        assert_eq!(out["parameters"]["limit"]["name"], "limit");
        assert_eq!(out["definitions"]["Order"]["properties"]["name"]["type"], "string");
        assert_eq!(out.get("components"), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_split_description_bundles_back_to_its_single_file() {
        let dir = std::env::temp_dir().join(format!("honig-split-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("paths")).unwrap();
        std::fs::create_dir_all(dir.join("schemas")).unwrap();
        let comment = "type: object\nproperties:\n  replies: {type: array, items: {$ref: './Comment.yaml'}}\n";
        std::fs::write(dir.join("schemas").join("Comment.yaml"), comment).unwrap();
        let page = "type: object\nproperties:\n  items: {type: array, items: {$ref: './Comment.yaml'}}\n";
        std::fs::write(dir.join("schemas").join("Page.yaml"), page).unwrap();
        std::fs::write(
            dir.join("paths").join("comments.yaml"),
            "/comments/{id}:\n  get:\n    responses:\n      '200':\n        description: ok\n        content:\n          application/json:\n            schema: {$ref: '../schemas/Page.yaml'}\n",
        )
        .unwrap();
        let split = json!({
            "openapi": "3.1.0",
            "paths": {"/comments/{id}": {"$ref": "./paths/comments.yaml#/~1comments~1{id}"}},
            "components": {"schemas": {
                "Comment": {"$ref": "./schemas/Comment.yaml"},
                "Page": {"$ref": "./schemas/Page.yaml"}
            }}
        });
        let single = json!({
            "openapi": "3.1.0",
            "paths": {"/comments/{id}": {"get": {"responses": {"200": {"description": "ok", "content": {
                "application/json": {"schema": {"$ref": "#/components/schemas/Page"}}
            }}}}}},
            "components": {"schemas": {
                "Comment": {"type": "object", "properties": {"replies": {"type": "array", "items": {"$ref": "#/components/schemas/Comment"}}}},
                "Page": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/components/schemas/Comment"}}}}
            }}
        });
        let mut d = Diagnostics::default();
        let out = bundle(split, Some(&dir), &mut d);
        assert!(d.is_empty(), "{d:?}");
        assert_eq!(out, single);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn joins_refs_against_urls_and_paths() {
        assert_eq!(join_ref("https://a.io/x/y/spec.yaml", "../s/P.yaml"), "https://a.io/x/s/P.yaml");
        assert_eq!(join_ref("https://a.io/x/spec.yaml", "./P.yaml"), "https://a.io/x/P.yaml");
        assert_eq!(join_ref("https://a.io/x/spec.yaml", "/abs/P.yaml"), "https://a.io/abs/P.yaml");
        assert_eq!(join_ref("dir/spec.yaml", "P.yaml"), "dir/P.yaml");
    }

    #[test]
    #[cfg(not(feature = "remote-refs"))]
    fn remote_refs_fail_without_the_feature() {
        let root =
            json!({"openapi": "3.2.0", "components": {"schemas": {"A": {"$ref": "https://example.invalid/a.yaml"}}}});
        let mut d = Diagnostics::default();
        bundle(root, None, &mut d);
        assert!(d.has_errors(), "{d:?}");
        assert!(d.iter().any(|x| x.message.contains("remote-refs")), "{d:?}");
    }

    #[test]
    fn shares_external_parameters_and_adopts_sections() {
        let dir = std::env::temp_dir().join(format!("honig-params-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("params")).unwrap();
        std::fs::write(
            dir.join("params").join("lang.yaml"),
            "name: lang\nin: query\nschema: {type: string, enum: [en, de]}\n",
        )
        .unwrap();
        std::fs::write(dir.join("params").join("_index.yaml"), "lang: {$ref: './lang.yaml'}\n").unwrap();
        let root = json!({
            "openapi": "3.2.0",
            "components": {"parameters": {"$ref": "params/_index.yaml"}},
            "paths": {
                "/a": {"get": {"parameters": [{"$ref": "params/lang.yaml"}], "responses": {}}},
                "/b": {"get": {"parameters": [{"$ref": "./params/lang.yaml"}], "responses": {}}}
            }
        });
        let mut d = Diagnostics::default();
        let out = bundle(root, Some(&dir), &mut d);
        assert!(d.is_empty(), "{d:?}");
        // One component adopted under its declared name and shared by both operations.
        assert_eq!(out["components"]["parameters"].as_object().unwrap().len(), 1);
        assert_eq!(out["components"]["parameters"]["lang"]["name"], "lang");
        for p in ["/a", "/b"] {
            assert_eq!(out["paths"][p]["get"]["parameters"][0]["$ref"], "#/components/parameters/lang");
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
