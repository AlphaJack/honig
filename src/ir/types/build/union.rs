//! Objects, `allOf` and unions as the `Builder` builds them.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::super::rewrite::FALLBACK;
use super::super::{
    Access, Constraints, EnumValue, Field, JsonKind, Kind, Struct, Ty, TypeDef, Union, UnionMode, Variant,
};
use super::Builder;
use super::keywords::{
    ANNOTATIONS, MAX_SCHEMA_DEPTH, both, dependent_names, literal_default, property_examples, regex, struct_patterns,
    type_set, union_keyword, value_kind,
};
use crate::ir::names::{Name, reserved, unique, unique_sorted};
use crate::json;
use crate::json::{escape_token, join};

impl Builder<'_> {
    pub(super) fn object(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, name: &Name) -> Struct {
        let required: BTreeSet<&str> =
            o.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let dependent = dependent_names(o);
        let mut taken = BTreeSet::new();
        let mut fields = vec![];
        if let Some(Value::Object(props)) = o.get("properties") {
            let patterns = struct_patterns(&Value::Object(o.clone())).cloned().unwrap_or_default();
            for (wire, prop) in props {
                let fptr = join(&join(ptr, "properties"), wire);
                // A declared name that a pattern matches meets that pattern's schema too.
                let matched: Vec<&Value> = patterns
                    .iter()
                    .filter(|(p, _)| regex(p).is_some_and(|re| re.find(wire).is_some()))
                    .map(|(_, x)| x)
                    .collect();
                let both = (!matched.is_empty())
                    .then(|| serde_json::json!({"allOf": std::iter::once(prop).chain(matched).collect::<Vec<_>>()}));
                let prop = both.as_ref().unwrap_or(prop);
                let ty = self.ty(prop, &fptr, &name.and(&Name::new(wire)));
                let read_only = prop.get("readOnly") == Some(&Value::Bool(true));
                let write_only = prop.get("writeOnly") == Some(&Value::Bool(true));
                let read_write = read_only || write_only;
                // A literal default makes an optional field always present. Whether `ty` can represent it
                // is known once every component is filled: `finish` drops the ones it cannot.
                // A count of the properties sees the ones sent: a default filled in would count one that was not.
                let counted = o.contains_key("minProperties") || o.contains_key("maxProperties");
                let default = literal_default(prop).filter(|_| {
                    !required.contains(wire.as_str()) && !read_write && !dependent.contains(wire.as_str()) && !counted
                });
                fields.push(Field {
                    name: Name::new(wire),
                    wire: wire.clone(),
                    ty,
                    // readOnly/writeOnly fields are absent in one direction.
                    required: (required.contains(wire.as_str()) && !read_write) || default.is_some(),
                    doc: doc_of(prop),
                    xml: self.xml_hint(prop),
                    constraints: self.constraints(prop),
                    deprecated: prop.get("deprecated") == Some(&Value::Bool(true)),
                    access: match (read_only, write_only) {
                        (true, false) => Access::ReadOnly,
                        (false, true) => Access::WriteOnly,
                        // Both or neither: the property travels both ways.
                        (true, true) | (false, false) => Access::ReadWrite,
                    },
                    spec_required: required.contains(wire.as_str()),
                    examples: property_examples(prop, &fptr),
                    default,
                    null_as_absent: self.from_v20 && !required.contains(wire.as_str()),
                });
            }
        }
        let names = unique_sorted(
            fields.iter().map(|f| (f.name.clone(), (f.wire.len(), f.wire.clone()))).collect(),
            &mut taken,
        );
        for (f, n) in fields.iter_mut().zip(names) {
            f.name = n;
        }
        // Unknown keys of an open object land in `extra`. So a proxy or a
        // load-modify-store cycle keeps them.
        let additional = match o.get("additionalProperties") {
            // A name that `patternProperties` matches is not additional, so its value need not fit
            // `additionalProperties`: the map stores any value and the run-time check judges each by its name.
            Some(Value::Object(m)) if !m.is_empty() && o.contains_key("patternProperties") => Some(Ty::Any),
            // Beside `patternProperties`, the schema applies only to names that no pattern matches: the check judges
            // them.
            Some(ap @ Value::Object(m)) if !m.is_empty() && !o.contains_key("patternProperties") => {
                Some(self.ty(ap, &join(ptr, "additionalProperties"), &name.and(&Name::new("Value"))))
            }
            Some(Value::Bool(true) | Value::Object(_)) => Some(Ty::Any),
            None if self.keep_unknown => Some(Ty::Any),
            _ => None,
        };
        // `additionalProperties: false` (JSON Schema 2020-12 section 10.3.2.3). An
        // undeclared property fails validation. `patternProperties` and
        // `unevaluatedProperties` admit more names, which their checks judge. A readOnly / writeOnly property is
        // declared even where its split type leaves it out. So such structs stay open too.
        // `patternProperties`, `unevaluatedProperties` and `propertyNames` judge the names a struct does not declare.
        // Their checks see the value as it re-serializes, so those names must survive decoding.
        // A dependent keyword that uses a name the struct does not declare looks for that name. With
        // `additionalProperties: false` that name is refused anyway: the struct stays closed.
        let undeclared_dependent = o.get("additionalProperties") != Some(&Value::Bool(false))
            && dependent_names(o)
                .into_iter()
                .any(|k| !o.get("properties").and_then(Value::as_object).is_some_and(|p| p.contains_key(k)));
        let additional = additional.or_else(|| {
            (["patternProperties", "unevaluatedProperties", "propertyNames"].iter().any(|k| o.contains_key(*k))
                || undeclared_dependent)
                .then_some(Ty::Any)
        });
        if let Some(held) = struct_patterns(&Value::Object(o.clone())).cloned() {
            return self.patterned(o, &held, fields, ptr, name);
        }
        let closed = o.get("additionalProperties") == Some(&Value::Bool(false))
            && !o.contains_key("patternProperties")
            && !o.contains_key("unevaluatedProperties")
            && fields.iter().all(|f| f.access == Access::ReadWrite);
        Struct { fields, additional, closed, patterns: vec![], additional_constraints: Box::default() }
    }

    /// Returns the struct of `o` with `fields` that stores its `patternProperties` `held` ([`struct_patterns`]): a map
    /// per pattern, and the names that none matches as `additionalProperties` says.
    pub(super) fn patterned(
        &mut self,
        o: &serde_json::Map<String, Value>,
        held: &serde_json::Map<String, Value>,
        fields: Vec<Field>,
        ptr: &str,
        name: &Name,
    ) -> Struct {
        let patterns = (held.iter().enumerate())
            .map(|(i, (p, x))| {
                let at = join(&join(ptr, "patternProperties"), p);
                let ty = self.ty(x, &at, &name.and(&Name::new(&format!("pattern {}", i + 1))));
                (p.clone(), ty, self.constraints(x))
            })
            .collect();
        let (additional, additional_constraints) = match o.get("additionalProperties") {
            Some(Value::Bool(false)) => (None, Box::default()),
            Some(extra @ Value::Object(m)) if !m.is_empty() => (
                Some(self.ty(extra, &join(ptr, "additionalProperties"), &name.and(&Name::new("Value")))),
                Box::new(self.constraints(extra)),
            ),
            _ => (Some(Ty::Any), Box::default()),
        };
        let closed = additional.is_none();
        Struct { fields, additional, closed, patterns, additional_constraints }
    }

    /// Returns the `oneOf` that `allOf: [{oneOf: [A, B]}, C]` means: `oneOf: [allOf: [A, C], allOf: [B, C]]`. Each
    /// value is one branch and also the rest. Merging the members into one struct would lose the union. The union may
    /// sit in a nested `allOf` member. Each branch is titled after its `$ref` or its one property, so the variant keeps
    /// that name. A discriminator stays: [`branch_ref`] finds a branch's target inside its `allOf`.
    pub(super) fn distributed_all_of(&self, o: &serde_json::Map<String, Value>, _ptr: &str) -> Option<Value> {
        let members = self.flat_members(o.get("allOf").and_then(Value::as_array)?, 0);
        let (i, key, union) = members.iter().enumerate().find_map(|(i, m)| {
            let (r, at) = self.deref(m)?;
            let key = ["oneOf", "anyOf"].into_iter().find(|k| r.get(*k).is_some_and(Value::is_array))?;
            // A union that is one of its own branches would spread without end: it stays an `allOf`.
            (!self.branches_reach(&r, &at, 0)).then_some((i, key, r))
        })?;
        let mut rest: Vec<Value> =
            members.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, m)| m.clone()).collect();
        // The schema's own object keywords beside `allOf` apply to every branch too.
        rest.extend(own_object_keywords(Some(o)));
        let branches: Vec<Value> = union
            .get(key)
            .and_then(Value::as_array)?
            .iter()
            .map(|b| {
                if is_null_branch(b) {
                    return b.clone();
                }
                let mut all = vec![b.clone()];
                all.extend(rest.iter().cloned());
                let mut branch = serde_json::Map::from_iter([("allOf".to_owned(), Value::Array(all))]);
                let title = b
                    .get("$ref")
                    .and_then(Value::as_str)
                    .map(|r| self.ref_name(r))
                    .or_else(|| b.get("title").and_then(Value::as_str).map(str::to_owned))
                    .or_else(|| branch_hint(b));
                if let Some(t) = title {
                    branch.insert("title".to_owned(), Value::String(t));
                }
                Value::Object(branch)
            })
            .collect();
        let mut out = serde_json::Map::from_iter([(key.to_owned(), Value::Array(branches))]);
        if let Some(d) = o.get("description").or_else(|| union.get("description")) {
            out.insert("description".to_owned(), d.clone());
        }
        if let Some(d) = union.get("discriminator") {
            out.insert("discriminator".to_owned(), d.clone());
        }
        Some(Value::Object(out))
    }

    /// Does a branch of the union `u`, or a branch of a union that a branch is, lead back to the schema at `to`?
    fn branches_reach(&self, u: &Value, to: &str, depth: usize) -> bool {
        depth < super::keywords::MAX_ALIAS_HOPS
            && ["oneOf", "anyOf"].iter().filter_map(|k| u.get(*k).and_then(Value::as_array)).flatten().any(|b| {
                self.deref(b).is_some_and(|(target, at)| {
                    !at.is_empty() && (at == to || self.branches_reach(&target, to, depth + 1))
                })
            })
    }

    /// Returns the members of an `allOf` with the members of a nested pure `allOf` lifted out. Such a member's own
    /// object keywords stay as a member of their own. A member that is a union stays whole.
    pub(super) fn flat_members(&self, members: &[Value], depth: usize) -> Vec<Value> {
        let mut out = vec![];
        for m in members {
            let resolved = self.deref(m).map(|(r, _)| r);
            let nested = resolved
                .as_ref()
                .filter(|r| depth < MAX_SCHEMA_DEPTH && r.get("oneOf").is_none() && r.get("anyOf").is_none());
            match nested.and_then(|r| r.get("allOf").and_then(Value::as_array).map(|a| (r, a))) {
                Some((r, inner)) => {
                    if let Some(own) = own_object_keywords(r.as_object()) {
                        out.push(own);
                    }
                    out.extend(self.flat_members(inner, depth + 1));
                }
                None => out.push(m.clone()),
            }
        }
        out
    }

    /// Returns the `allOf` that `{properties: {name}, required: [name], oneOf: [A, B]}` means: `allOf: [{properties,
    /// required}, {oneOf}]`. The properties beside the union apply to each branch. Typing the union alone would lose
    /// them.
    pub(super) fn sibling_union(&self, o: &serde_json::Map<String, Value>, ptr: &str) -> Option<Value> {
        let key = union_keyword(o)?;
        let own = own_object_keywords(Some(o))?;
        let mut union = serde_json::Map::from_iter([(key.to_owned(), o[key].clone())]);
        if let Some(d) = o.get("discriminator") {
            union.insert("discriminator".to_owned(), d.clone());
        }
        let mut members = vec![own, Value::Object(union)];
        members.extend(o.get("allOf").and_then(Value::as_array).into_iter().flatten().cloned());
        let mut wrapped = serde_json::Map::from_iter([("allOf".to_owned(), Value::Array(members))]);
        if let Some(d) = o.get("description") {
            wrapped.insert("description".to_owned(), d.clone());
        }
        self.distributed_all_of(&wrapped, ptr)
    }

    /// Returns `allOf`: merge every member into one struct.
    pub(super) fn all_of(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, name: &Name) -> Option<Kind> {
        // Members that only annotate (`{description: ...}`) add nothing to the type.
        let members: Vec<Value> =
            o.get("allOf").and_then(Value::as_array)?.iter().filter(|m| !is_annotation_only(m)).cloned().collect();
        if members.is_empty() {
            return Some(Kind::Alias(Ty::Any));
        }
        let own_props = json::object(o, "properties").is_some_and(|p| !p.is_empty());
        if members.len() == 1 && !own_props && !o.contains_key("oneOf") && !o.contains_key("anyOf") {
            return Some(Kind::Alias(self.ty(&members[0], &format!("{ptr}/allOf/0"), name)));
        }
        // Merge every member into one flat struct instead of reusing a `$ref`
        // member through `#[serde(flatten)]`: flatten breaks
        // `deny_unknown_fields` and buffers the input.
        let mut merged = serde_json::Map::new();
        let mut props = serde_json::Map::new();
        let mut required: BTreeSet<String> = BTreeSet::new();
        let mut additional = None;
        // A member's `patternProperties` or `unevaluatedProperties` judges undeclared names at run time.
        let mut judged = o.contains_key("patternProperties") || o.contains_key("unevaluatedProperties");
        let mut object_like = own_props;
        let mut stack: Vec<(Value, String)> =
            members.iter().enumerate().rev().map(|(i, m)| (m.clone(), format!("{ptr}/allOf/{i}"))).collect();
        let mut seen = BTreeSet::new();
        while let Some((m, mptr)) = stack.pop() {
            let Some((m, target)) = self.deref(&m) else { continue };
            let base_ptr = if target.is_empty() { mptr } else { target };
            if !seen.insert(base_ptr.clone()) {
                continue;
            }
            let Some(mo) = m.as_object() else { continue };
            if let Some(Value::Array(nested)) = mo.get("allOf") {
                for (i, n) in nested.iter().enumerate().rev() {
                    stack.push((n.clone(), format!("{base_ptr}/allOf/{i}")));
                }
            }
            if let Some(Value::Object(p)) = mo.get("properties") {
                object_like = true;
                // Refs in OpenAPI are document-absolute. So values copy safely.
                for (k, v) in p {
                    both(&mut props, k, v);
                }
            }
            if let Some(Value::Array(r)) = mo.get("required") {
                required.extend(json::strs(r).map(String::from));
            }
            if let Some(ap) = mo.get("additionalProperties") {
                additional = Some(ap.clone());
            }
            judged |= mo.contains_key("patternProperties") || mo.contains_key("unevaluatedProperties");
        }
        if let Some(Value::Object(p)) = o.get("properties") {
            for (k, v) in p {
                both(&mut props, k, v);
            }
        }
        if let Some(Value::Array(r)) = o.get("required") {
            required.extend(json::strs(r).map(String::from));
        }
        // The schema's own `additionalProperties` wins over its members'.
        if let Some(ap) = o.get("additionalProperties") {
            additional = Some(ap.clone());
        }
        if !object_like {
            // allOf of non-objects such as [$ref: SomeEnum, {description}]. The first member with the `type` that
            // every member allows. 3.2 "Parsing and Serializing": `allOf: [{type: [string, number]}, {type:
            // string}]` is a string, never a number.
            let first = &members[0];
            let narrowed = self.narrowed_types(&members).and_then(|types| {
                let (f, _) = self.deref(first)?;
                let mut own = f.as_object()?.clone();
                let wide = own.get("type").and_then(Value::as_array).is_some_and(|ts| ts.len() > types.len());
                own.insert("type".into(), types_value(&types));
                wide.then_some(Value::Object(own))
            });
            return Some(Kind::Alias(self.ty(narrowed.as_ref().unwrap_or(first), &format!("{ptr}/allOf/0"), name)));
        }
        merged.insert("type".into(), "object".into());
        merged.insert("properties".into(), Value::Object(props));
        merged.insert("required".into(), Value::Array(required.into_iter().map(Value::String).collect()));
        if let Some(ap) = additional {
            merged.insert("additionalProperties".into(), ap);
        }
        if judged {
            // Kept for `object`: the struct stores undeclared names for the run-time check to judge.
            merged.insert("unevaluatedProperties".into(), Value::Bool(true));
        }
        Some(Kind::Struct(self.object(&merged, ptr, name)))
    }

    /// Returns the `type` names that every `allOf` member allows (members without `type` allow all). This applies when
    /// a member declares one and some type is left.
    pub(super) fn narrowed_types(&self, members: &[Value]) -> Option<Vec<String>> {
        let mut out: Option<Vec<String>> = None;
        for m in members {
            let Some((m, _)) = self.deref(m) else { continue };
            let Some(o) = m.as_object().filter(|o| o.contains_key("type")) else { continue };
            let (mut ts, null) = type_set(o);
            if null && !ts.iter().any(|t| t == "null") {
                ts.push("null".into());
            }
            out = Some(match out {
                None => ts,
                Some(prev) => prev.into_iter().filter(|t| ts.contains(t)).collect(),
            });
        }
        out.filter(|ts| !ts.is_empty())
    }

    /// Converts `oneOf` / `anyOf` to a union, an `Option` or the single branch.
    pub(super) fn union(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, name: &Name) -> Option<Kind> {
        let key = if o.contains_key("oneOf") { "oneOf" } else { "anyOf" };
        let branches = o.get(key).and_then(Value::as_array)?.clone();
        let mut variants: Vec<(Value, String)> = vec![];
        for (i, b) in branches.iter().enumerate() {
            let is_null = is_null_branch(b);
            if !is_null {
                variants.push((b.clone(), format!("{ptr}/{key}/{i}")));
            }
        }
        if variants.is_empty() {
            return Some(Kind::Alias(Ty::Unit));
        }
        if variants.len() == 1 {
            let (b, bptr) = &variants[0];
            return Some(Kind::Alias(self.ty(b, bptr, name)));
        }
        // An untitled inline branch is numbered by the sorted text of its schema, so a reorder of the branches renames
        // none of them.
        let texts: Vec<String> = variants.iter().map(|(b, _)| b.to_string()).collect();
        let rank = |i: usize| texts.iter().filter(|t| texts.get(i).is_some_and(|own| *t < own)).count() + 1;
        let mut named = vec![];
        for (i, (b, bptr)) in variants.iter().enumerate() {
            // `null` is the `Other` fallback of the union and not the payload of a variant.
            // An inline branch with a `title` gets its type name from it. Others are numbered.
            let branch =
                b.get("title").and_then(Value::as_str).map_or_else(|| format!("Variant{}", rank(i)), str::to_owned);
            let ty = self.ty(b, bptr, &name.and(&Name::new(&branch))).non_null().clone();
            let vname = match b.get("$ref").and_then(Value::as_str) {
                Some(r) => Some(Name::new(&self.ref_name(r))),
                None => b.get("title").and_then(Value::as_str).map_or_else(
                    || {
                        if let Ty::Named(id) = &ty {
                            // An inline branch's type is named after the union: the variant keeps the rest.
                            let own = &self.defs[id.0].name;
                            (own != name).then(|| own.after(name).unwrap_or_else(|| own.clone()))
                        } else if let Ty::Format(f) = &ty {
                            Some(Name::new(f.name()))
                        } else {
                            Some(Name::new(kind_name(&ty)))
                        }
                    },
                    |t| Some(Name::new(t)),
                ),
            };
            let vname = vname.filter(|n| !n.is("other")).unwrap_or_else(|| Name::new(&format!("Variant{}", rank(i))));
            let constraints = if matches!(ty, Ty::Named(_)) { Constraints::default() } else { self.constraints(b) };
            named.push((vname, ty, constraints, texts.get(i).cloned().unwrap_or_default()));
        }
        // Several array branches are told apart by what they contain (`NumberArray`, `DocumentArray`) and not as
        // `Array`, `Array2`. One alone stays `Array`.
        if named.iter().filter(|(n, ..)| n.is("array")).count() > 1 {
            for (n, ty, ..) in &mut named {
                if n.is("array") {
                    *n = Name::new(&self.array_name(ty));
                }
            }
        }
        let names =
            unique_sorted(named.iter().map(|(n, .., text)| (n.clone(), text.clone())).collect(), &mut BTreeSet::new());
        let mut out: Vec<Variant> = named
            .into_iter()
            .zip(names)
            .map(|((_, ty, constraints, _), name)| Variant { name, ty, tags: vec![], constraints })
            .collect();
        // A branch that any value passes (`{$comment: ...}` or the RFC 2557 example of 3.2) is what `Other` already
        // stores. Next to typed branches it would be a second catch-all that cannot be told apart.
        let open = out.iter().any(|v| v.ty != Ty::Any) && out.iter().any(|v| v.ty == Ty::Any);
        if open {
            (variants, out) = variants.into_iter().zip(out).filter(|(_, v)| v.ty != Ty::Any).unzip();
        }
        strip_common_prefix(&mut out);
        let mode = self.union_mode(o, &variants, &mut out);
        let fallback = match mode {
            UnionMode::Tagged { default, .. } => default.filter(|i| out[*i].tags.is_empty()),
            UnionMode::ByKind | UnionMode::Untagged => None,
        };
        let hints: Vec<Option<String>> = variants.iter().map(|(b, _)| branch_hint(b)).collect();
        self.name_by_tag(&mut out, name, fallback, &hints);
        Some(Kind::Union(Union { variants: out, mode, open, rest: false, exclusive: key == "oneOf" }))
    }

    /// Returns what an array branch contains as the name of its variant: `NumberArray` for numbers, `NumberArrayArray`
    /// for arrays of them and `DocumentArray` for a named type.
    pub(super) fn array_name(&self, ty: &Ty) -> String {
        if let Ty::Vec(t) | Ty::Array(t, _) = ty {
            format!("{}_array", self.array_name(t))
        } else if let Ty::Option(t) | Ty::Indirect(t) = ty {
            self.array_name(t)
        } else if let Ty::Named(id) = ty {
            self.defs[id.0].name.words().join("_")
        } else if let Ty::Format(f) = ty {
            f.name().to_owned()
        } else {
            kind_name(ty).to_owned()
        }
    }

    /// Names the variants still named `VariantN` after their tags (`Email`,
    /// `EmailOrSms` or `Otherwise` for the fallback of a conditional union). Their
    /// inline type does too (`NotifyEmail`).
    pub(super) fn name_by_tag(
        &mut self,
        out: &mut [Variant],
        parent: &Name,
        fallback: Option<usize>,
        hints: &[Option<String>],
    ) {
        let mut taken: BTreeSet<String> = out.iter().map(|v| v.name.key()).collect();
        for (i, v) in out.iter_mut().enumerate() {
            let generic = v.name.is_numbered("variant");
            let new = if Some(i) == fallback {
                Name::new("Otherwise")
            } else if v.tags.is_empty() {
                match hints.get(i).cloned().flatten() {
                    Some(h) => Name::new(&h),
                    None => continue,
                }
            } else {
                Name::new(&v.tags.join(" or "))
            };
            if !generic || new.is("other") || taken.contains(&new.key()) {
                continue;
            }
            // The variant's type and the types nested in it share its old prefix.
            let (from, to) = (parent.and(&v.name), parent.and(&new));
            for d in 0..self.defs.len() {
                let own = &self.defs[d].name;
                let renamed = if *own == from {
                    Some(to.clone())
                } else {
                    own.after(&from)
                        .filter(|rest| !rest.words()[0].starts_with(|c: char| c.is_ascii_digit()))
                        .map(|rest| to.and(&rest))
                };
                if let Some(renamed) = renamed {
                    self.defs[d].name = unique(renamed, &mut self.names);
                }
            }
            taken.remove(&v.name.key());
            taken.insert(new.key());
            v.name = new;
        }
    }

    pub(super) fn union_mode(
        &self,
        o: &serde_json::Map<String, Value>,
        branches: &[(Value, String)],
        out: &mut [Variant],
    ) -> UnionMode {
        let resolved: Vec<Option<serde_json::Map<String, Value>>> = branches
            .iter()
            .map(|(b, _)| self.deref(b).and_then(|(v, _)| v.as_object().cloned()).map(|m| self.flatten_all_of(m)))
            .collect();
        let all_objects = resolved.iter().all(Option::is_some)
            && out.iter().all(|v| matches!(self.space_kind(&v.ty), JsonKind::Object | JsonKind::Any));
        // 1. Explicit discriminator.
        if let Some(d) = o.get("discriminator").and_then(Value::as_object)
            && let Some(prop) = d.get("propertyName").and_then(Value::as_str)
            && all_objects
        {
            let mapping: Vec<(String, String)> = d
                .get("mapping")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned()))).collect())
                .unwrap_or_default();
            for (i, (b, _)) in branches.iter().enumerate() {
                let r = branch_ref(b);
                let target = r.and_then(|r| self.doc.local_pointer(r));
                let mut tags: Vec<String> = mapping
                    .iter()
                    .filter(|(_, m)| {
                        let mt = self
                            .doc
                            .local_pointer(m)
                            .or_else(|| Some(format!("/components/schemas/{}", escape_token(m))));
                        mt.is_some() && mt == target
                    })
                    .map(|(k, _)| k.clone())
                    .collect();
                if tags.is_empty() {
                    if let Some(c) = resolved[i].as_ref().and_then(|m| const_of(m, prop)) {
                        tags.push(c);
                    } else if let Some(r) = r {
                        // Implicit mapping: the component name.
                        tags.push(self.ref_name(r));
                    }
                }
                out[i].tags = tags;
            }
            let default = d.get("defaultMapping").and_then(Value::as_str).and_then(|dm| {
                let t = self.doc.local_pointer(dm);
                branches.iter().position(|(b, _)| branch_ref(b).and_then(|r| self.doc.local_pointer(r)) == t)
            });
            return UnionMode::Tagged { property: prop.to_owned(), default };
        }
        // 2. Inferred tag: one property with a distinct constant in every branch.
        if all_objects && let Some(mode) = inferred_tag(branches, &resolved, out) {
            return mode;
        }
        // 3. Distinct JSON kinds.
        let kinds: Vec<JsonKind> = out.iter().map(|v| self.space_kind(&v.ty)).collect();
        let distinct: BTreeSet<&JsonKind> = kinds.iter().collect();
        if !kinds.contains(&JsonKind::Any) && distinct.len() == kinds.len() {
            return UnionMode::ByKind;
        }
        UnionMode::Untagged
    }

    pub(super) fn flatten_all_of(&self, mut m: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
        let Some(Value::Array(members)) = m.get("allOf").cloned() else { return m };
        let mut props = json::object(&m, "properties").cloned().unwrap_or_default();
        for mem in members {
            if let Some((mv, _)) = self.deref(&mem)
                && let Some(p) = json::object(&mv, "properties")
            {
                for (k, v) in p {
                    props.entry(k.clone()).or_insert(v.clone());
                }
            }
        }
        m.insert("properties".into(), Value::Object(props));
        m
    }

    pub(super) fn space_kind(&self, ty: &Ty) -> JsonKind {
        Kinds::remembering(&self.defs, &self.kinds).of(ty)
    }

    /// Returns `type: [string, integer]` to union by JSON kind.
    pub(super) fn multi_type(
        &mut self,
        o: &serde_json::Map<String, Value>,
        types: &[&str],
        ptr: &str,
        name: &Name,
    ) -> Kind {
        let mut variants = vec![];
        for t in types {
            let mut single = o.clone();
            single.insert("type".into(), Value::String(t.to_string()));
            let ty = self.ty(&Value::Object(single), &format!("{ptr}/type/{t}"), &name.and(&Name::new(t)));
            // The keywords sit on the schema with the `type` list. Whatever contains that schema checks them.
            variants.push(Variant { name: Name::new(t), ty, tags: vec![], constraints: Constraints::default() });
        }
        // integer + number overlap: integer must be tried first (ByKind handles it).
        let kinds: BTreeSet<JsonKind> = variants.iter().map(|v| self.space_kind(&v.ty)).collect();
        let mode = if kinds.len() == variants.len() { UnionMode::ByKind } else { UnionMode::Untagged };
        Kind::Union(Union { variants, mode, open: false, rest: false, exclusive: false })
    }
}

