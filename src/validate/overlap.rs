//! Union branches that one value can match. An `anyOf`, or a `oneOf` without a discriminator, whose object branches
//! nothing tells apart decodes such a value as the first and drops what the others carry: a warning says so.
//!
//! A discriminated `oneOf` whose branches can all match one value. 3.2 says a discriminator "MUST NOT change the
//! validation outcome": `oneOf` still means exactly one. Branches `Cat` and `Dog` that share every property both match
//! `{"petType": "cat", "name": "Tom"}`, and the value is invalid. A branch that sets the discriminating property with
//! `const` (or a one-value `enum`) cannot match another branch's tag. A warning lists the branches that do not.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json::{Doc, join};
use crate::walk::{self, Obj, Visit};

/// Warns about each discriminated `oneOf` with a branch that does not pin its tag, and about each union with two
/// object branches that nothing tells apart.
pub(crate) fn check(doc: &Doc, diags: &mut Diagnostics) {
    let mut found = Found { doc, out: vec![] };
    walk::walk(&mut doc.root.clone(), &mut found);
    for (ptr, message) in found.out {
        diags.warning(&ptr, message);
    }
}

struct Found<'a> {
    doc: &'a Doc,
    out: Vec<(String, String)>,
}

/// What tells an object branch apart: its properties, the required ones, the `const` and `type` of each and whether
/// it is closed.
#[derive(Default)]
struct Object {
    properties: BTreeSet<String>,
    required: BTreeSet<String>,
    consts: BTreeMap<String, Value>,
    types: BTreeMap<String, Value>,
    closed: bool,
}

impl Object {
    /// Can no value match both `self` and `other`? A required property of one that the other does not declare or
    /// types otherwise, a `const` property with another value, or a closed branch tells them apart.
    fn apart(&self, other: &Self) -> bool {
        let missing = |a: &Self, b: &Self| {
            a.required.iter().any(|r| {
                !b.properties.contains(r) || a.types.get(r).is_some_and(|t| b.types.get(r).is_some_and(|u| u != t))
            })
        };
        let consts = self.consts.iter().any(|(k, v)| other.consts.get(k).is_some_and(|w| w != v));
        self.closed || other.closed || missing(self, other) || missing(other, self) || consts
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
        let mut o =
            Object { closed: branch.get("additionalProperties") == Some(&Value::Bool(false)), ..Object::default() };
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
            o.closed |= m.closed;
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
                             property or a `const` tag that tells them apart, or close one with \
                             `additionalProperties: false`"
                        ),
                    ));
                }
            }
        }
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

/// Returns how a message names branch `i`: its `$ref` or its position.
fn name(branch: &Value, i: usize) -> String {
    branch.get("$ref").and_then(Value::as_str).map_or_else(|| format!("#{i}"), String::from)
}

impl Visit for Found<'_> {
    fn schema(&mut self, s: &mut Obj, ptr: &str) {
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

    #[test]
    fn branches_that_do_not_pin_the_tag_are_named() {
        let pet = |tag: Value| json!({"type": "object", "properties": {"petType": tag, "name": {"type": "string"}}});
        let loose = json!({"openapi": "3.2.0", "components": {"schemas": {
            "Pet": {"oneOf": [{"$ref": "#/components/schemas/Cat"}, {"$ref": "#/components/schemas/Dog"}],
                    "discriminator": {"propertyName": "petType"}},
            "Cat": pet(json!({"type": "string"})),
            "Dog": {"allOf": [pet(json!({"const": "dog"})), {"required": ["bark"], "properties": {"bark": {"type": "boolean"}}}]}
        }}});
        let w = warnings(loose);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].contains("#/components/schemas/Cat") && !w[0].contains("Dog"), "{w:?}");
        let pinned = json!({"openapi": "3.2.0", "components": {"schemas": {
            "Pet": {"oneOf": [{"$ref": "#/components/schemas/Cat"}, {"$ref": "#/components/schemas/Dog"}],
                    "discriminator": {"propertyName": "petType"}},
            "Cat": pet(json!({"enum": ["cat"]})), "Dog": pet(json!({"const": "dog"}))
        }}});
        assert_eq!(warnings(pinned), Vec::<String>::new());
    }

    /// A branch that pins its tag through a long chain of `$ref`s and `allOf` members pins it: every link is
    /// followed until one repeats, and a cycle pins nothing.
    #[test]
    fn a_tag_pinned_through_a_long_chain_is_pinned() {
        let mut schemas = serde_json::Map::new();
        schemas.insert(
            "Pet".into(),
            json!({"oneOf": [{"$ref": "#/components/schemas/D0"}, {"$ref": "#/components/schemas/Cat"}],
                "discriminator": {"propertyName": "petType"}}),
        );
        schemas.insert("Cat".into(), json!({"type": "object", "properties": {"petType": {"const": "cat"}}}));
        for i in 0..20 {
            schemas.insert(format!("D{i}"), json!({"allOf": [{"$ref": format!("#/components/schemas/D{}", i + 1)}]}));
        }
        schemas.insert("D20".into(), json!({"type": "object", "properties": {"petType": {"const": "dog"}}}));
        let doc = json!({"openapi": "3.2.0", "components": {"schemas": schemas.clone()}});
        assert_eq!(warnings(doc), Vec::<String>::new());
        schemas.insert("D20".into(), json!({"$ref": "#/components/schemas/D0"}));
        let cycle = json!({"openapi": "3.2.0", "components": {"schemas": schemas}});
        assert_eq!(warnings(cycle).len(), 1, "a cycle pins nothing");
    }
}
