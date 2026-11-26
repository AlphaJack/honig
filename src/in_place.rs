//! Schemas that apply themselves to the same value.
//!
//! A `$ref` that leads back to its own schema through `allOf`, `anyOf`, `oneOf`, `not`, `if`, `then`, `else` or
//! `dependentSchemas` alone, and never through `properties` or `items`, checks that schema again on the same value
//! without end. JSON Schema 2020-12 section 9.4.1 leaves its meaning undefined. Where such a schema meets itself again
//! on the same value, it accepts no value there: the least meaning, which `allOf` and `anyOf` keep. A cycle through
//! `oneOf`, `not` or `if` has no such meaning: a branch that accepts nothing changes what the others mean. It is an
//! error.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::diag::Diagnostics;
use crate::ir::types::scc;
use crate::json::{Doc, escape_token, is_data, join, percent_decode, pointer, unescape_token};

/// A `$ref` that a schema applies to its own value.
struct Edge {
    /// The pointer of the schema that the `$ref` refers to.
    to: String,
    /// The pointer of the `$ref`.
    at: String,
    /// `oneOf`, `not` or `if` when the `$ref` lies under one: the keyword negates or counts what the `$ref` accepts.
    through: Option<&'static str>,
}

/// Reports each schema that applies itself to the same value and rewrites it to its least meaning, so that neither
/// the generator nor generated code checks it without end.
pub(crate) fn cut(root: &mut Value, diags: &mut Diagnostics) {
    let doc = Doc::new(root.clone());
    let graph = graph(&doc);
    let mut rewritten = vec![];
    for group in cycles(&graph) {
        let negated: Vec<&Edge> =
            (group.iter().flat_map(|p| &graph[p])).filter(|e| e.through.is_some() && group.contains(&e.to)).collect();
        for e in &negated {
            let through = e.through.unwrap_or_default();
            diags.error(
                &e.at,
                format!(
                    "refers back to the schema that it is in, on the same value, through `{through}`: checking it \
                     would never end (JSON Schema 2020-12 section 9.4.1), and a `{through}` has no meaning with the \
                     reference cut. Move the reference under `properties` or `items`, or remove it"
                ),
            );
        }
        if !negated.is_empty() {
            continue;
        }
        rewritten.extend(least(&doc, &group, diags));
    }
    for (p, least) in rewritten {
        if let Some(slot) = root.pointer_mut(&canonical(&p)) {
            *slot = least;
        }
    }
}

/// Returns the `$ref`s that each schema a `$ref` refers to applies to its own value.
fn graph(doc: &Doc) -> BTreeMap<String, Vec<Edge>> {
    let mut targets = BTreeSet::new();
    references(&doc.root, "", &mut |r| {
        if let Some(p) = doc.local_pointer(r) {
            targets.insert(p);
        }
    });
    let mut graph = BTreeMap::new();
    for p in targets {
        let mut out = vec![];
        if let Some(s) = pointer(&doc.root, &p) {
            edges(doc, s, &p, None, &mut out);
        }
        graph.insert(p, out);
    }
    graph
}