/// Picks the mode of a union while it is built. A variant whose type is still being built then has no kind yet, so the
/// union tries its variants in order. Once every type is built, such a union whose variants have distinct kinds picks
/// by kind, as one built later would. Repeated until nothing changes: a union's kind can depend on another union.
pub(super) fn by_kind_when_known(defs: &mut [TypeDef]) {
    loop {
        let mut known = Kinds::new(defs);
        let settle: Vec<usize> = (0..defs.len())
            .filter(|&i| {
                let Kind::Union(u) = &defs[i].kind else { return false };
                if u.mode != UnionMode::Untagged {
                    return false;
                }
                let kinds: Vec<JsonKind> = u.variants.iter().map(|v| known.of(&v.ty)).collect();
                let distinct: BTreeSet<&JsonKind> = kinds.iter().collect();
                !kinds.contains(&JsonKind::Any) && distinct.len() == kinds.len()
            })
            .collect();
        if settle.is_empty() {
            return;
        }
        for i in settle {
            if let Kind::Union(u) = &mut defs[i].kind {
                u.mode = UnionMode::ByKind;
            }
        }
    }
}

/// Most string values an inline branch's variant is named after (`OptimizingOrDone`). More look like a list.
pub(super) const MAX_HINT_VALUES: usize = 3;

