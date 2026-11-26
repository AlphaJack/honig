//! Union branches that one value can match. An `anyOf`, or a `oneOf` without a discriminator, whose object branches
//! nothing tells apart decodes such a value as the first and drops what the others carry: a warning says so.
//!
//! A discriminated `oneOf` whose branches can all match one value. 3.2 says a discriminator "MUST NOT change the
//! validation outcome": `oneOf` still means exactly one. Branches `Book` and `Gadget` that share every property both
//! match `{"kind": "book", "name": "Dune"}`, and the value is invalid. A branch that sets the discriminating property
//! with `const` (or a one-value `enum`) cannot match another branch's tag. A warning lists the branches that do not.
//!
//! Two branches of a `oneOf` that are the same schema, but for annotations, refuse every value of that schema: each such
//! value matches both. A warning says so. When every branch has such a twin, the `oneOf` refuses every value: an error.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json::{Doc, join};
use crate::walk::{self, Obj, Visit};

/// Warns about each discriminated `oneOf` with a branch that does not pin its tag, and about each union with two
/// object branches that nothing tells apart. Reports each `oneOf` whose every branch has a twin.
pub(crate) fn check(doc: &Doc, diags: &mut Diagnostics) {
    let mut found = Found { doc, out: vec![], errors: vec![] };
    walk::walk(&mut doc.root.clone(), &mut found);
    for (ptr, message) in found.out {
        diags.warning(&ptr, message);
    }
    for (ptr, message) in found.errors {
        diags.error(&ptr, message);
    }
}

/// Keywords that never change which values a schema accepts.
const ANNOTATIONS: [&str; 8] =
    ["title", "description", "summary", "examples", "example", "deprecated", "$comment", "externalDocs"];

struct Found<'a> {
    doc: &'a Doc,
    out: Vec<(String, String)>,
    errors: Vec<(String, String)>,
}

/// What tells an object branch apart: its properties, the required ones and the `const` and `type` of each. Being closed
/// does not: a closed branch still accepts a value of the properties that both declare.
#[derive(Default)]
struct Object {
    properties: BTreeSet<String>,
    required: BTreeSet<String>,
    consts: BTreeMap<String, Value>,
    types: BTreeMap<String, Value>,
}

impl Object {
    /// Can no value match both `self` and `other`? A required property of one that the other does not declare or
    /// types otherwise, or a `const` property with another value, tells them apart.
    fn apart(&self, other: &Self) -> bool {
        let missing = |a: &Self, b: &Self| {
            a.required.iter().any(|r| {
                !b.properties.contains(r) || a.types.get(r).is_some_and(|t| b.types.get(r).is_some_and(|u| u != t))
            })
        };
        let consts = self.consts.iter().any(|(k, v)| other.consts.get(k).is_some_and(|w| w != v));
        missing(self, other) || missing(other, self) || consts
    }
}

