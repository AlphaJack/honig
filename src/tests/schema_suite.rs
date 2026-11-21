//! The JSON Schema Test Suite against generated types. Each case of `tests/fixtures/json-schema-test-suite` that applies
//! to 2020-12 becomes the field `v` of an object of one description. Its type must decode every valid instance and refuse every invalid
//! one. `json-schema-test-suite.known` lists the tests that do not hold today, each with its reason. The test fails on
//! a test that does not hold and is not listed, and on a listed test that holds: the list stays true. It rewrites the
//! list when it is stale.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

/// The release of JSON Schema that `OpenAPI` 3.1 and 3.2 use.
const DIALECT: u32 = 2020;

/// The base of the `$id` that each case gets, so that its `#` is the case and not the description.
const BASE: &str = "https://suite.honig.test/case/";

/// Where the suite's cases retrieve the documents that they refer to.
const REMOTE: &str = "http://localhost:1234/";

/// One schema of the suite and its tests.
struct Case {
    schema: Value,
    /// The documents that the schema refers to, by the URI that it retrieves them at (`externalSchemas`).
    external: serde_json::Map<String, Value>,
    /// Each test as (its key, its instance, whether the instance is valid).
    tests: Vec<(String, Value, bool)>,
}

#[test]
#[ignore = "compiles generated crates"]
fn generated_types_follow_the_json_schema_test_suite() {
    let cases = cases();
    let mut outcome = BTreeMap::new();
    let mut decoded = vec![];
    for (i, case) in cases.iter().enumerate() {
        match generable(i, case) {
            Ok(()) => decoded.push(i),
            Err(reason) => outcome.extend(case.tests.iter().map(|(key, _, _)| (key.clone(), Some(reason.clone())))),
        }
    }
    outcome.extend(decode_all(&cases, &decoded));
    let found: BTreeMap<String, String> =
        outcome.into_iter().filter_map(|(key, reason)| reason.map(|r| (key, r))).collect();
    let path = known_path();
    let known = read_known(&path);
    let new: Vec<&String> = found.keys().filter(|k| !known.contains_key(*k)).collect();
    let fixed: Vec<&String> = known.keys().filter(|k| !found.contains_key(*k)).collect();
    if !new.is_empty() || !fixed.is_empty() {
        let text: String = found.iter().map(|(key, reason)| format!("{key}\t{reason}\n")).collect();
        std::fs::write(&path, text).unwrap();
    }
    assert!(new.is_empty(), "tests that do not hold, now listed in {}: {new:#?}", path.display());
    assert!(fixed.is_empty(), "listed tests that hold, now unlisted in {}: {fixed:#?}", path.display());
}

/// Every case of the suite that applies to 2020-12, in the order of its files.
fn cases() -> Vec<Case> {
    let dir = super::common::fixtures().join("json-schema-test-suite");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let mut out = vec![];
    let mut seen = BTreeMap::new();
    for file in files {
        let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        for case in doc["tests"].as_array().unwrap() {
            if !compatible(case["compatibility"].as_str()) {
                continue;
            }
            let name = format!("{stem}: {}", case["description"].as_str().unwrap_or_default());
            let tests = case["tests"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| {
                    let key = unique(&mut seen, format!("{name}: {}", t["description"].as_str().unwrap_or_default()));
                    (key, t["data"].clone(), t["valid"].as_bool().unwrap())
                })
                .collect();
            let external = case["externalSchemas"].as_object().cloned().unwrap_or_default();
            out.push(Case { schema: case["schema"].clone(), external, tests });
        }
    }
    out
}

/// Returns `key`, or `key (n)` for its n-th repeat. A few descriptions repeat within a file.
fn unique(seen: &mut BTreeMap<String, usize>, key: String) -> String {
    let n = seen.entry(key.clone()).or_insert(0);
    *n += 1;
    if *n == 1 { key } else { format!("{key} ({n})") }
}

/// Does a case with `compatibility` apply to 2020-12? The algorithm of the suite's README: each constraint is a
/// minimum (`7`), a maximum (`<=2019`) or one release (`=2020`). The first that reaches 2020 decides.
fn compatible(compatibility: Option<&str>) -> bool {
    let Some(text) = compatibility else { return true };
    let mut valid = true;
    for constraint in text.split(',') {
        let (op, release) = constraint
            .strip_prefix("<=")
            .map(|r| ("<=", r))
            .or_else(|| constraint.strip_prefix('=').map(|r| ("=", r)))
            .unwrap_or(("", constraint));
        let release: u32 = release.trim().parse().unwrap();
        valid = match op {
            "<=" => DIALECT <= release,
            "=" => DIALECT == release,
            _ => DIALECT >= release,
        };
        if DIALECT <= release {
            break;
        }
    }
    valid
}