/// Returns the `$ref` of a union branch, also when distributing an `allOf` wrapped it (`{allOf: [{$ref: X}, rest]}`).
pub(super) fn branch_ref(b: &Value) -> Option<&str> {
    b.get("$ref").and_then(Value::as_str).or_else(|| {
        let first = b.get("allOf")?.as_array()?.first()?;
        first.get("$ref").and_then(Value::as_str)
    })
}

/// Returns the object keywords of a schema that apply beside its `allOf` or union: `type`, `properties`, `required` and
/// `additionalProperties`. `None` when it has no properties or requirements of its own.
pub(super) fn own_object_keywords(o: Option<&serde_json::Map<String, Value>>) -> Option<Value> {
    let o = o?;
    let has = |k: &str| match o.get(k) {
        Some(Value::Object(m)) => !m.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        _ => false,
    };
    if !has("properties") && !has("required") {
        return None;
    }
    let own: serde_json::Map<String, Value> = o
        .iter()
        .filter(|(k, _)| matches!(k.as_str(), "type" | "properties" | "required" | "additionalProperties"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Some(Value::Object(own))
}

/// Returns a name an inline branch suggests for its variant: its string values as tags are named (`enum: [ok]` is `Ok`,
/// `enum: [optimizing, done]` is `OptimizingOrDone`, up to three) or its one property (`{error: ...}` is `Error`).
pub(super) fn branch_hint(b: &Value) -> Option<String> {
    if let Some(Value::Array(values)) = b.get("enum") {
        let strings: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
        return (strings.len() == values.len() && (1..=MAX_HINT_VALUES).contains(&strings.len()))
            .then(|| strings.join(" or "));
    }
    if let Some(Value::String(v)) = b.get("const") {
        return Some(v.clone());
    }
    match b.get("properties") {
        Some(Value::Object(props)) if props.len() == 1 => props.keys().next().cloned(),
        _ => None,
    }
}

/// Returns the branches of a `oneOf` or `anyOf` other than `null`, as `union` types them. One left makes the union an
/// alias of that branch.
pub(super) fn only_non_null_branch(s: &Value) -> Vec<&Value> {
    let key = if s.get("oneOf").is_some() { "oneOf" } else { "anyOf" };
    s.get(key).and_then(Value::as_array).into_iter().flatten().filter(|b| !is_null_branch(b)).collect()
}

/// Returns a `oneOf` or `anyOf` branch that only `null` passes. It is the `Other` of the union and no variant of its
/// own.
pub(super) fn is_null_branch(b: &Value) -> bool {
    b.get("type").and_then(Value::as_str) == Some("null") || b.get("const") == Some(&Value::Null)
}

pub(crate) fn kind_name(ty: &Ty) -> &'static str {
    match ty {
        Ty::Bool => "Bool",
        Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => "Integer",
        Ty::F32 | Ty::F64 | Ty::Decimal => "Number",
        Ty::String => "String",
        Ty::Vec(_) | Ty::Array(..) | Ty::Tuple(_) => "Array",
        Ty::Map(_) => "Object",
        Ty::Unit => "Null",
        Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Any | Ty::Named(_) | Ty::Option(_) | Ty::Indirect(_) => "Value",
    }
}

pub(crate) fn strip_common_prefix(vs: &mut [Variant]) {
    if vs.len() < 2 {
        return;
    }
    let names: Vec<&Name> = vs.iter().map(|v| &v.name).collect();
    let Some(cut) = Name::without_common_prefix(&names) else { return };
    let unique_names: BTreeSet<String> = cut.iter().map(Name::key).collect();
    if unique_names.len() == cut.len() && cut.iter().all(|c| !c.is("other")) {
        for (v, c) in vs.iter_mut().zip(cut) {
            v.name = c;
        }
    }
}

/// Returns a union tagged by one property with distinct constants in every branch
/// (a conditional union's fallback branch accepts every other value).
pub(crate) fn inferred_tag(
    branches: &[(Value, String)],
    resolved: &[Option<serde_json::Map<String, Value>>],
    out: &mut [Variant],
) -> Option<UnionMode> {
    let candidates: Vec<String> = resolved
        .first()?
        .as_ref()?
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())?;
    let fallback = branches.iter().position(|(b, _)| b.get(FALLBACK) == Some(&Value::Bool(true)));
    for prop in candidates {
        let tags: Option<Vec<Vec<String>>> = resolved
            .iter()
            .enumerate()
            .map(|(i, r)| if Some(i) == fallback { Some(vec![]) } else { r.as_ref().and_then(|m| consts_of(m, &prop)) })
            .collect();
        let Some(tags) = tags else { continue };
        let all: Vec<&String> = tags.iter().flatten().collect();
        let set: BTreeSet<&String> = all.iter().copied().collect();
        if set.len() == all.len() && !all.is_empty() {
            for (v, tag) in out.iter_mut().zip(tags) {
                v.tags = tag;
            }
            return Some(UnionMode::Tagged { property: prop, default: fallback });
        }
    }
    None
}