impl Found<'_> {
    /// Returns the object that `branch` describes through `$ref`s and `allOf` members, or `None` for a branch that is
    /// no object or declares no property. `seen` contains the `$ref`s on the way: one met again is a cycle, which adds nothing.
    fn object(&self, branch: &Value, seen: &mut Vec<String>) -> Option<Object> {
        if let Some(r) = branch.get("$ref").and_then(Value::as_str) {
            if seen.iter().any(|s| s == r) {
                return Some(Object::default());
            }
            seen.push(r.to_owned());
            let found = self.doc.get(r).and_then(|t| self.object(t, seen));
            seen.pop();
            return found;
        }
        let typed = branch.get("type").and_then(Value::as_str);
        if typed.is_some_and(|t| t != "object") || (typed.is_none() && branch.get("properties").is_none()) {
            return None;
        }
        let mut o = Object::default();
        for (name, p) in branch.get("properties").and_then(Value::as_object).into_iter().flatten() {
            o.properties.insert(name.clone());
            if let Some(c) = p.get("const") {
                o.consts.insert(name.clone(), c.clone());
            }
            if let Some(t) = p.get("type") {
                o.types.insert(name.clone(), t.clone());
            }
        }
        o.required.extend(
            branch
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(String::from),
        );
        for m in branch.get("allOf").and_then(Value::as_array).into_iter().flatten() {
            let m = self.object(m, seen).unwrap_or_default();
            o.properties.extend(m.properties);
            o.required.extend(m.required);
            o.consts.extend(m.consts);
            o.types.extend(m.types);
        }
        (!o.properties.is_empty()).then_some(o)
    }

    /// Records an error for each two object branches of an `anyOf`, or of a `oneOf` without a discriminator, that no
    /// keyword tells apart.
    fn alike(&mut self, s: &Obj, ptr: &str) {
        for key in ["anyOf", "oneOf"].into_iter().filter(|k| *k == "anyOf" || !s.contains_key("discriminator")) {
            let branches = s.get(key).and_then(Value::as_array).into_iter().flatten().enumerate();
            let objects: Vec<(String, Object)> =
                branches.filter_map(|(i, b)| Some((name(b, i), self.object(b, &mut vec![])?))).collect();
            for (i, (a, x)) in objects.iter().enumerate() {
                for (b, _) in objects[i + 1..].iter().filter(|(_, y)| !x.apart(y)) {
                    self.out.push((
                        join(ptr, key),
                        format!(
                            "a value can match both {a} and {b}, and it decodes as the first: add a required \
                             property or a `const` tag that tells them apart"
                        ),
                    ));
                }
            }
        }
    }

    /// Returns the branches of the `oneOf` of `s` that are the same schema as an earlier one once their `$ref`s are
    /// followed and their annotations dropped, each with that earlier one, and whether every branch has such a twin.
    fn twins(&self, s: &Obj) -> (Vec<(String, String)>, bool) {
        let Some(branches) = s.get("oneOf").and_then(Value::as_array) else { return (vec![], false) };
        let plain: Vec<Value> = branches.iter().map(|b| self.plain(b)).collect();
        let twin_of = |i: usize| (0..plain.len()).find(|j| *j != i && plain[*j] == plain[i]);
        let pairs = (0..plain.len())
            .filter_map(|i| twin_of(i).filter(|j| *j < i).map(|j| (name(&branches[j], j), name(&branches[i], i))))
            .collect();
        (pairs, !plain.is_empty() && (0..plain.len()).all(|i| twin_of(i).is_some()))
    }

    /// Returns `branch` with its `$ref`s to whole schemas followed and its annotations dropped.
    fn plain(&self, branch: &Value) -> Value {
        let mut seen = vec![];
        let mut b = branch;
        while let Some(r) = b.get("$ref").and_then(Value::as_str)
            && b.as_object().is_some_and(|o| o.keys().all(|k| k == "$ref" || annotation(k)))
            && !seen.contains(&r)
            && let Some(t) = self.doc.get(r)
        {
            seen.push(r);
            b = t;
        }
        without_annotations(b, "")
    }

    /// Returns whether the branch through `$ref`s and `allOf` members fixes `prop` to one value. `seen` contains the
    /// `$ref`s on the way: one met again is a cycle, which fixes nothing more.
    fn pins(&self, branch: &Value, prop: &str, seen: &mut Vec<String>) -> bool {
        let branch = match branch.get("$ref").and_then(Value::as_str) {
            Some(r) if seen.iter().any(|s| s == r) => return false,
            Some(r) => match self.doc.get(r) {
                Some(t) => {
                    seen.push(r.to_owned());
                    let pinned = self.pins(t, prop, seen);
                    seen.pop();
                    return pinned;
                }
                None => return false,
            },
            None => branch,
        };
        let own = branch.get("properties").and_then(|p| p.get(prop)).is_some_and(crate::json::fixes_one_value);
        own || branch.get("allOf").and_then(Value::as_array).is_some_and(|m| m.iter().any(|m| self.pins(m, prop, seen)))
    }
}

/// Is `key` a keyword that never changes which values a schema accepts?
fn annotation(key: &str) -> bool {
    ANNOTATIONS.contains(&key) || key.starts_with("x-")
}

