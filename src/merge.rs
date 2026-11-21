//! Several root documents as one, each a complete document with its own paths and servers. Each file is bundled against its own directory and upgraded to 3.2 alone. Then
//! paths, webhooks, components and tags merge.
//!
//! What a file says for all of its operations stays with them. Files whose one server differs only in its path
//! (`/client/v3`, `/media/v1`) share the host: each file's base path moves into its path
//! keys and the root server keeps the host. So the client's base URL serves every operation. Root `servers` that
//! differ otherwise move onto the file's path items (3.2 section 4.9: a path item's `servers` override the root's). Root `security` that
//! differs moves onto the file's operations that have none of their own. The same path and method in two files and
//! two different components under one name are errors: the API holds one of each, and the description decides which.

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
            each_item(&mut doc, |item| onto_operations(item, "security", &security));
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

/// The path item field with the operations of methods that have no field of their own (3.2 section 4.9).
const ADDITIONAL: &str = "additionalOperations";

/// Adds the path items of `from` to `into`. Two files may share a path but not a method of it.
///
/// `servers`, `parameters` and the other fields that the files of a shared path do not agree on move from the path
/// item onto the operations of the file that set them. So no file's operations take a field of another file.
fn items(into: &mut Map<String, Value>, from: Value, key: &str, file: &str, diags: &mut Diagnostics) {
    let Value::Object(from) = from else { return };
    for (path, item) in from {
        let Value::Object(mut item) = item else { continue };
        let Some(Value::Object(have)) = into.get_mut(&path) else {
            into.insert(path, Value::Object(item));
            continue;
        };
        let mut differing: Vec<String> = have
            .keys()
            .chain(item.keys())
            .filter(|k| !METHODS.contains(&k.as_str()) && *k != ADDITIONAL && have.get(*k) != item.get(*k))
            .cloned()
            .collect();
        differing.sort_unstable();
        differing.dedup();
        for k in differing {
            for owner in [&mut *have, &mut item] {
                if let Some(value) = owner.remove(&k) {
                    onto_operations(owner, &k, &value);
                }
            }
        }
        let at = |k: &str| format!("{file}#/{key}/{}/{k}", crate::json::escape_token(&path));
        for (k, v) in item {
            match (have.get_mut(&k), v) {
                (None, v) => {
                    have.insert(k, v);
                }
                (Some(old), v) if *old == v => {}
                (Some(Value::Object(ops)), Value::Object(added)) if k == ADDITIONAL => {
                    for (method, op) in added {
                        match ops.get(&method) {
                            None => {
                                ops.insert(method, op);
                            }
                            Some(old) if *old == op => {}
                            Some(_) => diags.error(
                                at(&format!("{ADDITIONAL}/{}", crate::json::escape_token(&method))),
                                format!("`{method} {path}` is also described by an earlier file"),
                            ),
                        }
                    }
                }
                (Some(_), _) => {
                    diags.error(at(&k), format!("`{} {path}` is also described by an earlier file", k.to_uppercase()));
                }
            }
        }
    }
}