/// Returns the string values that a branch pins `prop` to. That is its `const` or every value of its `enum`.
pub(crate) fn consts_of(m: &serde_json::Map<String, Value>, prop: &str) -> Option<Vec<String>> {
    if let Some(c) = const_of(m, prop) {
        return Some(vec![c]);
    }
    let values = m.get("properties")?.get(prop)?.get("enum")?.as_array()?;
    let strings: Option<Vec<String>> = values.iter().map(json::owned_str).collect();
    strings.filter(|s| !s.is_empty())
}

pub(crate) fn const_of(m: &serde_json::Map<String, Value>, prop: &str) -> Option<String> {
    let p = m.get("properties")?.get(prop)?;
    if let Some(c) = p.get("const").and_then(Value::as_str) {
        return Some(c.to_owned());
    }
    match p.get("enum").and_then(Value::as_array) {
        Some(values) if values.len() == 1 => values[0].as_str().map(String::from),
        _ => None,
    }
}

pub(crate) fn const_ty(c: &Value) -> Ty {
    match c {
        Value::Bool(_) => Ty::Bool,
        Value::Number(n) if n.is_i64() || n.is_u64() => Ty::I64,
        Value::Number(_) => Ty::F64,
        Value::String(_) => Ty::String,
        Value::Null => Ty::Unit,
        Value::Array(_) | Value::Object(_) => Ty::Any,
    }
}