/// The schema of case `i` as a component: an object whose required `v` is the schema of the case. A type alias does
/// not check its schema where it is declared, a field does. The schema gets an `$id` of its own unless it has one, so
/// that its `#` is the case and not the description.
fn component(i: usize, case: &Case) -> Value {
    let mut schema = case.schema.clone();
    if let Some(o) = schema.as_object_mut() {
        o.entry("$id").or_insert_with(|| json!(format!("{BASE}{i}")));
        embed(o, &case.external);
    }
    // Cases retrieve different documents at the same URI: each case gets URIs of its own.
    let text = schema.to_string().replace(REMOTE, &format!("{REMOTE}case{i}/"));
    let schema: Value = serde_json::from_str(&text).unwrap();
    json!({"type": "object", "required": ["v"], "properties": {"v": schema}})
}

/// Puts each document that a case retrieves into the `$defs` of its schema, with the URI that it is retrieved at as its
/// `$id`, as the suite's README suggests. A document with an `$id` of its own keeps it: the URI becomes a `$ref` to it.
fn embed(schema: &mut serde_json::Map<String, Value>, external: &serde_json::Map<String, Value>) {
    // A metaschema that `$schema` names picks vocabularies, which honig does not read: it is no schema of the value.
    let meta = schema.get("$schema").and_then(Value::as_str).map(str::to_owned);
    for (n, (uri, document)) in
        external.iter().enumerate().filter(|(_, (uri, _))| meta.as_deref() != Some(uri.as_str()))
    {
        let mut document = document.clone();
        let own = document.get("$id").and_then(Value::as_str).filter(|id| *id != uri).map(str::to_owned);
        let defs = schema.entry("$defs").or_insert_with(|| json!({})).as_object_mut().unwrap();
        if let Some(own) = own {
            defs.insert(format!("retrieved{n}"), json!({"$id": uri, "$ref": own}));
        } else if let Some(o) = document.as_object_mut() {
            o.insert("$id".to_owned(), json!(uri));
        }
        defs.insert(format!("external{n}"), document);
    }
}

/// Does case `i` generate on its own? A case that refers to another document is not tried: that would fetch it.
fn generable(i: usize, case: &Case) -> Result<(), String> {
    // The documents that the case retrieves may refer further: a document outside them would be fetched.
    if refers_outside(&component(i, case)) {
        return Err("refers to another document".to_owned());
    }
    let doc = super::common::spec("3.2.0", &json!({}), &json!({"schemas": {format!("Case{i}"): component(i, case)}}));
    let spec = super::common::write(&format!("suite-{i}"), &doc, &[]);
    // An error stops generation, also when files come out anyway.
    let diagnostics = match crate::generate::generate(&spec, &crate::emit::Options::default()) {
        Ok(generated) => generated.diagnostics,
        Err(d) => d,
    };
    match diagnostics.0.iter().find(|x| x.level == crate::diag::Level::Error) {
        Some(error) => Err(format!("does not generate: {}", one_line(&error.message))),
        None => Ok(()),
    }
}

/// Does `schema` hold a reference to a document that no `$id` within it names? Each `$id` and reference resolves against
/// the base of the resource around it.
fn refers_outside(schema: &Value) -> bool {
    let mut ids = vec![];
    let mut refs = vec![];
    collect(schema, "", &mut ids, &mut refs);
    refs.iter().any(|r| {
        let doc = r.split('#').next().unwrap_or_default();
        doc.contains(':') && !ids.iter().any(|id: &String| id.trim_end_matches('#') == doc)
    })
}

