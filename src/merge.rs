//! Several root documents as one. Matrix describes its client-server API in 74 files, each a complete document
//! with its own paths and servers. Each file is bundled against its own directory and upgraded to 3.2 alone. Then
//! paths, webhooks, components and tags merge.
//!
//! What a file says for all of its operations stays with them. Files whose one server differs only in its path
//! (Matrix: `/_matrix/client/v3`, `/_matrix/media/v1`) share the host: each file's base path moves into its path
//! keys and the root server keeps the host. So the client's base URL serves every operation. Root `servers` that
//! differ otherwise move onto the file's path items (3.2 section 4.9: a path item's `servers` override the root's). Root `security` that
//! differs moves onto the file's operations that have none of their own. The same path and method in two files and
//! two different components under one name are errors: guessing which one wins would change the API.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::diag::{Diagnostic, Diagnostics};
use crate::{parse, resolve, upgrade};

/// The HTTP methods of a path item (3.2 section 4.9).
const METHODS: [&str; 9] = ["get", "put", "post", "delete", "options", "head", "patch", "trace", "query"];

/// Returns the files of `specs` merged into one 3.2 document. `None` when a file does not load.
pub(crate) fn merge(specs: &[PathBuf], diags: &mut Diagnostics) -> Option<Value> {
    let mut docs = vec![];
    for path in specs {
        let mut own = Diagnostics::default();
        let doc = one(path, &mut own);
        diags.extend(located(own, path));
        docs.push((path, doc?));
    }
    let shared =
        |key: &str| docs.windows(2).all(|w| w.first().map(|(_, d)| d.get(key)) == w.get(1).map(|(_, d)| d.get(key)));
    let (servers_shared, security_shared) = (shared("servers"), shared("security"));
    let split: Vec<Option<(String, String)>> = docs.iter().map(|(_, d)| one_server(d)).collect();
    let host = split.first().cloned().flatten().map(|(h, _)| h);
    let fold = !servers_shared && host.is_some() && split.iter().all(|s| s.as_ref().map(|(h, _)| h) == host.as_ref());
    let mut out = Map::new();
    if let (true, Some(host)) = (fold, &host) {
        out.insert("servers".to_owned(), serde_json::json!([{"url": host}]));
    }
    for ((path, doc), split) in docs.into_iter().zip(split) {
        let Value::Object(mut doc) = doc else { continue };
        let file = path.display().to_string();
        if fold {
            doc.remove("servers");
            let base = split.map_or_else(String::new, |(_, b)| b);
            if let Some(Value::Object(paths)) = doc.get_mut("paths") {
                *paths = std::mem::take(paths).into_iter().map(|(p, item)| (format!("{base}{p}"), item)).collect();
            }
        } else if !servers_shared && let Some(servers) = doc.remove("servers") {
            each_item(&mut doc, |item| {
                item.entry("servers").or_insert_with(|| servers.clone());
            });
        }
        if !security_shared && let Some(security) = doc.remove("security") {
            each_item(&mut doc, |item| {
                for m in METHODS {
                    if let Some(Value::Object(op)) = item.get_mut(m) {
                        op.entry("security").or_insert_with(|| security.clone());
                    }
                }
            });
        }
        for (key, value) in doc {
            match (key.as_str(), out.get_mut(&key)) {
                (_, None) => {
                    out.insert(key, value);
                }
                ("paths" | "webhooks", Some(Value::Object(into))) => items(into, value, &key, &file, diags),
                ("components", Some(Value::Object(into))) => components(into, value, &file, diags),
                ("tags", Some(Value::Array(into))) => {
                    for tag in value.as_array().into_iter().flatten() {
                        if !into.iter().any(|t| t.get("name") == tag.get("name")) {
                            into.push(tag.clone());
                        }
                    }
                }
                // `openapi`, `info` and the rest: the first file's.
                _ => {}
            }
        }
    }
    Some(Value::Object(out))
}

/// Returns the (host, base path) of a document with exactly one server, its variables at their defaults. `None` with
/// several servers or when a variable's alternatives change the base path: folding would drop them. Alternatives
/// that only change the host (`http` or `https`) do not matter: the client's base URL picks the host.
fn one_server(doc: &Value) -> Option<(String, String)> {
    let [server] = doc.get("servers")?.as_array()?.as_slice() else { return None };
    let template = server.get("url")?.as_str()?;
    let vars = server.get("variables").and_then(Value::as_object);
    let expand = |pick: &dyn Fn(&str, &Value) -> Option<String>| -> Option<(String, String)> {
        let mut url = template.to_owned();
        for (name, var) in vars.into_iter().flatten() {
            url = url.replace(&format!("{{{name}}}"), &pick(name, var)?);
        }
        let (scheme, rest) = url.split_once("://")?;
        let (host, base) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        Some((format!("{scheme}://{host}"), base.trim_end_matches('/').to_owned()))
    };
    let default = |_: &str, v: &Value| v.get("default")?.as_str().map(str::to_owned);
    let found = expand(&default)?;
    for (name, var) in vars.into_iter().flatten() {
        for alt in var.get("enum").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            let pick = |n: &str, v: &Value| if n == name { Some(alt.to_owned()) } else { default(n, v) };
            if expand(&pick)?.1 != found.1 {
                return None;
            }
        }
    }
    Some(found)
}

/// Returns one file bundled against its directory and upgraded to 3.2.
fn one(path: &Path, diags: &mut Diagnostics) -> Option<Value> {
    let value = match parse::load_file(path) {
        Ok((v, _)) => v,
        Err(d) => {
            diags.push(d);
            return None;
        }
    };
    let value = resolve::bundle_with(value, resolve::Base::new(path.parent()), diags);
    upgrade::to_v32(value, diags)
}