pub(crate) fn str_enum(values: &[&str]) -> Kind {
    let mut taken = reserved(&["Other"]);
    let mut seen_wire = BTreeSet::new();
    let wires: Vec<&str> = values.iter().copied().filter(|w| seen_wire.insert(*w)).collect();
    let names = unique_sorted(wires.iter().map(|w| (Name::new(w), (w.len(), *w))).collect(), &mut taken);
    let vs = names.into_iter().zip(wires).map(|(name, w)| EnumValue { name, wire: w.to_owned(), doc: None }).collect();
    Kind::StrEnum(vs)
}

/// Returns a schema that the description declares: a component schema, or the schema of a component parameter or
/// header.
pub(super) fn named_by_description(ptr: &str) -> bool {
    let entry = |section: &str, tail: &str| {
        ptr.strip_prefix(section).and_then(|rest| rest.strip_suffix(tail)).is_some_and(|key| !key.contains('/'))
    };
    entry("/components/schemas/", "")
        || entry("/components/parameters/", "/schema")
        || entry("/components/headers/", "/schema")
}

pub(crate) fn is_annotation_only(v: &Value) -> bool {
    v.as_object().is_some_and(|o| o.keys().all(|k| ANNOTATIONS.contains(&k.as_str()) || k.starts_with("x-")))
}

