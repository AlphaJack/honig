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

/// One schema of the suite and its tests.
struct Case {
    schema: Value,
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
            out.push(Case { schema: case["schema"].clone(), tests });
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
    }
    json!({"type": "object", "required": ["v"], "properties": {"v": schema}})
}

/// Does case `i` generate on its own? A case that refers to another document is not tried: that would fetch it.
fn generable(i: usize, case: &Case) -> Result<(), String> {
    if refers_outside(&case.schema) {
        return Err("refers to another document".to_owned());
    }
    let doc = super::common::spec("3.2.0", &json!({}), &json!({"schemas": {format!("Case{i}"): component(i, case)}}));
    let spec = super::common::write(&format!("suite-{i}"), &doc, &[]);
    crate::generate::generate(&spec, &crate::emit::Options::default()).map(drop).map_err(|d| {
        let first = d.0.iter().find(|x| x.level == crate::diag::Level::Error).map(|x| x.message.clone());
        format!("does not generate: {}", one_line(&first.unwrap_or_default()))
    })
}

/// Does `schema` hold a `$ref` that no `$id` within it names, other than a fragment of its own?
fn refers_outside(schema: &Value) -> bool {
    let mut ids = vec![];
    let mut refs = vec![];
    collect(schema, &mut ids, &mut refs);
    refs.iter().any(|r| {
        let doc = r.split('#').next().unwrap_or_default();
        !doc.is_empty() && !ids.iter().any(|id: &&str| id.trim_end_matches('#').ends_with(doc))
    })
}

fn collect<'a>(v: &'a Value, ids: &mut Vec<&'a str>, refs: &mut Vec<&'a str>) {
    match v {
        Value::Object(o) => {
            for (key, value) in o {
                match (key.as_str(), value.as_str()) {
                    ("$id", Some(id)) => ids.push(id),
                    ("$ref" | "$dynamicRef", Some(r)) => refs.push(r),
                    _ => collect(value, ids, refs),
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|x| collect(x, ids, refs)),
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

/// The `lib.rs` of the suite crate. Its test decodes each instance of `instances.json` as the type of its case. It
/// prints `decoded<TAB>ok` or `decoded<TAB>err<TAB><error>` per instance, in order. `ARMS` is one arm per case.
const DECODER: &str = r#"pub mod api;

fn decode(case: u64, data: serde_json::Value) -> Result<(), serde_json::Error> {
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
            Err(e) => println!("decoded\terr\t{}", e.to_string().replace(['\n', '\t'], " ")),
        }
    }
}
"#;

fn decoder(decoded: &[usize]) -> String {
    let arms: String = decoded
        .iter()
        .map(|i| format!("        {i} => serde_json::from_value::<api::types::Case{i}>(data).map(drop),\n"))
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