/// Returns `v`, found under the key `parent`, without its annotations. Data and the names of a map of schemas stay.
fn without_annotations(v: &Value, parent: &str) -> Value {
    let names = matches!(parent, "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas");
    match v {
        Value::Object(o) => Value::Object(
            (o.iter())
                .filter(|(k, _)| names || !annotation(k))
                .map(|(k, x)| {
                    let x = if crate::json::is_data(parent, k) { x.clone() } else { without_annotations(x, k) };
                    (k.clone(), x)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(|x| without_annotations(x, parent)).collect()),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => v.clone(),
    }
}

/// Returns how a message names branch `i`: its `$ref` or its position.
fn name(branch: &Value, i: usize) -> String {
    branch.get("$ref").and_then(Value::as_str).map_or_else(|| format!("#{i}"), String::from)
}

impl Visit for Found<'_> {
    fn schema(&mut self, s: &mut Obj, ptr: &str) {
        let (twins, all) = self.twins(s);
        if all {
            self.errors.push((
                join(ptr, "oneOf"),
                "each branch is the same schema as another, so a value matches two or none and `oneOf` refuses every \
                 value: tell them apart with a `const` tag or a required property"
                    .to_owned(),
            ));
            return;
        }
        for (a, b) in twins {
            self.out.push((
                join(ptr, "oneOf"),
                format!("{a} and {b} are the same schema, so `oneOf` refuses every value of it: remove one"),
            ));
        }
        self.alike(s, ptr);
        let Some(prop) = s.get("discriminator").and_then(|d| d.get("propertyName")).and_then(Value::as_str) else {
            return;
        };
        let Some(branches) = s.get("oneOf").and_then(Value::as_array) else { return };
        if branches.len() < 2 {
            return;
        }
        let loose: Vec<String> = branches
            .iter()
            .enumerate()
            .filter(|(_, b)| !self.pins(b, prop, &mut vec![]))
            .map(|(i, b)| name(b, i))
            .collect();
        // Two branches that only the tag tells apart, and one leaves it open: a value can match both.
        let rest = |b: &Value| {
            self.object(b, &mut vec![]).map(|mut o| {
                o.properties.remove(prop);
                o.required.remove(prop);
                o.consts.remove(prop);
                o.types.remove(prop);
                o
            })
        };
        let objects: Vec<(String, Option<Object>)> =
            branches.iter().enumerate().map(|(i, b)| (name(b, i), rest(b))).collect();
        let tied = objects.iter().enumerate().find_map(|(i, (a, x))| {
            objects[i + 1..].iter().find_map(|(b, y)| {
                let open = loose.contains(a) || loose.contains(b);
                (open && x.as_ref().zip(y.as_ref()).is_some_and(|(x, y)| !x.apart(y))).then(|| (a.clone(), b.clone()))
            })
        });
        if let Some((a, b)) = tied {
            self.out.push((
                join(ptr, "oneOf"),
                format!(
                    "{a} and {b} differ only by `{prop}`, which is not fixed, so a value matches both and `oneOf` refuses it: pin `{prop}` with `const` in each branch"
                ),
            ));
        } else if !loose.is_empty() {
            self.out.push((
                join(ptr, "oneOf"),
                format!(
                    "a value can match more than one branch: `{prop}` is not fixed with `const` in {}. `oneOf` needs \
                     exactly one match and the discriminator does not change that (3.2), so such a value is refused",
                    loose.join(", ")
                ),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn warnings(doc: Value) -> Vec<String> {
        let mut d = Diagnostics::default();
        check(&Doc::new(doc), &mut d);
        d.iter().map(|x| x.message.clone()).collect()
    }

    /// A closed branch still accepts a value of the properties that both declare: it tells two branches apart only
    /// when the other requires a property that it does not declare.
    #[test]
    fn a_closed_branch_still_overlaps_a_wider_one() {
        let union = |b: Value| {
            json!({"openapi": "3.2.0", "components": {"schemas": {"U": {"anyOf": [
                {"type": "object", "additionalProperties": false, "properties": {"a": {"type": "string"}}}, b]}}}})
        };
        let wider = json!({"type": "object", "properties": {"a": {"type": "string"}, "b": {"type": "string"}}});
        assert_eq!(warnings(union(wider)).len(), 1, "`{{\"a\": \"x\"}}` matches both");
        let needs_b = json!({"type": "object", "required": ["b"], "properties": {"b": {"type": "string"}}});
        assert_eq!(warnings(union(needs_b)), Vec::<String>::new());
    }

    #[test]
    fn branches_that_do_not_pin_the_tag_are_named() {
        let item = |tag: Value| json!({"type": "object", "properties": {"kind": tag, "name": {"type": "string"}}});
        let loose = json!({"openapi": "3.2.0", "components": {"schemas": {
            "Item": {"oneOf": [{"$ref": "#/components/schemas/Book"}, {"$ref": "#/components/schemas/Gadget"}],
                    "discriminator": {"propertyName": "kind"}},
            "Book": item(json!({"type": "string"})),
            "Gadget": {"allOf": [item(json!({"const": "gadget"})),
                {"required": ["batteries"], "properties": {"batteries": {"type": "boolean"}}}]}
        }}});
        let w = warnings(loose);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("#/components/schemas/Book") && !w[0].contains("Gadget"), "{w:?}");
        let pinned = json!({"openapi": "3.2.0", "components": {"schemas": {
            "Item": {"oneOf": [{"$ref": "#/components/schemas/Book"}, {"$ref": "#/components/schemas/Gadget"}],
                    "discriminator": {"propertyName": "kind"}},
            "Book": item(json!({"enum": ["book"]})), "Gadget": item(json!({"const": "gadget"}))
        }}});
        assert_eq!(warnings(pinned), Vec::<String>::new());
    }

    /// A branch that pins its tag through a long chain of `$ref`s and `allOf` members pins it: every link is
    /// followed until one repeats, and a cycle pins nothing.
    #[test]
    fn a_tag_pinned_through_a_long_chain_is_pinned() {
        let mut schemas = serde_json::Map::new();
        schemas.insert(
            "Item".into(),
            json!({"oneOf": [{"$ref": "#/components/schemas/D0"}, {"$ref": "#/components/schemas/Book"}],
                "discriminator": {"propertyName": "kind"}}),
        );
        schemas.insert("Book".into(), json!({"type": "object", "properties": {"kind": {"const": "book"}}}));
        for i in 0..20 {
            schemas.insert(format!("D{i}"), json!({"allOf": [{"$ref": format!("#/components/schemas/D{}", i + 1)}]}));
        }
        schemas.insert("D20".into(), json!({"type": "object", "properties": {"kind": {"const": "gadget"}}}));
        let doc = json!({"openapi": "3.2.0", "components": {"schemas": schemas.clone()}});
        assert_eq!(warnings(doc), Vec::<String>::new());
        schemas.insert("D20".into(), json!({"$ref": "#/components/schemas/D0"}));
        let cycle = json!({"openapi": "3.2.0", "components": {"schemas": schemas}});
        assert_eq!(warnings(cycle).len(), 1, "a cycle pins nothing");
    }
}