pub(crate) fn has_ref_only(o: &serde_json::Map<String, Value>) -> bool {
    o.contains_key("$ref") && o.keys().all(|k| k == "$ref" || ANNOTATIONS.contains(&k.as_str()) || k.starts_with("x-"))
}

/// The value of a `type` keyword for these names. That is one string or a list.
pub(super) fn types_value(types: &[String]) -> Value {
    match types {
        [one] => Value::String(one.clone()),
        many => Value::Array(many.iter().cloned().map(Value::String).collect()),
    }
}

/// Returns an object that admits no property at all: `additionalProperties: false` with nothing declared and no
/// `patternProperties` or `unevaluatedProperties` to admit a name. Only `{}` is valid. It is an empty closed struct
/// and not a map of anything.
pub(super) fn only_empty(o: &serde_json::Map<String, Value>) -> bool {
    o.get("additionalProperties") == Some(&Value::Bool(false))
        && json::object(o, "properties").is_none_or(serde_json::Map::is_empty)
        && !o.contains_key("patternProperties")
        && !o.contains_key("unevaluatedProperties")
}

/// Returns the title and description of a schema joined.
pub(crate) fn doc_of(s: &Value) -> Option<String> {
    with_external_docs(text_of(s), s)
}

/// Returns `title` and `description`.
pub(crate) fn text_of(s: &Value) -> Option<String> {
    let title = s.get("title").and_then(Value::as_str);
    let desc = s.get("description").and_then(Value::as_str);
    match (title, desc) {
        (Some(t), Some(d)) => Some(format!("{t}\n\n{d}")),
        (Some(t), None) => Some(t.to_owned()),
        (None, Some(d)) => Some(d.to_owned()),
        (None, None) => None,
    }
}