/// Calls `f` with each `$ref` of `v`, found under the key `parent`. Data such as `enum` or an example contains none.
fn references(v: &Value, parent: &str, f: &mut dyn FnMut(&str)) {
    match v {
        Value::Object(o) => {
            if let Some(r) = o.get("$ref").and_then(Value::as_str) {
                f(r);
            }
            o.iter().filter(|(k, _)| !is_data(parent, k)).for_each(|(k, x)| references(x, k, f));
        }
        Value::Array(a) => a.iter().for_each(|x| references(x, parent, f)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Collects the `$ref`s that `s` at `at` applies to its own value into `out`.
fn edges(doc: &Doc, s: &Value, at: &str, through: Option<&'static str>, out: &mut Vec<Edge>) {
    let Some(o) = s.as_object() else { return };
    if let Some(to) = o.get("$ref").and_then(Value::as_str).and_then(|r| doc.local_pointer(r)) {
        out.push(Edge { to, at: join(at, "$ref"), through });
    }
    for (key, sub_at, sub) in in_place(o, at) {
        let negates = matches!(key, "oneOf" | "not" | "if").then_some(key);
        edges(doc, sub, &sub_at, through.or(negates), out);
    }
}

/// Returns the subschemas of `o` at `at` that apply to the same value, with their keyword and pointer.
fn in_place<'a>(o: &'a Map<String, Value>, at: &str) -> Vec<(&'static str, String, &'a Value)> {
    let mut out = vec![];
    for key in ["allOf", "anyOf", "oneOf"] {
        let base = join(at, key);
        let items = o.get(key).and_then(Value::as_array).into_iter().flatten().enumerate();
        out.extend(items.map(|(i, s)| (key, join(&base, &i.to_string()), s)));
    }
    for key in ["not", "if", "then", "else"] {
        out.extend(o.get(key).map(|s| (key, join(at, key), s)));
    }
    let base = join(at, "dependentSchemas");
    let dependent = o.get("dependentSchemas").and_then(Value::as_object).into_iter().flatten();
    out.extend(dependent.map(|(name, s)| ("dependentSchemas", join(&base, name), s)));
    out
}

/// Returns the groups of schemas that apply each other to the same value in a cycle.
fn cycles(graph: &BTreeMap<String, Vec<Edge>>) -> Vec<BTreeSet<String>> {
    let keys: Vec<&String> = graph.keys().collect();
    let id: BTreeMap<&str, usize> = keys.iter().enumerate().map(|(i, k)| (k.as_str(), i)).collect();
    let next: Vec<Vec<usize>> =
        (graph.values()).map(|edges| edges.iter().filter_map(|e| id.get(e.to.as_str()).copied()).collect()).collect();
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (v, c) in scc(keys.len(), &next).into_iter().enumerate() {
        groups.entry(c).or_default().push(v);
    }
    (groups.into_values())
        .filter(|g| g.len() > 1 || g.iter().any(|v| next[*v].contains(v)))
        .map(|g| g.into_iter().map(|v| keys[v].clone()).collect())
        .collect()
}

/// How many copies of a schema one group may inline. Each schema of a group is expanded once per way through the
/// others, which grows as the factorial of the group: a dozen schemas that each refer to all others would not end.
const COPIES: usize = 4096;

/// Returns each schema of `group` with its least meaning and reports what was cut.
fn least(doc: &Doc, group: &BTreeSet<String>, diags: &mut Diagnostics) -> Vec<(String, Value)> {
    let mut e = Expansion { doc, group, cuts: BTreeSet::new(), left: COPIES, exhausted: false };
    let mut out = vec![];
    for p in group {
        let own = pointer(&doc.root, p).cloned().unwrap_or(Value::Bool(true));
        let least = e.expand(&own, p, &mut vec![p.clone()]);
        if e.exhausted {
            let first = group.first().map_or("", String::as_str);
            diags.error(
                first,
                format!(
                    "{} schemas apply each other on the same value in too many ways to give each a meaning (JSON \
                     Schema 2020-12 section 9.4.1): move the references between them under `properties` or `items`",
                    group.len()
                ),
            );
            return group.iter().map(|p| (p.clone(), Value::Bool(false))).collect();
        }
        let least = least.unwrap_or_else(|| {
            diags.error(
                p,
                "accepts no value: each way through it refers back to it on the same value (JSON Schema 2020-12 \
                 section 9.4.1)",
            );
            Value::Bool(false)
        });
        out.push((p.clone(), least));
    }
    for at in e.cuts {
        diags.warning(
            at,
            "refers back to the schema that it is in, on the same value: checking it would never end (JSON Schema \
             2020-12 section 9.4.1), so here it accepts no value",
        );
    }
    out
}

/// The expansion of the schemas of one group.
struct Expansion<'d> {
    doc: &'d Doc,
    group: &'d BTreeSet<String>,
    /// The pointer of each `$ref` cut.
    cuts: BTreeSet<String>,
    /// How many copies may still be inlined.
    left: usize,
    /// Whether one more copy was needed than [`COPIES`].
    exhausted: bool,
}

impl Expansion<'_> {
    /// Returns `s` at `at` with each `$ref` into the group replaced by the schema that it refers to, and each one back
    /// to a schema of `path` cut. `None` when `s` then accepts no value, or when no copy is left.
    fn expand(&mut self, s: &Value, at: &str, path: &mut Vec<String>) -> Option<Value> {
        let Some(o) = s.as_object() else { return Some(s.clone()) };
        let mut out = o.clone();
        let mut all = vec![];
        for (i, m) in o.get("allOf").and_then(Value::as_array).into_iter().flatten().enumerate() {
            all.push(self.expand(m, &join(&join(at, "allOf"), &i.to_string()), path)?);
        }
        if let Some(Value::Array(a)) = o.get("anyOf") {
            let base = join(at, "anyOf");
            let left: Vec<Value> = (a.iter().enumerate())
                .filter_map(|(i, b)| self.expand(b, &join(&base, &i.to_string()), path))
                .collect();
            if left.is_empty() {
                return None;
            }
            out.insert("anyOf".to_owned(), Value::Array(left));
        }
        for key in ["then", "else"] {
            if let Some(sub) = o.get(key) {
                out.insert(key.to_owned(), self.expand(sub, &join(at, key), path).unwrap_or(Value::Bool(false)));
            }
        }
        if let Some(Value::Object(m)) = o.get("dependentSchemas") {
            let base = join(at, "dependentSchemas");
            let least = (m.iter())
                .map(|(name, sub)| {
                    (name.clone(), self.expand(sub, &join(&base, name), path).unwrap_or(Value::Bool(false)))
                })
                .collect();
            out.insert("dependentSchemas".to_owned(), Value::Object(least));
        }
        let reference = o.get("$ref").and_then(Value::as_str).and_then(|r| self.doc.local_pointer(r));
        if let Some(to) = reference.filter(|to| self.group.contains(to)) {
            if path.contains(&to) {
                self.cuts.insert(join(at, "$ref"));
                return None;
            }
            if self.left == 0 {
                self.exhausted = true;
                return None;
            }
            self.left -= 1;
            let target = pointer(&self.doc.root, &to).cloned().unwrap_or(Value::Bool(true));
            path.push(to.clone());
            let inner = self.expand(&target, &to, path);
            path.pop();
            out.remove("$ref");
            all.insert(0, without_ids(inner?));
        }
        if o.contains_key("allOf") || !all.is_empty() {
            out.insert("allOf".to_owned(), Value::Array(all));
        }
        Some(Value::Object(out))
    }
}

/// Returns `s` without `$id`, `$anchor` and `$dynamicAnchor`: a copy must not take the place of the schema that
/// declares them.
fn without_ids(mut s: Value) -> Value {
    fn strip(v: &mut Value, parent: &str) {
        match v {
            Value::Object(o) => {
                for k in ["$id", "$anchor", "$dynamicAnchor"] {
                    o.remove(k);
                }
                o.iter_mut().filter(|(k, _)| !is_data(parent, k)).for_each(|(k, x)| strip(x, k));
            }
            Value::Array(a) => a.iter_mut().for_each(|x| strip(x, parent)),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    strip(&mut s, "");
    s
}

/// Returns `p` as `serde_json` reads a pointer: each token percent-decoded and escaped once.
fn canonical(p: &str) -> String {
    p.split('/').map(|t| escape_token(&unescape_token(&percent_decode(t)))).collect::<Vec<_>>().join("/")
}
