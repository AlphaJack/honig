//! A discriminated `oneOf` whose branches can all match one value. 3.2 says a discriminator "MUST NOT change the
//! validation outcome": `oneOf` still means exactly one. Petstore-style `Cat` and `Dog` that share every property
//! both match `{"petType": "cat", "name": "Tom"}`, and the value is invalid. A branch that pins the discriminating
//! property with `const` (or a one-value `enum`) cannot match another branch's tag. A warning lists the branches
//! that do not.

use serde_json::Value;

use crate::diag::Diagnostics;
use crate::json::{Doc, join};
use crate::walk::{self, Obj, Visit};

/// Warns about each discriminated `oneOf` with a branch that does not pin its tag.
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

impl Found<'_> {
    /// Returns whether the branch through `$ref`s and `allOf` members fixes `prop` to one value. `seen` holds the
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

impl Visit for Found<'_> {
    fn schema(&mut self, s: &mut Obj, ptr: &str) {
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
            .map(|(i, b)| b.get("$ref").and_then(Value::as_str).map_or_else(|| format!("#{i}"), String::from))
            .collect();
        if !loose.is_empty() {
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
            "Cat": pet(json!({"type": "string"})), "Dog": pet(json!({"const": "dog"}))
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