/// Returns `doc` followed by the `externalDocs` link of the object if there is one.
pub(crate) fn with_external_docs(doc: Option<String>, s: &Value) -> Option<String> {
    let Some(url) = s.pointer("/externalDocs/url").and_then(Value::as_str).map(str::trim).filter(|u| !u.is_empty())
    else {
        return doc;
    };
    let line =
        match s.pointer("/externalDocs/description").and_then(Value::as_str).map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) => format!("See also: {d} ({url})"),
            None => format!("See also: {url}"),
        };
    Some(match doc {
        Some(d) => format!("{d}\n\n{line}"),
        None => line,
    })
}

pub(crate) fn kind_in(defs: &[TypeDef], ty: &Ty) -> JsonKind {
    Kinds::new(defs).of(ty)
}

/// The JSON kinds of the named types of `defs`, each found once. A union of unions asks for the kind of each type
/// again on every path to it: found afresh each time, that grows with the paths and not with the types. `known` keeps
/// the kinds that are final across calls: a builder asks once per union, and a chain of unions would walk every union
/// before it each time.
pub(crate) struct Kinds<'d> {
    defs: &'d [TypeDef],
    /// The types met in this call: only those, so a call costs what it reaches and not the whole description.
    found: BTreeMap<usize, Found>,
    known: Option<&'d RefCell<Vec<Option<JsonKind>>>>,
    /// Whether the kind being found met a type still being built or a cycle: then it is not final.
    unsettled: bool,
}