/// Collects each `$id` and each `$ref` or `$dynamicRef` under `v`, resolved against `base`.
fn collect(v: &Value, base: &str, ids: &mut Vec<String>, refs: &mut Vec<String>) {
    match v {
        Value::Object(o) => {
            let base = match o.get("$id").and_then(Value::as_str) {
                Some(id) => crate::schema_ids::resolve(base, id),
                None => base.to_owned(),
            };
            ids.push(base.clone());
            for (key, value) in o {
                match (key.as_str(), value.as_str()) {
                    ("$id", Some(_)) => {}
                    ("$ref" | "$dynamicRef", Some(r)) => refs.push(crate::schema_ids::resolve(&base, r)),
                    _ => collect(value, &base, ids, refs),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|x| collect(x, base, ids, refs)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Builds one crate with the types of the cases in `decoded` and decodes every test of them. Returns each key with
/// `None` when the test holds and its reason when it does not.
fn decode_all(cases: &[Case], decoded: &[usize]) -> Vec<(String, Option<String>)> {
    let schemas: serde_json::Map<String, Value> =
        decoded.iter().map(|&i| (format!("Case{i}"), component(i, &cases[i]))).collect();
    let doc = super::common::spec("3.2.0", &json!({}), &json!({"schemas": schemas}));
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("gen-tests").join("schema-suite");
    // A missing directory is fine: there is nothing to clean up.
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let spec = dir.join("openapi.json");
    std::fs::write(&spec, serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
    super::common::new_crate(&spec, &dir, "schema-suite", &crate::emit::Options::default());
    super::common::use_local_runtime(&dir.join("Cargo.toml"));
    std::fs::write(dir.join("src").join("lib.rs"), decoder(decoded)).unwrap();
    let instances: Vec<Value> =
        decoded.iter().flat_map(|&i| cases[i].tests.iter().map(move |(_, data, _)| json!([i, {"v": data}]))).collect();
    std::fs::write(dir.join("instances.json"), serde_json::to_vec(&instances).unwrap()).unwrap();
    let out = Command::new(env!("CARGO"))
        .args(["test", "--quiet", "--lib", "--", "--nocapture"])
        .current_dir(&dir)
        .env("CARGO_TARGET_DIR", dir.parent().unwrap().join("target"))
        .output()
        .expect("cargo runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "the suite crate does not build: {}", String::from_utf8_lossy(&out.stderr));
    let mut results = stdout.lines().filter_map(|l| l.strip_prefix("decoded\t"));
    let mut outcome = vec![];
    for &i in decoded {
        for (key, _, valid) in &cases[i].tests {
            let result = results.next().unwrap_or_else(|| panic!("no result for {key}"));
            let reason = match (result.strip_prefix("err\t"), valid) {
                (None, true) | (Some(_), false) => None,
                (None, false) => Some("decodes an invalid instance".to_owned()),
                (Some(e), true) => Some(format!("refuses a valid instance: {}", one_line(e))),
            };
            outcome.push((key.clone(), reason));
        }
    }
    outcome
}

/// The `lib.rs` of the suite crate. Its test decodes each instance of `instances.json` as the type of its case, as
/// generated code decodes a JSON body: `1.0` is an integer there. It
/// prints `decoded<TAB>ok` or `decoded<TAB>err<TAB><error>` per instance, in order. `ARMS` is one arm per case.
const DECODER: &str = r#"pub mod api;

fn decode(case: u64, data: serde_json::Value) -> Result<(), api::runtime::ClientError> {
    let body = serde_json::to_vec(&data).unwrap();
    match case {
ARMS        _ => unreachable!("no case {case}"),
    }
}

#[test]
fn decode_every_instance() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/instances.json")).unwrap();
    let instances: Vec<(u64, serde_json::Value)> = serde_json::from_str(&text).unwrap();
    for (case, data) in instances {
        match decode(case, data) {
            Ok(()) => println!("decoded\tok"),
            Err(api::runtime::ClientError::Decode { message, .. }) => println!("decoded\terr\t{}", message.replace(['\n', '\t'], " ")),
            Err(e) => println!("decoded\terr\t{}", e.to_string().replace(['\n', '\t'], " ")),
        }
    }
}
"#;

fn decoder(decoded: &[usize]) -> String {
    let arms: String = decoded
        .iter()
        .map(|i| format!("        {i} => api::runtime::json_decode::<api::types::Case{i}>(None, &body).map(drop),\n"))
        .collect();
    DECODER.replace("ARMS", &arms)
}

/// The first line of `text`, cut to a length that a list line holds.
fn one_line(text: &str) -> String {
    const LENGTH: usize = 120;
    text.lines().next().unwrap_or_default().chars().take(LENGTH).collect()
}

fn known_path() -> PathBuf {
    super::common::fixtures().join("json-schema-test-suite.known")
}

/// The listed tests as (key, reason): one per line, the key and the reason split by a tab.
fn read_known(path: &Path) -> BTreeMap<String, String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().filter_map(|l| l.split_once('\t')).map(|(k, r)| (k.to_owned(), r.to_owned())).collect()
}