/// Returns `diags` with the file that each came from: a pointer alone no longer says which file.
fn located(diags: Diagnostics, path: &Path) -> Diagnostics {
    Diagnostics(
        diags.0.into_iter().map(|d| Diagnostic { pointer: format!("{}#{}", path.display(), d.pointer), ..d }).collect(),
    )
}

/// Runs `f` on every path item and webhook of `doc`.
fn each_item(doc: &mut Map<String, Value>, mut f: impl FnMut(&mut Map<String, Value>)) {
    for key in ["paths", "webhooks"] {
        for item in doc.get_mut(key).and_then(Value::as_object_mut).into_iter().flat_map(|m| m.values_mut()) {
            if let Value::Object(item) = item {
                f(item);
            }
        }
    }
}

/// Adds the path items of `from` to `into`. Two files may share a path but not a method of it.
fn items(into: &mut Map<String, Value>, from: Value, key: &str, file: &str, diags: &mut Diagnostics) {
    let Value::Object(from) = from else { return };
    for (path, item) in from {
        let Value::Object(item) = item else { continue };
        let Some(Value::Object(have)) = into.get_mut(&path) else {
            into.insert(path, Value::Object(item));
            continue;
        };
        for (k, v) in item {
            match have.get(&k) {
                None => {
                    have.insert(k, v);
                }
                Some(old) if *old == v => {}
                Some(_) if METHODS.contains(&k.as_str()) => diags.error(
                    format!("{file}#/{key}/{}/{k}", crate::json::escape_token(&path)),
                    format!("`{} {path}` is also described by an earlier file", k.to_uppercase()),
                ),
                // `servers`, `parameters` and the like that differ: each file's operations keep their own.
                Some(_) => {
                    for m in METHODS {
                        if let Some(Value::Object(op)) = have.get_mut(m) {
                            op.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
            }
        }
    }
}

/// Adds the components of `from` to `into`. The same name for two different values is an error.
fn components(into: &mut Map<String, Value>, from: Value, file: &str, diags: &mut Diagnostics) {
    let Value::Object(from) = from else { return };
    for (kind, group) in from {
        let Value::Object(group) = group else { continue };
        let have = into.entry(kind.clone()).or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(have) = have else { continue };
        for (name, v) in group {
            match have.get(&name) {
                None => {
                    have.insert(name, v);
                }
                Some(old) if *old == v => {}
                Some(_) => diags.error(
                    format!("{file}#/components/{kind}/{}", crate::json::escape_token(&name)),
                    format!("`{name}` is a different {kind} entry in an earlier file"),
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A fresh directory for one test's files.
    fn scratch(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("honig-merge-{test}-{}", std::process::id()))
    }

    fn write(dir: &Path, name: &str, v: &Value) -> PathBuf {
        let p = dir.join(name);
        crate::disk::write(&p, &v.to_string()).unwrap();
        p
    }

    fn file(path: &str, base: &str, schema: &Value) -> Value {
        json!({
            "openapi": "3.1.0", "info": {"title": path, "version": "1"},
            "servers": [{"url": "{scheme}://{host}{basePath}", "variables": {
                "scheme": {"default": "https"}, "host": {"default": "localhost"}, "basePath": {"default": base}}}],
            "paths": {path: {"get": {"operationId": path.trim_start_matches('/'), "responses": {"200": {
                "description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Id"}}}}}}}},
            "components": {"schemas": {"Id": schema}}
        })
    }

    #[test]
    fn each_file_keeps_its_servers_and_shared_components_merge() {
        let dir = scratch("servers");
        let id = json!({"type": "string", "pattern": "^@"});
        let a = write(&dir, "a.json", &file("/login", "/_matrix/client/v3", &id));
        let b = write(&dir, "b.json", &file("/media", "/_matrix/media/v1", &id));
        let mut d = Diagnostics::default();
        let v = merge(&[a, b], &mut d).unwrap();
        assert!(!d.has_errors(), "{d:?}");
        // One host and two base paths: each base path moves into its file's paths.
        assert_eq!(v["servers"], json!([{"url": "https://localhost"}]));
        assert!(v["paths"]["/_matrix/client/v3/login"]["get"].is_object(), "{:#}", v["paths"]);
        assert!(v["paths"]["/_matrix/media/v1/media"]["get"].is_object());
        assert_eq!(v["components"]["schemas"]["Id"]["pattern"], "^@");
    }

    #[test]
    fn files_on_different_hosts_keep_their_servers() {
        let dir = scratch("hosts");
        let mut b = file("/media", "/v1", &json!({"type": "string"}));
        b["servers"] = json!([{"url": "https://media.example.org/v1"}]);
        let a = write(&dir, "a.json", &file("/login", "/v3", &json!({"type": "string"})));
        let b = write(&dir, "b.json", &b);
        let mut d = Diagnostics::default();
        let v = merge(&[a, b], &mut d).unwrap();
        assert_eq!(v["paths"]["/media"]["servers"][0]["url"], "https://media.example.org/v1");
        assert_eq!(v["paths"]["/login"]["servers"][0]["url"], "{scheme}://{host}{basePath}");
    }

    #[test]
    fn one_name_for_two_components_is_an_error() {
        let dir = scratch("clash");
        let a = write(&dir, "a.json", &file("/login", "/v3", &json!({"type": "string"})));
        let b = write(&dir, "b.json", &file("/media", "/v3", &json!({"type": "integer"})));
        let mut d = Diagnostics::default();
        merge(&[a, b], &mut d);
        assert!(d.iter().any(|x| x.message.contains("`Id` is a different schemas entry")), "{d:?}");
    }
}