/// Where finding the kind of a named type is.
#[derive(Clone, Copy)]
pub(super) enum Found {
    /// Met again while it is found, the type is on a cycle and accepts any value.
    Finding,
    Kind(JsonKind),
}

impl<'d> Kinds<'d> {
    pub(crate) fn new(defs: &'d [TypeDef]) -> Self {
        Self { defs, found: BTreeMap::new(), known: None, unsettled: false }
    }

    /// [`Kinds::new`] that loads and keeps the final kinds in `known`.
    fn remembering(defs: &'d [TypeDef], known: &'d RefCell<Vec<Option<JsonKind>>>) -> Self {
        Self { known: Some(known), ..Self::new(defs) }
    }

    pub(crate) fn of(&mut self, ty: &Ty) -> JsonKind {
        match ty {
            Ty::Bool => JsonKind::Bool,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => JsonKind::Integer,
            Ty::F32 | Ty::F64 | Ty::Decimal => JsonKind::Number,
            Ty::String | Ty::Bytes | Ty::Binary | Ty::Format(_) => JsonKind::String,
            Ty::Any => JsonKind::Any,
            Ty::Unit => JsonKind::Null,
            Ty::Vec(_) | Ty::Array(..) | Ty::Tuple(_) => JsonKind::Array,
            Ty::Map(_) => JsonKind::Object,
            Ty::Option(t) | Ty::Indirect(t) => self.of(t),
            Ty::Named(id) => self.named(id.0),
        }
    }

    fn named(&mut self, i: usize) -> JsonKind {
        if let Some(kind) = self.known.and_then(|k| k.borrow().get(i).copied().flatten()) {
            return kind;
        }
        match self.found.get(&i) {
            Some(Found::Kind(kind)) => return *kind,
            Some(Found::Finding) => {
                self.unsettled = true;
                return JsonKind::Any;
            }
            None => {}
        }
        self.found.insert(i, Found::Finding);
        let outer = std::mem::replace(&mut self.unsettled, false);
        let defs = self.defs;
        let kind = match &defs[i].kind {
            Kind::Struct(_) => JsonKind::Object,
            Kind::StrEnum(_) => JsonKind::String,
            Kind::Checked(c) => self.of(&c.base),
            Kind::IntEnum(_) => JsonKind::Integer,
            Kind::Const(v) => value_kind(v),
            // A closed union whose variants share a kind has that kind. An open one accepts any value.
            Kind::Union(u) if !u.open => {
                let mut kinds = u.variants.iter().map(|v| self.of(&v.ty));
                let first = kinds.next().unwrap_or(JsonKind::Any);
                if kinds.all(|k| k == first) { first } else { JsonKind::Any }
            }
            Kind::Union(_) => JsonKind::Any,
            Kind::Pending => {
                self.unsettled = true;
                JsonKind::Any
            }
            Kind::Alias(t) | Kind::Newtype(t) => self.of(t),
        };
        self.found.insert(i, Found::Kind(kind));
        if !self.unsettled
            && let Some(known) = self.known
        {
            let mut known = known.borrow_mut();
            if known.len() <= i {
                known.resize(i + 1, None);
            }
            known[i] = Some(kind);
        }
        self.unsettled |= outer;
        kind
    }
}