/// Gives each operation of a path item the field `key` of the path item. An operation's own field wins. Parameters
/// add up: an operation's parameter replaces the one of the path item with its name and location.
fn onto_operations(item: &mut Map<String, Value>, key: &str, value: &Value) {
    let give = |op: &mut Value| {
        let Value::Object(op) = op else { return };
        match (key, value) {
            ("parameters", Value::Array(shared)) => {
                let Value::Array(own) = op.entry(key).or_insert_with(|| Value::Array(vec![])) else { return };
                let id = |p: &Value| (p.get("name").cloned(), p.get("in").cloned());
                let added: Vec<Value> = shared
                    .iter()
                    .filter(|p| !own.iter().any(|o| o == *p || (o.get("name").is_some() && id(o) == id(p))))
                    .cloned()
                    .collect();
                own.extend(added);
            }
            _ => {
                op.entry(key).or_insert_with(|| value.clone());
            }
        }
    };
    for (k, op) in item.iter_mut() {
        if METHODS.contains(&k.as_str()) {
            give(op);
        } else if k == ADDITIONAL
            && let Value::Object(ops) = op
        {
            ops.values_mut().for_each(give);
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
        let a = write(&dir, "a.json", &file("/login", "/api/client/v3", &id));
        let b = write(&dir, "b.json", &file("/media", "/api/media/v1", &id));
        let mut d = Diagnostics::default();
        let v = merge(&[a, b], &mut d).unwrap();
        assert!(!d.has_errors(), "{d:?}");
        // One host and two base paths: each base path moves into its file's paths.
        assert_eq!(v["servers"], json!([{"url": "https://localhost"}]));
        assert!(v["paths"]["/api/client/v3/login"]["get"].is_object(), "{:#}", v["paths"]);
        assert!(v["paths"]["/api/media/v1/media"]["get"].is_object());
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

    #[test]
    fn one_method_in_two_files_is_an_error() {
        let dir = scratch("method");
        let id = json!({"type": "string"});
        let mut b = file("/login", "/v3", &id);
        b["paths"]["/login"]["get"]["operationId"] = json!("again");
        let a = write(&dir, "a.json", &file("/login", "/v3", &id));
        let b = write(&dir, "b.json", &b);
        let mut d = Diagnostics::default();
        let v = merge(&[a, b.clone()], &mut d).unwrap();
        let errors: Vec<(String, String)> = d.iter().map(|x| (x.pointer.clone(), x.message.clone())).collect();
        assert_eq!(
            errors,
            [(
                format!("{}#/paths/~1login/get", b.display()),
                "`GET /login` is also described by an earlier file".to_owned()
            )]
        );
        assert_eq!(v["paths"]["/login"]["get"]["operationId"], "login", "the first file's");
    }

    /// Two files on different hosts share a path with different methods. Each method keeps its own file's servers.
    #[test]
    fn methods_of_one_path_keep_the_servers_of_their_file() {
        let dir = scratch("shared-path");
        let id = json!({"type": "string"});
        let a = file("/login", "/v3", &id);
        let mut b = file("/login", "/v3", &id);
        b["servers"] = json!([{"url": "https://other.example.org/v3"}]);
        let get = b["paths"]["/login"]["get"].take();
        b["paths"]["/login"] = json!({"post": get});
        let a = write(&dir, "a.json", &a);
        let b = write(&dir, "b.json", &b);
        let mut d = Diagnostics::default();
        let v = merge(&[a, b], &mut d).unwrap();
        assert!(!d.has_errors(), "{d:?}");
        let item = &v["paths"]["/login"];
        let servers = |m: &str| item[m].get("servers").unwrap_or_else(|| &item["servers"]).clone();
        assert_eq!(
            servers("get"),
            json!([{"url": "{scheme}://{host}{basePath}", "variables": {
            "scheme": {"default": "https"}, "host": {"default": "localhost"}, "basePath": {"default": "/v3"}}}]),
            "{item:#}"
        );
        assert_eq!(servers("post"), json!([{"url": "https://other.example.org/v3"}]), "{item:#}");
    }

    /// Two files share `/login`: `a` has `GET` and `b` has `POST`. `edit_a` and `edit_b` change their path items.
    fn shared_path(test: &str, edit_a: impl Fn(&mut Value), edit_b: impl Fn(&mut Value)) -> (Value, Diagnostics) {
        let dir = scratch(test);
        let id = json!({"type": "string"});
        let mut a = file("/login", "/v3", &id);
        let mut b = file("/login", "/v3", &id);
        let get = b["paths"]["/login"]["get"].take();
        b["paths"]["/login"] = json!({"post": get});
        edit_a(&mut a["paths"]["/login"]);
        edit_b(&mut b["paths"]["/login"]);
        let a = write(&dir, "a.json", &a);
        let b = write(&dir, "b.json", &b);
        let mut d = Diagnostics::default();
        let v = merge(&[a, b], &mut d).unwrap();
        (v["paths"]["/login"].clone(), d)
    }

    fn names(op: &Value) -> Vec<&str> {
        op["parameters"].as_array().into_iter().flatten().filter_map(|p| p["name"].as_str()).collect()
    }

    #[test]
    fn a_field_of_one_file_stays_with_its_operations() {
        let q = json!([{"name": "q", "in": "query", "schema": {"type": "string"}}]);
        let (item, d) = shared_path("later-only", |_| {}, |b| b["parameters"] = q.clone());
        assert!(!d.has_errors(), "{d:?}");
        assert_eq!((names(&item["get"]), names(&item["post"])), (vec![], vec!["q"]), "{item:#}");
        assert!(item.get("parameters").is_none(), "on the path item it would reach `GET`: {item:#}");
        let (item, _) = shared_path("earlier-only", |a| a["parameters"] = q.clone(), |_| {});
        assert_eq!((names(&item["get"]), names(&item["post"])), (vec!["q"], vec![]), "{item:#}");
        assert!(item.get("parameters").is_none(), "{item:#}");
    }

    #[test]
    fn path_parameters_add_to_those_of_an_operation() {
        let (item, _) = shared_path(
            "parameters",
            |_| {},
            |b| {
                b["parameters"] = json!([{"name": "q", "in": "query"}, {"name": "id", "in": "query"}]);
                b["post"]["parameters"] = json!([{"name": "id", "in": "query", "required": true}]);
            },
        );
        assert_eq!(names(&item["post"]), ["id", "q"], "{item:#}");
        assert_eq!(item["post"]["parameters"][0]["required"], true, "the operation's own wins");
    }

    #[test]
    fn additional_operations_merge_by_method() {
        let op = |id: &str| json!({"operationId": id, "responses": {"204": {"description": "ok"}}});
        let (item, d) = shared_path(
            "additional",
            |a| a["additionalOperations"] = json!({"COPY": op("copy")}),
            |b| b["additionalOperations"] = json!({"LINK": op("link")}),
        );
        assert!(!d.has_errors(), "{d:?}");
        assert_eq!(item["additionalOperations"]["COPY"]["operationId"], "copy");
        assert_eq!(item["additionalOperations"]["LINK"]["operationId"], "link");
        let (_, d) = shared_path(
            "additional-clash",
            |a| a["additionalOperations"] = json!({"COPY": op("copy")}),
            |b| b["additionalOperations"] = json!({"COPY": op("again")}),
        );
        let messages: Vec<&str> = d.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(messages, ["`COPY /login` is also described by an earlier file"]);
    }

    #[test]
    fn tags_merge_by_name() {
        let dir = scratch("tags");
        let id = json!({"type": "string"});
        let mut a = file("/login", "/v3", &id);
        a["tags"] = json!([{"name": "auth"}]);
        let mut b = file("/media", "/v3", &id);
        b["tags"] = json!([{"name": "auth", "description": "again"}, {"name": "media"}]);
        let a = write(&dir, "a.json", &a);
        let b = write(&dir, "b.json", &b);
        let mut d = Diagnostics::default();
        let v = merge(&[a, b], &mut d).unwrap();
        assert_eq!(v["tags"], json!([{"name": "auth"}, {"name": "media"}]));
    }

    /// Files with different top-level `security` keep it on their own operations: one file's requirement never
    /// reaches another's.
    #[test]
    fn each_file_keeps_its_security() {
        let dir = scratch("security");
        let mut a = file("/login", "/v3", &json!({"type": "string"}));
        a["security"] = json!([{"key": []}]);
        let a = write(&dir, "a.json", &a);
        let b = write(&dir, "b.json", &file("/media", "/v3", &json!({"type": "string"})));
        let mut d = Diagnostics::default();
        let v = merge(&[a, b], &mut d).unwrap();
        assert!(v.get("security").is_none(), "{v:#}");
        assert_eq!(v["paths"]["/login"]["get"]["security"], json!([{"key": []}]), "{:#}", v["paths"]);
        assert!(v["paths"]["/media"]["get"].get("security").is_none(), "{:#}", v["paths"]);
    }

    #[test]
    fn a_diagnostic_names_its_file() {
        let dir = scratch("located");
        let id = json!({"$ref": "#/components/schemas/Nope"});
        let a = write(&dir, "a.json", &file("/login", "/v3", &json!({"type": "string"})));
        let b = write(&dir, "b.json", &file("/media", "/v3", &id));
        let mut d = Diagnostics::default();
        merge(&[a, b.clone()], &mut d);
        let prefix = format!("{}#/", b.display());
        assert!(!d.is_empty() && d.iter().all(|x| x.pointer.starts_with(&prefix)), "{d:?}");
    }

    #[test]
    fn a_variable_that_changes_the_base_path_keeps_the_servers() {
        let with = |var: &str, alternatives: Value| {
            let dir = scratch(&format!("variable-{var}"));
            let id = json!({"type": "string"});
            let mut a = file("/login", "/v3", &id);
            a["servers"][0]["variables"][var]["enum"] = alternatives;
            let a = write(&dir, "a.json", &a);
            let b = write(&dir, "b.json", &file("/media", "/v1", &id));
            let mut d = Diagnostics::default();
            merge(&[a, b], &mut d).unwrap()
        };
        let folded = with("scheme", json!(["https", "http"]));
        assert!(folded["paths"]["/v3/login"].is_object(), "the scheme changes only the host: {:#}", folded["paths"]);
        let kept = with("basePath", json!(["/v3", "/v4"]));
        assert!(kept["paths"]["/login"]["servers"].is_array(), "{:#}", kept["paths"]);
    }
}
