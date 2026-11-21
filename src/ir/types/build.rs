//! Reading schemas into the type graph: the `Builder` and the schema keywords it understands.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::cycles::break_cycles;
use super::rewrite::conditional_union;
use super::{
    Checked, Constraints, Evaluated, EvaluatedItems, Format, JsonKind, Kind, Leftover, Rest, Ty, TypeDef, TypeId,
    TypeSpace, UnionMode, XmlHint,
};
use crate::diag::Diagnostics;
use crate::ir::names::{Name, Reserved, unique};
use crate::json;
use crate::json::join;
use crate::json::{Doc, pointer};

mod keywords;
mod union;

use keywords::{
    ANNOTATIONS, Evaluation, MAX_ALIAS_HOPS, MAX_SCHEMA_DEPTH, MAX_TUPLE, MAX_TYPE_DEPTH, UNTYPED_KEYWORDS,
    asserts_nothing, beside_union, checked_items, checked_scalar, count_of, fixed_len, inferred_kind, int_enum,
    int_enum_values, int_format, json_string_schema, kind_scoped, may_be, mixed, references, scoped_keywords,
    string_format, struct_patterns, type_set, typed_conditional, typed_const, typed_dependent_schemas,
    typed_object_keywords, typed_patterns, typeless_assertions, untyped, untyped_value_checks, values_union,
};
pub(crate) use keywords::{binary_media, literal_default};
pub(crate) use union::{Kinds, doc_of, is_annotation_only, kind_in, text_of, with_external_docs};
use union::{
    by_kind_when_known, const_ty, has_ref_only, named_by_description, only_empty, only_non_null_branch, str_enum,
};

/// Builds a `TypeSpace` from schemas.
#[derive(Debug)]
pub(crate) struct Builder<'a> {
    /// The document being converted.
    pub doc: &'a Doc,
    /// Where problems are reported.
    pub diags: &'a mut Diagnostics,
    /// The dynamic anchors of the document: a `$dynamicRef` whose anchor several resources declare is not typed.
    dynamic: crate::schema_ids::Dynamic,
    /// Named types built so far.
    pub defs: Vec<TypeDef>,
    names: BTreeSet<String>,
    /// Names that operations start from. They are the names that the target keeps for itself as `unique` records them.
    pub(crate) reserved_operations: BTreeSet<String>,
    /// Pointer of a schema to its named type.
    by_ptr: BTreeMap<String, TypeId>,
    /// Pointer of an unnamed schema a component points at (`#/x-ext/<hash>`) to that component's name.
    claimed: BTreeMap<String, String>,
    /// Pointer of a non-component schema to its converted type (memo).
    ptr_cache: BTreeMap<String, Ty>,
    in_progress: BTreeSet<String>,
    /// Nesting depth of `ty` calls: a guard against runaway inline expansion.
    depth: usize,
    /// Objects without `additionalProperties` keep unknown keys too (config
    /// `unknown_fields = "keep"`); explicitly open objects always do.
    pub keep_unknown: bool,
    /// The generated crate's dependencies (`-` as `_`): an object-form `x-rust-type` refers to one of them.
    pub crates: Vec<String>,
    /// The description was Swagger 2.0: an optional property decodes `null` as absent
    /// ([`crate::ir::types::Field::null_as_absent`]).
    pub from_v20: bool,
    /// The `$ref` targets whose keywords are being collected: one met again is a cycle, whose keywords its own type
    /// enforces.
    reading: RefCell<Vec<String>>,
    /// The schemas that typed checks decode the value as (`not`, `dependentSchemas`), by index, with the pointer of
    /// each when it has one. `finish` builds a type for each.
    subschemas: RefCell<Vec<(Value, Option<String>)>>,
    /// The index of each queued subschema by its pointer and its text: queuing one looks it up in one step.
    subschema_index: RefCell<BTreeMap<(Option<String>, String), usize>>,
    /// The JSON kind of each type once it is final ([`Kinds`]).
    kinds: RefCell<Vec<Option<JsonKind>>>,
    /// Schemas that no generated check enforces, by pointer, with what makes them so. `finish` reports each as an
    /// error.
    unchecked: RefCell<BTreeMap<String, String>>,
    /// Schemas nested too deep to type where they are ([`MAX_TYPE_DEPTH`]): each a reserved type, filled from no
    /// depth by `finish`.
    deferred: Vec<(TypeId, Value, String)>,
}

/// Returns `ty` with its aliases in `defs` followed at most 16 deep. An alias chain can loop.
pub(super) fn unalias<'t>(defs: &'t [TypeDef], ty: &'t Ty) -> &'t Ty {
    let mut cur = ty;
    for _ in 0..MAX_ALIAS_HOPS {
        let Ty::Named(id) = cur else { break };
        let Kind::Alias(t) = &defs[id.0].kind else { break };
        cur = t;
    }
    cur
}

impl<'a> Builder<'a> {
    /// Starts converting `doc`. Nothing gets a name in `reserved`, the own names of the target.
    pub(crate) fn new(doc: &'a Doc, diags: &'a mut Diagnostics, reserved: Reserved<'_>) -> Self {
        Builder {
            doc,
            diags,
            dynamic: crate::schema_ids::dynamic(&doc.root),
            defs: vec![],
            names: crate::ir::names::reserved(reserved.types),
            reserved_operations: crate::ir::names::reserved(reserved.operations),
            by_ptr: BTreeMap::new(),
            claimed: BTreeMap::new(),
            ptr_cache: BTreeMap::new(),
            in_progress: BTreeSet::new(),
            depth: 0,
            keep_unknown: false,
            crates: vec![],
            from_v20: false,
            reading: RefCell::new(vec![]),
            kinds: RefCell::default(),
            subschemas: RefCell::default(),
            subschema_index: RefCell::default(),
            unchecked: RefCell::default(),
            deferred: vec![],
        }
    }

    /// Registers and converts every `components.schemas` entry and keeps their names.
    pub(crate) fn components(&mut self) {
        let Some(Value::Object(schemas)) = pointer(&self.doc.root, "/components/schemas").cloned() else { return };
        let mut ids = vec![];
        for (name, schema) in &schemas {
            let ptr = join("/components/schemas", name);
            let id = self.reserve(&Name::new(name), &ptr);
            // A component that only points at a schema without a name of its own (`#/x-ext/<hash>` of a bundler)
            // is that schema under the name of the component. References to either reach it. The first claims it.
            // A later one is an alias.
            let ptr = match self.unnamed_target(schema) {
                Some(target) if !self.by_ptr.contains_key(&target) => {
                    self.by_ptr.insert(target.clone(), id);
                    self.claimed.insert(target.clone(), name.clone());
                    target
                }
                Some(_) | None => ptr,
            };
            ids.push((id, ptr));
        }
        for (id, ptr) in ids {
            let schema = pointer(&self.doc.root, &ptr).cloned().unwrap_or(Value::Bool(true));
            self.fill(id, &schema, &ptr);
        }
    }

    /// Returns the name that a `$ref` gives. That is the name of the component, also through a pointer that a component
    /// claimed. Otherwise the last token of the pointer. An implicit discriminator mapping uses it on the wire.
    fn ref_name(&self, r: &str) -> String {
        let claimed = self.doc.local_pointer(r).and_then(|t| self.claimed.get(&t).cloned());
        claimed.unwrap_or_else(|| json::unescape_token(r.rsplit('/').next().unwrap_or(r)))
    }

    /// Returns where `schema` points when it is only a `$ref` to a local schema outside `components/schemas`.
    fn unnamed_target(&self, schema: &Value) -> Option<String> {
        let o = schema.as_object().filter(|o| has_ref_only(o))?;
        let target = self.doc.local_pointer(o.get("$ref")?.as_str()?)?;
        (!target.starts_with("/components/schemas/")).then_some(target)
    }

    fn reserve(&mut self, name: &Name, ptr: &str) -> TypeId {
        let name = unique(name.clone(), &mut self.names);
        let id = TypeId(self.defs.len());
        self.defs.push(TypeDef {
            name,
            doc: None,
            kind: Kind::Pending,
            xml: XmlHint::default(),
            rust_type: None,
            examples: vec![],
            deprecated: false,
            json_string: false,
            component: named_by_description(ptr),
            own: Constraints::default(),
            lenient: false,
        });
        self.by_ptr.insert(ptr.to_owned(), id);
        id
    }

    /// Converts a component schema into the reserved def `id`.
    fn fill(&mut self, id: TypeId, schema: &Value, ptr: &str) {
        let name = self.defs[id.0].name.clone();
        let doc = doc_of(schema);
        let kind = match self.named_kind(schema, ptr, &name) {
            Some(kind) => kind,
            None => Kind::Alias(self.ty(schema, ptr, &name)),
        };
        // An alias of itself (`A: {$ref: A}`) is meaningless: make it Any.
        let kind = if matches!(kind, Kind::Alias(Ty::Named(t)) if t == id) { Kind::Alias(Ty::Any) } else { kind };
        self.defs[id.0].doc = doc;
        self.defs[id.0].kind = kind;
        self.defs[id.0].own = self.constraints_at_ptr(schema, ptr);
        // JSON Schema `examples` (an array) and the older `example`.
        let mut examples: Vec<(String, Value)> =
            schema.get("example").map(|v| (join(ptr, "example"), v.clone())).into_iter().collect();
        if let Some(Value::Array(xs)) = schema.get("examples") {
            examples.extend(xs.iter().enumerate().map(|(i, v)| (format!("{ptr}/examples/{i}"), v.clone())));
        }
        self.defs[id.0].examples = examples;
        self.defs[id.0].deprecated = schema.get("deprecated") == Some(&Value::Bool(true));
        self.defs[id.0].json_string = matches!(self.defs[id.0].kind, Kind::Newtype(_))
            && schema.as_object().is_some_and(|o| json_string_schema(o).is_some());
        // `x-rust-type: crate::domain::Money` reuses your type. So does the object form `{crate, version, path}`
        // when the crate is a dependency: `generate` checks that. An invalid path is ignored.
        let (path, krate) = match schema.get("x-rust-type") {
            Some(Value::String(p)) => (Some(p.as_str()), None),
            Some(Value::Object(o)) => (o.get("path").and_then(Value::as_str), o.get("crate").and_then(Value::as_str)),
            _ => (None, None),
        };
        self.defs[id.0].rust_type =
            path.filter(|p| p.trim_start_matches("::").split("::").all(crate::text::is_identifier)).map(String::from);
        if let Some(c) = krate.filter(|_| self.defs[id.0].rust_type.is_some())
            && !self.crates.contains(&c.replace('-', "_"))
        {
            let path = self.defs[id.0].rust_type.take().unwrap_or_default();
            self.diags.warning(
                join(ptr, "x-rust-type"),
                format!("`{path}` comes from crate `{c}`, which is not a dependency: generating a type instead"),
            );
        }
        self.defs[id.0].xml = XmlHint::of(schema);
    }

    /// Returns a property's `xml` object. Items' `$ref`s give the item name.
    fn xml_hint(&self, prop: &Value) -> XmlHint {
        // A property element is named by the property (or its own `xml.name`),
        // not by the referenced schema; items get the item schema's name.
        let mut hint = XmlHint::of(prop);
        if hint.item.is_none() {
            hint.item = self.array_items(prop).and_then(|items| self.xml_name(&items));
        }
        hint
    }

    /// Returns the validation keywords of a schema through `$ref`s and `allOf` members.
    pub(crate) fn constraints(&self, s: &Value) -> Constraints {
        self.constraints_at(s, 0, None)
    }

    /// Returns [`Builder::constraints`] of the schema at `ptr`: a `$dynamicRef` inside resolves from its resource, as
    /// it does where a `$ref` reaches the schema.
    fn constraints_at_ptr(&self, s: &Value, ptr: &str) -> Constraints {
        self.constraints_at(s, 0, Some(ptr))
    }

    /// Puts `patternProperties` of `s` into `c`: each pattern with the type of its schema, and what
    /// `additionalProperties` accepts of the other names.
    fn queue_patterns(
        &self,
        s: &Value,
        patterns: Option<&serde_json::Map<String, Value>>,
        at: Option<&str>,
        c: &mut Constraints,
    ) {
        // Without patterns and without `type`, the union of the implied shape stores the object, the check the other
        // names. With a `type`, the struct or map stores them, and a map of one pattern its names.
        let Some(patterns) = patterns else {
            if s.get("type").is_none() && s.get("patternProperties").is_none() {
                self.queue_closed(s, c);
            }
            return;
        };
        let at = at.map(|p| join(p, "patternProperties"));
        c.pattern_properties = (patterns.iter())
            .map(|(p, sub)| (p.clone(), self.queue_subschema(sub, at.as_deref().map(|a| join(a, p)))))
            .collect();
        self.queue_closed(s, c);
    }

    /// Puts `additionalProperties` (`false` or a schema) of `s` into `c`: the names that `s` declares, and what the
    /// other names that no pattern matches accept.
    fn queue_closed(&self, s: &Value, c: &mut Constraints) {
        let declared = s.get("properties").and_then(Value::as_object).into_iter().flat_map(serde_json::Map::keys);
        let declared = declared.cloned().collect();
        c.closed_to = match s.get("additionalProperties") {
            Some(Value::Bool(false)) => Some((declared, None)),
            Some(extra @ Value::Object(_)) => {
                let n = self.queue_subschema(extra, None);
                Some((declared, Some(n)))
            }
            _ => None,
        };
    }

    /// Returns `unevaluatedProperties` of `s` as generated code checks it: what `s` evaluates, and what the other names
    /// accept. `None` for `unevaluatedProperties: true`, which accepts every name.
    fn unevaluated(&self, s: &Value, at: Option<&str>) -> Option<(Evaluated, Leftover)> {
        let leftover = match s.get("unevaluatedProperties")? {
            Value::Bool(true) => return None,
            Value::Bool(false) => Leftover::Refused,
            schema @ (Value::Object(_) | Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_)) => {
                Leftover::Schema(self.queue_subschema(schema, at.map(|p| join(p, "unevaluatedProperties"))))
            }
        };
        Some((self.evaluation(s, 0, true)?, leftover))
    }

    /// Returns `unevaluatedItems` of `s` as generated code checks it: what `s` evaluates of a list, and what the other
    /// items accept. `None` for `unevaluatedItems: true`, which accepts every item.
    fn unevaluated_items(&self, s: &Value, at: Option<&str>) -> Option<(EvaluatedItems, Leftover)> {
        let leftover = match s.get("unevaluatedItems")? {
            Value::Bool(true) => return None,
            Value::Bool(false) => Leftover::Refused,
            schema @ (Value::Object(_) | Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_)) => {
                Leftover::Schema(self.queue_subschema(schema, at.map(|p| join(p, "unevaluatedItems"))))
            }
        };
        Some((self.evaluation(s, 0, true)?, leftover))
    }

    /// Returns what `s` evaluates ([`Evaluated`] of names, [`EvaluatedItems`] of items). With `queue`, the schema of
    /// each branch is queued for a type; without, nothing is. `None` when that is not known before the value is: a
    /// `dependentSchemas`, a `$dynamicRef` or a cycle of references.
    fn evaluation<E: Evaluation>(&self, s: &Value, depth: usize, queue: bool) -> Option<E> {
        if depth > MAX_SCHEMA_DEPTH {
            return None;
        }
        let o = match s {
            Value::Bool(_) => return Some(E::default()),
            Value::Object(o) => o,
            Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_) => return None,
        };
        if o.contains_key("$dynamicRef") {
            return None;
        }
        let index = |b: &Value| if queue { self.queue_subschema(b, None) } else { 0 };
        let mut ev = E::own(o, depth, &index);
        // A dependent schema evaluates when its name is present: when the value decodes as `{required: [name]}`.
        for (name, d) in o.get("dependentSchemas").and_then(Value::as_object).into_iter().flatten() {
            let branch = self.evaluation(d, depth + 1, queue)?;
            ev.branches().push((index(&serde_json::json!({"required": [name]})), true, branch));
        }
        if let Some(r) = o.get("$ref").and_then(Value::as_str) {
            let p = self.doc.local_pointer(r)?;
            let target = pointer(&self.doc.root, &p)?;
            if self.reading.borrow().contains(&p) {
                return None;
            }
            self.reading.borrow_mut().push(p.clone());
            let reached = self.evaluation(target, depth + 1, queue);
            self.reading.borrow_mut().pop();
            ev.merge(reached?);
        }
        for (_, m) in json::items(o.get("allOf")) {
            ev.merge(self.evaluation(m, depth + 1, queue)?);
        }
        for (_, b) in ["anyOf", "oneOf"].iter().flat_map(|k| json::items(o.get(*k))) {
            let branch = self.evaluation(b, depth + 1, queue)?;
            ev.branches().push((index(b), true, branch));
        }
        if let Some(cond) = o.get("if") {
            let n = index(cond);
            let mut then: E = self.evaluation(cond, depth + 1, queue)?;
            if let Some(t) = o.get("then") {
                then.merge(self.evaluation(t, depth + 1, queue)?);
            }
            ev.branches().push((n, true, then));
            if let Some(e) = o.get("else") {
                let otherwise = self.evaluation(e, depth + 1, queue)?;
                ev.branches().push((n, false, otherwise));
            }
        }
        Some(ev)
    }

    /// Does a `$dynamicRef` in `s` resolve to a schema that depends on where evaluation came from?
    /// ([`Builder::dynamic_ref_varies`]).
    fn dynamic_refs_vary(&self, s: &Value) -> bool {
        let mut refs = vec![];
        references(s, &["$dynamicRef"], &mut refs);
        refs.iter().any(|r| self.dynamic_ref_varies(r))
    }

    /// Does `$dynamicRef` `r` resolve to a schema that depends on where evaluation came from? Only when its target
    /// bookends it ([`Builder::bookend`]) and another resource that declares that `$dynamicAnchor` reaches the resource
    /// of the target: the outermost of them in scope wins. A binding site is specialized into a copy of its own, which
    /// does not reach the generic. Otherwise the reference always lands on its target, as its type does.
    fn dynamic_ref_varies(&self, r: &str) -> bool {
        let (Some(target), Some(name)) = self.bookend(r) else { return false };
        let resource = self.dynamic.resource_of(&target);
        (self.dynamic.anchors.keys())
            .filter(|(res, n)| *n == name && *res != resource)
            .any(|(res, _)| self.reaches(res, &resource, &mut BTreeSet::new()))
    }

    /// Does the schema at `from` refer, through any chain of references, into the schema at `to`?
    fn reaches(&self, from: &str, to: &str, seen: &mut BTreeSet<String>) -> bool {
        let mut refs = vec![];
        if let Some(s) = pointer(&self.doc.root, from) {
            references(s, &["$ref", "$dynamicRef"], &mut refs);
        }
        refs.into_iter().filter_map(|r| self.doc.local_pointer(r)).any(|p| {
            p == to || p.starts_with(&format!("{to}/")) || (seen.insert(p.clone()) && self.reaches(&p, to, seen))
        })
    }

    /// Returns the index of `schema`, at `at`, in [`Builder::subschemas`], queued once.
    fn queue_subschema(&self, schema: &Value, at: Option<String>) -> usize {
        let mut subschemas = self.subschemas.borrow_mut();
        let n = subschemas.len();
        let i = *self.subschema_index.borrow_mut().entry((at.clone(), schema.to_string())).or_insert(n);
        if i == n {
            subschemas.push((schema.clone(), at));
        }
        i
    }

    /// Queues the item schemas of a tuple (`prefixItems`, then `items`) as subschemas.
    fn queue_tuple(&self, s: &Value, at: Option<&str>) -> (Vec<usize>, Rest) {
        let prefix = s.get("prefixItems").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        let at_prefix = at.map(|p| join(p, "prefixItems"));
        let prefix = (prefix.iter().enumerate())
            .map(|(i, p)| self.queue_subschema(p, at_prefix.as_deref().map(|a| join(a, &i.to_string()))))
            .collect();
        let rest = match s.get("items") {
            None | Some(Value::Bool(true)) => Rest::Any,
            Some(Value::Bool(false)) => Rest::Refused,
            Some(items) => Rest::Schema(self.queue_subschema(items, at.map(|p| join(p, "items")))),
        };
        (prefix, rest)
    }

    /// Queues `not`, and `if` with its `then` and `else` unless a tagged union enforces them, as subschemas of `c`.
    fn queue_not_and_if(&self, s: &Value, at: Option<&str>, typed_conditional: bool, c: &mut Constraints) {
        if let Some(not) = s.get("not") {
            c.not.push(self.queue_subschema(not, at.map(|p| join(p, "not"))));
        }
        if let Some(cond) = s.get("if").filter(|_| !typed_conditional) {
            let queue = |k: &str| s.get(k).map(|b| self.queue_subschema(b, at.map(|p| join(p, k))));
            c.conditions.push((self.queue_subschema(cond, at.map(|p| join(p, "if"))), queue("then"), queue("else")));
        }
    }

    /// Queues the schemas that a value also decodes as: the keywords beside a union, and the members of an `allOf` that
    /// one merged type cannot represent.
    fn queue_also(&self, s: &Value, at: Option<&str>, c: &mut Constraints) {
        if let Some(beside) = beside_union(s) {
            c.also.push(self.queue_subschema(&beside, None));
        }
        // Members that one merged type cannot represent: the value also decodes as each of them.
        if self.lossy_all_of(s) {
            let at = at.map(|p| join(p, "allOf"));
            for (i, m) in json::items(s.get("allOf")) {
                c.also.push(self.queue_subschema(m, at.as_deref().map(|a| join(a, &i.to_string()))));
            }
        }
    }

    fn constraints_at(&self, s: &Value, depth: usize, at: Option<&str>) -> Constraints {
        // Keywords further down belong to the named types on the way, which enforce them. Only a schema checked whole
        // loses what it cuts (`inline_scoped`).
        if depth > MAX_SCHEMA_DEPTH {
            return Constraints::default();
        }
        let mut c = Constraints::own(s);
        let (dependents, names, pattern_map) = typed_object_keywords(s, &mut c);
        // What the names that nothing evaluated accept is known before the value is.
        let unevaluated =
            s.get("unevaluatedProperties").is_some() && self.evaluation::<Evaluated>(s, 0, false).is_some();
        let unevaluated_items =
            s.get("unevaluatedItems").is_some() && self.evaluation::<EvaluatedItems>(s, 0, false).is_some();
        // `propertyNames` of any other schema: each name decodes as its type.
        let names_any =
            !names && s.get("propertyNames").is_some() && s.as_object().is_some_and(|o| may_be(o, "object"));
        let dependent_schemas = typed_dependent_schemas(s);
        // `contains` on an array type counts the items that decode as the type of its schema.
        let typed_contains = s.get("contains").is_some() && s.as_object().is_some_and(|o| may_be(o, "array"));
        // A struct that stores its patterns decodes each name as them.
        let held_patterns = struct_patterns(s).is_some();
        let patterns = (!pattern_map && !held_patterns).then(|| typed_patterns(s)).flatten();
        // `if`/`then` on one property's constants becomes a tagged union (`conditional_union`): its variants enforce
        // the conditions. Only conditions that it cannot type stay a run-time check.
        let typed_conditional = typed_conditional(s);
        let conditional_key = |k: &str| {
            // An `if` that is no tagged union is a check of the types of its schemas.
            matches!(k, "if" | "then" | "else")
                || (dependents && k == "dependentRequired")
                || ((names || names_any) && k == "propertyNames")
                || (pattern_map && matches!(k, "patternProperties" | "additionalProperties"))
                || k == "not"
                || (dependent_schemas.is_some() && k == "dependentSchemas")
                || (unevaluated && k == "unevaluatedProperties")
                || (unevaluated_items && k == "unevaluatedItems")
                // On a schema whose `type` accepts no array, `contains` asserts nothing.
                || matches!(k, "contains" | "minContains" | "maxContains")
                || ((patterns.is_some() || held_patterns) && k == "patternProperties")
        };
        let conditional_member = |m: &Value| {
            typed_conditional
                && m.as_object().is_some_and(|m| m.contains_key("if") && m.keys().all(|k| conditional_key(k)))
        };
        // A `$dynamicRef` inside whose anchor several resources declare resolves in the dynamic scope from here: only
        // the run-time check of the whole schema follows it there.
        let dynamic = self.dynamic_refs_vary(s);
        // A tuple (`prefixItems`, maybe closed by `items: false`) is checked item by item ([`Constraints::tuple`]).
        let typed_tuple = !dynamic && checked_items(s);
        let mixed = mixed(s);
        let scoped = typeless_assertions(s) && kind_scoped(s);
        // Without `type`, a value of any kind or of the union of an implied shape: `required` and `format` by kind.
        if scoped || s.as_object().is_some_and(|o| inferred_kind(o).is_some()) {
            scoped_keywords(s, &mut c);
        }
        let untyped = untyped(s, scoped, [dynamic, mixed], &conditional_key);
        if untyped.is_empty() {
            self.queue_not_and_if(s, at, typed_conditional, &mut c);
            if let Some(sub) = s.get("contains").filter(|_| typed_contains) {
                let n = self.queue_subschema(sub, at.map(|p| join(p, "contains")));
                let contains = Some((n, count_of(s, "minContains").unwrap_or(1), count_of(s, "maxContains")));
                if s.get("type").is_none() { c.json_contains = contains } else { c.contains = contains }
            }
            self.queue_branch_counts(s, at, &mut c);
            self.queue_patterns(s, patterns, at, &mut c);
            if typed_tuple {
                c.tuple = Some(self.queue_tuple(s, at));
            }
            self.queue_also(s, at, &mut c);
            untyped_value_checks(s, &mut c);
            c.unevaluated = unevaluated.then(|| self.unevaluated(s, at)).flatten();
            c.unevaluated_items = unevaluated_items.then(|| self.unevaluated_items(s, at)).flatten();
            if let Some(pn) = s.get("propertyNames").filter(|_| names_any) {
                c.names_schema = Some(self.queue_subschema(pn, at.map(|p| join(p, "propertyNames"))));
            }
            for (name, sub) in dependent_schemas.into_iter().flatten() {
                let at = at.map(|p| join(&join(p, "dependentSchemas"), name));
                c.dependent_schemas.push((name.clone(), self.queue_subschema(sub, at)));
            }
        } else {
            // No generated check enforces them: generation stops with the keywords that it cannot check.
            self.unchecked.borrow_mut().entry(at.unwrap_or_default().to_owned()).or_insert_with(|| untyped.join(", "));
        }
        for m in s.get("allOf").and_then(Value::as_array).into_iter().flatten().filter(|m| !conditional_member(m)) {
            c.merge(self.constraints_at(m, depth + 1, None));
        }
        self.merge_reached(s, depth, &mut c);
        c
    }

    /// Merges into `c` the keywords of what `s` reaches as one value: its `$ref` target, the one branch beside `null`
    /// and the constraints of its items.
    fn merge_reached(&self, s: &Value, depth: usize, c: &mut Constraints) {
        // One hop: a target that has keywords beside its own `$ref` (3.1) keeps them. Following the whole chain
        // would skip them.
        // A target that is being collected already is a cycle: its type enforces its keywords where it recurs.
        if let Some(p) = s.get("$ref").and_then(Value::as_str).and_then(|r| self.doc.local_pointer(r))
            && let Some(target) = pointer(&self.doc.root, &p)
            && !self.reading.borrow().contains(&p)
        {
            self.reading.borrow_mut().push(p.clone());
            c.merge(self.constraints_at(target, depth + 1, Some(&p)));
            self.reading.borrow_mut().pop();
        }
        // `anyOf: [{type: integer, maximum: 10}, {type: null}]` is typed as its one other branch. So that branch's
        // keywords are the value's.
        if let [branch] = only_non_null_branch(s).as_slice() {
            c.merge(self.constraints_at(branch, depth + 1, None));
        }
        // Beside `prefixItems`, `items` applies to the rest only: the tuple checks it.
        if c.items.is_none()
            && s.get("prefixItems").is_none()
            && let Some(items) = s.get("items")
        {
            let ic = self.constraints_at(items, depth + 1, None);
            if !ic.is_empty() {
                c.items = Some(Box::new(ic));
            }
        }
    }

    /// Returns an `allOf` that one merged type cannot represent. The members merge into one struct (or the type they
    /// share) with one value per keyword. So two members that set the same keyword (two `maximum`s, two
    /// `additionalProperties`), or whose `type`s share none, would lose one of them: the whole schema is then checked
    /// at run time. A property that two members declare is the `allOf` of its schemas, which its field enforces.
    /// `required` adds up, one shared `type` is kept and each member's untyped keywords are checked with it. So the
    /// usual `allOf: [$ref: Base, {type: object, properties: ...}]` and an `allOf` of `if`/`then` pairs stay typed.
    fn lossy_all_of(&self, s: &Value) -> bool {
        let Some(o) = s.as_object() else { return false };
        let Some(members) = o.get("allOf").and_then(Value::as_array) else { return false };
        let members: Vec<Value> = self
            .flat_members(members, 0)
            .iter()
            .filter_map(|m| self.deref(m).map(|(m, _)| m))
            .filter(|m| !is_annotation_only(m))
            .collect();
        let mut keys = BTreeSet::new();
        // The schema's own keywords beside `allOf` merge in too.
        let mut own = o.clone();
        own.remove("allOf");
        let all: Vec<Value> = std::iter::once(Value::Object(own)).chain(members.iter().cloned()).collect();
        let has = |m: &Value, ks: &[&str]| ks.iter().any(|k| m.get(*k).is_some());
        // `additionalProperties` sees only the names of its own schema, and `items` only its own `prefixItems`: a
        // merged type would extend them to those of the others.
        let lone = |ks: &[&str], others: &[&str]| {
            all.iter()
                .enumerate()
                .any(|(i, m)| has(m, ks) && all.iter().enumerate().any(|(j, n)| i != j && has(n, others)))
        };
        if lone(&["additionalProperties"], &["properties", "patternProperties", "additionalProperties"])
            || lone(&["items", "prefixItems"], &["items", "prefixItems"])
        {
            return true;
        }
        for m in &all {
            let Some(mo) = m.as_object() else { continue };
            for k in mo.keys() {
                // A member's untyped keywords (`if`, `not`) are checked whole with that member already. A property
                // that two members declare merges into the `allOf` of its schemas: its field enforces both.
                let lossless = matches!(k.as_str(), "type" | "required" | "allOf" | "$ref" | "properties")
                    || UNTYPED_KEYWORDS.contains(&k.as_str())
                    || ANNOTATIONS.contains(&k.as_str())
                    || k.starts_with("x-");
                if !lossless && !keys.insert(k.clone()) {
                    return true;
                }
            }
        }
        members.iter().any(|m| m.get("type").is_some()) && self.narrowed_types(&members).is_none()
    }

    /// Returns a `oneOf` or `anyOf` that its union type cannot judge alone: a branch without one `type` of its own, two
    /// `oneOf` branches of one type (it means exactly one) or keywords beside the union (`type: string` next to
    /// `anyOf`). Branches of distinct types (`integer` or `string`) are judged by their variant. A discriminator does
    /// not change this (3.2: it "MUST NOT change the validation outcome").
    fn loose_union(&self, s: &Value) -> bool {
        let Some(o) = s.as_object() else { return false };
        let Some(branches) = ["oneOf", "anyOf"].iter().find_map(|k| o.get(*k)?.as_array().filter(|a| !a.is_empty()))
        else {
            return false;
        };
        // The tag picks the one branch a value can match, and that variant checks the rest.
        if self.pins_every_tag(o, branches) {
            return false;
        }
        // `anyOf` asks for one branch at least: decoding checks every keyword of a branch, so the first branch that
        // decodes proves it, whatever types the branches share. `oneOf` asks for exactly one: its untagged union
        // decodes every branch and counts. A discriminator that does
        // not pin each branch picks one by its tag and would not count, so the whole schema is checked then.
        let one_of = o.contains_key("oneOf");
        // A branch that asserts nothing accepts any value: an `anyOf` with one is true for every value, which its
        // variant of any value decodes.
        if !one_of && branches.iter().any(asserts_nothing) {
            return false;
        }
        // Two branches of one type under a discriminator that picks one: `counted_one_of` counts the branches.
        branches.iter().any(|b| self.deref(b).is_none_or(|(b, _)| b.get("type").and_then(Value::as_str).is_none()))
    }

    /// Queues the branches of a union that its type cannot judge alone as subschemas of `c`: those of a `oneOf` that a
    /// value must decode as exactly one of ([`Builder::counted_one_of`] or [`Builder::loose_union`]), and those of a
    /// loose `anyOf`, one of which at least.
    fn queue_branch_counts(&self, s: &Value, at: Option<&str>, c: &mut Constraints) {
        let loose = self.loose_union(s);
        let one_of = self.counted_one_of(s).or_else(|| s.get("oneOf").and_then(Value::as_array).filter(|_| loose));
        let any_of = s.get("anyOf").and_then(Value::as_array).filter(|_| loose);
        for (key, branches, out) in [("oneOf", one_of, &mut c.one_of), ("anyOf", any_of, &mut c.any_of)] {
            for (i, branch) in branches.into_iter().flatten().enumerate() {
                let at = at.map(|p| join(&join(p, key), &i.to_string()));
                out.push(self.queue_subschema(branch, at));
            }
        }
    }

    /// Returns the branches of a `oneOf` whose discriminator picks one branch by its tag, while two branches share a
    /// type: a value may then match more than one. Its check counts the branches that the value decodes as.
    fn counted_one_of<'s>(&self, s: &'s Value) -> Option<&'s Vec<Value>> {
        let o = s.as_object()?;
        let branches = o.get("oneOf")?.as_array().filter(|_| o.contains_key("discriminator"))?;
        if self.pins_every_tag(o, branches) {
            return None;
        }
        let mut types = BTreeSet::new();
        let shared = branches.iter().any(|b| {
            let t = self.deref(b).and_then(|(b, _)| b.get("type").and_then(Value::as_str).map(str::to_owned));
            t.is_some_and(|t| !types.insert(t))
        });
        (shared && !self.loose_union(s)).then_some(branches)
    }

    /// Does every branch of the union `o` fix its discriminating property to a value of its own? The tag then picks the
    /// one branch that a value can match.
    fn pins_every_tag(&self, o: &serde_json::Map<String, Value>, branches: &[Value]) -> bool {
        let tag = o.get("discriminator").and_then(|d| d.get("propertyName")).and_then(Value::as_str);
        tag.is_some_and(|prop| branches.iter().all(|b| self.pins_tag(b, prop, 0)))
    }

    /// Does `branch` (through `$ref`s and `allOf` members) fix `prop` with `const` or a one-value `enum`?
    fn pins_tag(&self, branch: &Value, prop: &str, depth: usize) -> bool {
        if depth > MAX_SCHEMA_DEPTH {
            return false;
        }
        let Some((b, _)) = self.deref(branch) else { return false };
        let own = b.get("properties").and_then(|p| p.get(prop)).is_some_and(json::fixes_one_value);
        own || b
            .get("allOf")
            .and_then(Value::as_array)
            .is_some_and(|m| m.iter().any(|m| self.pins_tag(m, prop, depth + 1)))
    }

    /// Returns the `items` schema of an array reached through `$ref` and `allOf`.
    fn array_items(&self, s: &Value) -> Option<Value> {
        let (s, _) = self.deref(s)?;
        if let Some(items) = s.get("items") {
            return Some(items.clone());
        }
        s.get("allOf").and_then(Value::as_array)?.iter().find_map(|m| self.array_items(m))
    }

    /// Returns `xml.name` of a schema through `$ref` and `allOf` members.
    fn xml_name(&self, s: &Value) -> Option<String> {
        if let Some(n) = XmlHint::of(s).name {
            return Some(n);
        }
        if let Some(members) = s.get("allOf").and_then(Value::as_array) {
            return members.iter().find_map(|m| self.xml_name(m));
        }
        let (target, _) = s.get("$ref").and_then(|_| self.deref(s))?;
        self.xml_name(&target)
    }

    /// Is `ty` (through aliases) a struct?
    pub(crate) fn is_struct(&self, ty: &Ty) -> bool {
        matches!(unalias(&self.defs, ty), Ty::Named(id) if matches!(self.defs[id.0].kind, Kind::Struct(_)))
    }

    /// Schemas that need a named type of their own (struct, enum or union). `None`
    /// means that the schema maps to an existing type expression.
    fn named_kind(&mut self, s: &Value, ptr: &str, name: &Name) -> Option<Kind> {
        // `unevaluatedProperties` judges the names that its branches and members leave. The run-time check sees the
        // value as it re-serializes, so the objects it contains keep the names that they do not declare.
        let unevaluated = s.get("unevaluatedProperties").is_some_and(|u| u != &Value::Bool(true));
        let keep = self.keep_unknown;
        self.keep_unknown |= unevaluated;
        let kind = self.own_kind(s, ptr, name);
        self.keep_unknown = keep;
        kind
    }

    /// [`Self::named_kind`] without the scope of `unevaluatedProperties`.
    fn own_kind(&mut self, s: &Value, ptr: &str, name: &Name) -> Option<Kind> {
        let o = s.as_object()?;
        if has_ref_only(o) {
            return None;
        }
        let (types, _) = type_set(o);
        let non_null: Vec<&str> = types.iter().map(String::as_str).filter(|t| *t != "null").collect();
        if let Some(c) = typed_const(o) {
            return Some(Kind::Const(c.clone()));
        }
        if checked_scalar(o) {
            let base = self.scalar(o, ptr, name);
            return Some(Kind::Checked(Box::new(Checked { base, constraints: Constraints::own(s) })));
        }
        if let Some(Value::Object(u)) = self.sibling_union(o, ptr) {
            return self.union(&u, ptr, name);
        }
        if o.contains_key("oneOf") || o.contains_key("anyOf") {
            return self.union(o, ptr, name);
        }
        if let Some(inner) = json_string_schema(o) {
            return Some(Kind::Newtype(self.ty(inner, &join(ptr, "contentSchema"), &name.and(&Name::new("Content")))));
        }
        // `properties` or `items` without `type`: they apply to objects or arrays only, and any other value is valid
        // too (2020-12 section 7.6.1). One variant stores the shape, `Other` the rest.
        if let Some(kind) = inferred_kind(o) {
            self.diags.warning(
                ptr,
                format!(
                    "no `type`: the `{kind}` keywords here apply to {kind}s only and any other value is valid too, so \
                     the type has an `Other` variant. Add `type: {kind}` for a plain {kind}"
                ),
            );
            let mut union = self.multi_type(o, &[kind], ptr, name);
            if let Kind::Union(u) = &mut union {
                u.rest = true;
                u.mode = UnionMode::ByKind;
            }
            return Some(union);
        }
        // `if`/`then` on one property's constant is a tagged union in disguise.
        if let Some(Value::Object(u)) = conditional_union(o) {
            return self.union(&u, ptr, name);
        }
        if let Some(Value::Object(u)) = self.distributed_all_of(o, ptr) {
            return self.union(&u, ptr, name);
        }
        if o.contains_key("allOf") {
            return self.all_of(o, ptr, name);
        }
        if let Some(ints) = int_enum_values(o) {
            return Some(int_enum(o, &ints));
        }
        if let Some(Value::Array(values)) = o.get("enum") {
            let strings: Vec<&str> = json::strs(values).collect();
            let non_null_values = values.iter().filter(|v| !v.is_null()).count();
            if !strings.is_empty() && strings.len() == non_null_values && non_null.iter().all(|t| *t == "string") {
                return Some(str_enum(&strings));
            }
            if let Some(u) = values_union(o) {
                return self.union(&u, ptr, name);
            }
            return None;
        }
        if non_null.len() > 1 {
            return Some(self.multi_type(o, &non_null, ptr, name));
        }
        let is_object = non_null.first() == Some(&"object") || (non_null.is_empty() && o.contains_key("properties"));
        let patterned = struct_patterns(s).is_some();
        if is_object && (json::object(o, "properties").is_some_and(|p| !p.is_empty()) || only_empty(o) || patterned) {
            return Some(Kind::Struct(self.object(o, ptr, name)));
        }
        None
    }

    /// Returns the type expression for a schema at `ptr`. `hint` gives the name of any inline type.
    pub(crate) fn ty(&mut self, s: &Value, ptr: &str, hint: &Name) -> Ty {
        let Some(o) = s.as_object() else { return Ty::Any };
        if self.depth > MAX_TYPE_DEPTH {
            // Its type accepts `null` too: the reserved type is the whole schema.
            if let Some(id) = self.by_ptr.get(ptr) {
                return Ty::Named(*id);
            }
            let id = self.reserve(hint, ptr);
            self.deferred.push((id, s.clone(), ptr.to_owned()));
            return Ty::Named(id);
        }
        let (_, nullable) = type_set(o);
        self.depth += 1;
        let base = self.ty_non_null(s, ptr, hint);
        self.depth -= 1;
        if nullable { base.option() } else { base }
    }

    fn ty_non_null(&mut self, s: &Value, ptr: &str, hint: &Name) -> Ty {
        let Some(o) = s.as_object() else { return Ty::Any };
        if let Some(r) = o.get("$ref").and_then(Value::as_str) {
            if has_ref_only(o) {
                return self.ty_ref(r, ptr);
            }
            // `$ref` with sibling constraints. Treat it as allOf [ref, rest]. `unevaluated*` stays beside the
            // `allOf`: it sees what the reference evaluates too (2020-12 section 11.3).
            let mut rest = o.clone();
            rest.remove("$ref");
            let mut merged = serde_json::Map::new();
            for k in ["unevaluatedProperties", "unevaluatedItems"] {
                if let Some(v) = rest.remove(k) {
                    merged.insert(k.to_owned(), v);
                }
            }
            merged.insert("allOf".to_owned(), serde_json::json!([{"$ref": r}, Value::Object(rest)]));
            let merged = Value::Object(merged);
            return self.ty_non_null(&merged, ptr, hint);
        }
        if let Some(r) = o.get("$dynamicRef").and_then(Value::as_str) {
            // Bookended by more than one resource and not bound by a generic: it resolves in the dynamic scope,
            // somewhere a type cannot know. The run-time check judges the value.
            if self.dynamic_ref_varies(r) {
                return Ty::Any;
            }
            return self.ty_ref(r, ptr);
        }
        // Anything needing its own item becomes an inline named type.
        let name = hint.clone();
        if Self::needs_item(o) {
            if let Some(id) = self.by_ptr.get(ptr) {
                return Ty::Named(*id);
            }
            let id = self.reserve(&name, ptr);
            match self.named_kind(s, ptr, &self.defs[id.0].name.clone()) {
                Some(kind) => {
                    self.defs[id.0].doc = doc_of(s);
                    self.defs[id.0].json_string = matches!(kind, Kind::Newtype(_)) && json_string_schema(o).is_some();
                    self.defs[id.0].kind = kind;
                    self.defs[id.0].own = self.constraints_at_ptr(s, ptr);
                    return Ty::Named(id);
                }
                None => {
                    // Not an item after all (e.g. single-branch union): drop the def.
                    let ty = self.scalar(o, ptr, hint);
                    self.defs[id.0].kind = Kind::Alias(ty.clone());
                    return ty;
                }
            }
        }
        self.scalar(o, ptr, hint)
    }

    /// Returns whether a literal `default` can be written as a value of `t`. (scalars and enums through aliases)
    pub(crate) fn literal_type(&self, t: &Ty) -> bool {
        let t = unalias(&self.defs, t);
        if let Ty::Named(id) = t {
            return match &self.defs[id.0].kind {
                Kind::StrEnum(_) | Kind::IntEnum(_) => true,
                // A default of a checked type is built without its checks: the test of the examples checks it.
                Kind::Checked(c) => self.literal_type(&c.base),
                Kind::Struct(_)
                | Kind::Union(_)
                | Kind::Alias(_)
                | Kind::Newtype(_)
                | Kind::Const(_)
                | Kind::Pending => false,
            };
        }
        matches!(
            t,
            Ty::Bool
                | Ty::I8
                | Ty::I16
                | Ty::I32
                | Ty::I64
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::F32
                | Ty::F64
                | Ty::Decimal
                | Ty::String
        )
    }

    fn needs_item(o: &serde_json::Map<String, Value>) -> bool {
        let patterned = struct_patterns(&Value::Object(o.clone())).is_some();
        if typed_const(o).is_some() || checked_scalar(o) || inferred_kind(o).is_some() || patterned {
            return true;
        }
        if o.contains_key("oneOf")
            || o.contains_key("anyOf")
            || o.contains_key("allOf")
            || json_string_schema(o).is_some()
        {
            return true;
        }
        let (types, _) = type_set(o);
        let non_null: Vec<&String> = types.iter().filter(|t| *t != "null").collect();
        if non_null.len() > 1 {
            return true;
        }
        if let Some(Value::Array(values)) = o.get("enum") {
            let integers = non_null.iter().all(|t| *t == "integer") && values.iter().all(|v| v.is_null() || v.is_i64());
            return values.iter().any(Value::is_string) || (integers && values.iter().any(Value::is_i64));
        }
        (json::object(o, "properties").is_some_and(|p| !p.is_empty()) || only_empty(o))
            && non_null.first().is_none_or(|t| *t == "object")
    }

    /// Returns an array. That is a closed tuple, a fixed-length array or a `Vec`.
    fn array(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, hint: &Name) -> Ty {
        match (o.get("prefixItems"), o.get("items")) {
            (Some(Value::Array(prefix)), Some(Value::Bool(false)))
                if !prefix.is_empty()
                    && prefix.len() <= MAX_TUPLE
                    && o.get("minItems").and_then(json::count).is_some_and(|m| m >= prefix.len() as u64) =>
            {
                let base = join(ptr, "prefixItems");
                Ty::Tuple(
                    prefix
                        .iter()
                        .enumerate()
                        .map(|(i, s)| self.ty(s, &join(&base, &i.to_string()), &hint.numbered(i)))
                        .collect(),
                )
            }
            (None, Some(items)) => {
                let item = self.ty(items, &join(ptr, "items"), &hint.and(&Name::new("Item")));
                match fixed_len(o) {
                    Some(n) => Ty::Array(Box::new(item), n),
                    None => Ty::Vec(Box::new(item)),
                }
            }
            _ => Ty::Vec(Box::new(Ty::Any)),
        }
    }

    /// Returns types that never need a named item.
    fn scalar(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, hint: &Name) -> Ty {
        let (types, _) = type_set(o);
        // Values of several kinds and no type for them all, such as `enum: [6, "foo", []]`: the first kind would refuse
        // the others. The run-time check judges the value (`untyped_values`).
        if types.iter().filter(|t| *t != "null").count() > 1 {
            return Ty::Any;
        }
        // `null` beside another type makes that type nullable. Alone it is the type: `type: null` allows only `null`.
        let t = types.iter().find(|t| *t != "null").or_else(|| types.first()).map(String::as_str);
        let format = o.get("format").and_then(Value::as_str).unwrap_or("");
        match t {
            Some("boolean") => Ty::Bool,
            Some("integer") => int_format(format).unwrap_or(Ty::I64),
            // The registry lists the integer formats for `number` too.
            Some("number") if format == "float" => Ty::F32,
            // "A fixed point decimal number of unspecified precision and range". Its digits and never an f64.
            Some("number") if matches!(format, "decimal" | "decimal128") => Ty::Decimal,
            Some("number") => int_format(format).unwrap_or(Ty::F64),
            // 3.0 `format: byte` is base64 like 3.1 `contentEncoding: base64`; 3.1 writes 3.0's `format: binary`
            // as a `contentMediaType` that is not text.
            Some("string")
                if format == "binary"
                    || format == "byte"
                    || o.get("contentEncoding").and_then(Value::as_str) == Some("base64")
                    || o.get("contentMediaType").and_then(Value::as_str).is_some_and(binary_media) =>
            {
                Ty::Binary
            }
            Some("string") if o.get("contentEncoding").and_then(Value::as_str) == Some("base64url") => {
                Ty::Format(Format::Base64Url)
            }
            Some("string") => match string_format(format) {
                Some(f) => {
                    // The format's type decides the value: `generate` does not check a `pattern` beside it.
                    if let Some(p) = o.get("pattern").and_then(Value::as_str) {
                        self.diags.warning(
                            join(ptr, "pattern"),
                            format!("pattern `{p}` is not checked: the value is the type of `format: {format}`"),
                        );
                    }
                    Ty::Format(f)
                }
                None => Ty::String,
            },
            Some("null") => Ty::Unit,
            Some("array") => self.array(o, ptr, hint),
            Some("object") | None => {
                if let Some(c) = o.get("const") {
                    return const_ty(c);
                }
                // One `patternProperties` schema and no other name allowed types every value; the key pattern is still
                // checked. An open map stores any value under a name that the pattern does not match.
                let single_pattern = o.get("patternProperties").and_then(Value::as_object).filter(|p| p.len() == 1);
                if let Some((pat, schema)) = single_pattern.and_then(|p| p.iter().next())
                    && !o.contains_key("properties")
                    && o.get("additionalProperties") == Some(&Value::Bool(false))
                {
                    let at = join(&join(ptr, "patternProperties"), pat);
                    return Ty::Map(Box::new(self.ty(schema, &at, &hint.and(&Name::new("Value")))));
                }
                // Beside `patternProperties`, `additionalProperties` applies to the names that no pattern matches: the
                // check judges each value.
                match o.get("additionalProperties") {
                    Some(ap @ Value::Object(m)) if !m.is_empty() && !o.contains_key("patternProperties") => Ty::Map(
                        Box::new(self.ty(ap, &join(ptr, "additionalProperties"), &hint.and(&Name::new("Value")))),
                    ),
                    _ if t == Some("object")
                        || o.contains_key("properties")
                        || o.contains_key("additionalProperties") =>
                    {
                        Ty::Map(Box::new(Ty::Any))
                    }
                    _ => Ty::Any,
                }
            }
            Some(_) => Ty::Any,
        }
    }

    fn ty_ref(&mut self, r: &str, at: &str) -> Ty {
        let Some(target) = self.doc.local_pointer(r) else {
            self.diags.warning(at, format!("`{r}` does not resolve; using an untyped value"));
            return Ty::Any;
        };
        if let Some(id) = self.by_ptr.get(&target).copied() {
            // A named type that also accepts `null` (`type: [string, "null"]` or `null` among its `enum` values) is
            // declared without it. The reference carries it. So an optional field can tell `null` from absent.
            let accepts_null =
                pointer(&self.doc.root, &target).and_then(Value::as_object).is_some_and(|o| type_set(o).1);
            let alias = matches!(self.defs[id.0].kind, Kind::Alias(_));
            return if accepts_null && !alias { Ty::Named(id).option() } else { Ty::Named(id) };
        }
        if let Some(ty) = self.ptr_cache.get(&target) {
            return ty.clone();
        }
        if !self.in_progress.insert(target.clone()) {
            return Ty::Any; // cycle through a non-component pointer
        }
        let schema = pointer(&self.doc.root, &target).cloned().unwrap_or(Value::Bool(true));
        let hint = target.rsplit('/').next().map(json::unescape_token).unwrap_or_default();
        let ty = self.ty(&schema, &target, &Name::new(&hint));
        self.in_progress.remove(&target);
        self.ptr_cache.insert(target, ty.clone());
        ty
    }

    /// Returns where `$dynamicRef` `r` points statically, and its anchor name when the target bookends it: the target
    /// declares a `$dynamicAnchor` that `r` names (`#node`), or `r` is the pointer the `$id` pass wrote for a bookended
    /// reference.
    pub(crate) fn bookend(&self, r: &str) -> (Option<String>, Option<String>) {
        let ptr = self.doc.local_pointer(r);
        let anchor = ptr
            .as_deref()
            .and_then(|p| pointer(&self.doc.root, p))
            .and_then(|t| t.get("$dynamicAnchor"))
            .and_then(Value::as_str)
            .filter(|a| r.starts_with("#/") || r.strip_prefix('#') == Some(*a))
            .map(String::from);
        (ptr, anchor)
    }

    /// Follows `$ref`s to the schema object they point at.
    fn deref(&self, s: &Value) -> Option<(Value, String)> {
        let mut cur = s.clone();
        let mut ptr = String::new();
        for _ in 0..MAX_ALIAS_HOPS {
            match cur.get("$ref").and_then(Value::as_str) {
                Some(r) => {
                    ptr = self.doc.local_pointer(r)?;
                    cur = pointer(&self.doc.root, &ptr)?.clone();
                }
                None => return Some((cur, ptr)),
            }
        }
        None
    }

    /// Finishes the space: resolves placeholders, drops defaults that the type of their field cannot represent and
    /// breaks cycles with `Box`.
    pub(crate) fn finish(mut self) -> TypeSpace {
        self.fill_deferred();
        let subschemas = self.build_subschemas();
        for (at, why) in std::mem::take(self.unchecked.get_mut()) {
            self.diags.error(at, format!("generated code cannot check this schema: {why}"));
        }
        for d in &mut self.defs {
            if matches!(d.kind, Kind::Pending) {
                d.kind = Kind::Alias(Ty::Any);
            }
        }
        by_kind_when_known(&mut self.defs);
        let unfit: Vec<(usize, usize)> = (self.defs.iter().enumerate())
            .filter_map(|(i, d)| if let Kind::Struct(s) = &d.kind { Some((i, s)) } else { None })
            .flat_map(|(i, s)| s.fields.iter().enumerate().map(move |(j, f)| (i, j, f)))
            .filter(|(_, _, f)| f.default.is_some() && !self.literal_type(&f.ty))
            .map(|(i, j, _)| (i, j))
            .collect();
        for (i, j) in unfit {
            if let Kind::Struct(s) = &mut self.defs[i].kind {
                // Only optional properties carry a default.
                (s.fields[j].default, s.fields[j].required) = (None, false);
            }
        }
        let mut space = TypeSpace::new(self.defs);
        space.subschemas = subschemas;
        break_cycles(&mut space);
        space
    }
}

impl Builder<'_> {
    /// Builds the type of each queued schema, with its constraints. A schema inside one may queue more.
    /// Fills the types of the schemas that were nested too deep to type where they are, each from no depth. Filling
    /// one may defer more.
    fn fill_deferred(&mut self) {
        let depth = std::mem::take(&mut self.depth);
        while let Some((id, schema, ptr)) = self.deferred.pop() {
            self.fill(id, &schema, &ptr);
        }
        self.depth = depth;
    }

    fn build_subschemas(&mut self) -> Vec<(Ty, Constraints)> {
        let mut built = vec![];
        // Such a type judges the whole value: its objects keep the names that they do not declare, which its checks may
        // need (`required` of an undeclared name).
        let keep = std::mem::replace(&mut self.keep_unknown, true);
        loop {
            // The borrow ends here: building the type may queue more.
            let next = self.subschemas.borrow().get(built.len()).cloned();
            let Some((schema, at)) = next else { break };
            let ptr = at.clone().unwrap_or_else(|| format!("#/x-honig-not/{}", built.len()));
            let ty = self.ty(&schema, &ptr, &Name::new("Not"));
            self.fill_deferred();
            let c = match &at {
                Some(p) => self.constraints_at_ptr(&schema, p),
                None => self.constraints(&schema),
            };
            built.push((ty, c));
        }
        self.keep_unknown = keep;
        built
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;
    use serde_json::json;

    fn build(schemas: &Value) -> TypeSpace {
        let doc = Doc::new(json!({"openapi": "3.2.0", "components": {"schemas": schemas}}));
        let mut d = Diagnostics::default();
        let mut b = Builder::new(&doc, &mut d, Reserved::default());
        b.components();
        b.finish()
    }

    fn by_name<'a>(s: &'a TypeSpace, n: &str) -> &'a TypeDef {
        s.defs.iter().find(|d| d.name == Name::new(n)).unwrap_or_else(|| panic!("no {n}"))
    }

    /// Returns the keywords that generated code cannot check in the schema `S`.
    fn unchecked(schema: &Value) -> String {
        let doc = Doc::new(json!({"openapi": "3.2.0", "components": {"schemas": {"S": schema}}}));
        let mut d = Diagnostics::default();
        let mut b = Builder::new(&doc, &mut d, Reserved::default());
        b.components();
        b.finish();
        d.iter().filter_map(|x| x.message.strip_prefix("generated code cannot check this schema: ")).collect()
    }

    /// A keyword that a type or a generated check enforces is checked; beside it, one that none enforces still stops
    /// generation. The same keyword where nothing enforces it stops it too.
    #[test]
    fn a_held_keyword_hides_no_other() {
        let held = [
            json!({"type": "object", "dependentRequired": {"a": ["b"]}}),
            json!({"type": "object", "propertyNames": {"maxLength": 3}}),
            json!({"propertyNames": {"maxLength": 3}}),
            json!({"type": "object", "patternProperties": {"^a": {"type": "string"}}, "additionalProperties": false}),
            json!({"type": "object", "patternProperties": {"^a": {"type": "string"}}}),
            json!({"type": "object", "dependentSchemas": {"a": {}}}),
            json!({"type": "object", "properties": {"a": {}}, "unevaluatedProperties": false}),
            json!({"type": "array", "prefixItems": [{}], "unevaluatedItems": false}),
        ];
        for mut s in held {
            s["dependencies"] = json!({"a": ["b"]});
            assert_eq!(unchecked(&s), "dependencies", "{s}");
        }
        let dynamic = json!([{"$dynamicRef": "#m"}]);
        let unheld = [
            (json!({"type": "string", "dependentRequired": {"a": ["b"]}}), "dependentRequired"),
            (json!({"type": "string", "propertyNames": {"maxLength": 3}}), "propertyNames"),
            (json!({"type": "string", "patternProperties": {"^a": {}}}), "patternProperties"),
            (json!({"type": "object", "allOf": dynamic, "unevaluatedProperties": false}), "unevaluatedProperties"),
            (json!({"type": "array", "allOf": dynamic, "unevaluatedItems": false}), "unevaluatedItems"),
        ];
        for (s, keyword) in unheld {
            assert_eq!(unchecked(&s), keyword, "{s}");
        }
    }

    /// An integer enum's variants are named by their values, or by `x-enum-varnames` when it lists one for each.
    #[test]
    fn integer_variants_are_named_by_value() {
        let s = build(
            &json!({"E": {"type": "integer", "enum": [0, -1, 1]}, "F": {"type": "integer", "enum": [1, 2], "x-enum-varnames": ["One", "Two"]}}),
        );
        let names = |n: &str| {
            let Kind::IntEnum(vs) = &by_name(&s, n).kind else { panic!("{n}: an integer enum") };
            vs.iter().map(|(name, _)| name.key()).collect::<Vec<_>>()
        };
        assert_eq!(names("E"), ["N0", "Minus1", "N1"]);
        assert_eq!(names("F"), ["One", "Two"]);
    }

    /// Two aliases that contain each other cannot be `type` aliases: each becomes a newtype.
    #[test]
    fn aliases_in_a_cycle_become_newtypes() {
        let list = |of: &str| json!({"type": "array", "items": {"$ref": format!("#/components/schemas/{of}")}});
        let s = build(&json!({"A": list("B"), "B": list("A")}));
        assert_matches!(by_name(&s, "A").kind, Kind::Newtype(_));
        assert_matches!(by_name(&s, "B").kind, Kind::Newtype(_));
    }

    /// An `enum` without `type` has the type of each of its values. A list or an object among them is one too: the
    /// `enum` is not typed as its first scalar, which would refuse the list. Its type is a union of one constant per
    /// value.
    #[test]
    fn an_untyped_enum_has_the_type_of_each_value() {
        let o = json!({"enum": [6, "foo", [], true, {"foo": 12}, 7, null]});
        let (types, nullable) = type_set(o.as_object().unwrap());
        assert_eq!(types, ["integer", "string", "array", "boolean", "object"]);
        assert!(nullable);
        let s = build(
            &json!({"Mixed": {"type": "object", "properties": {"v": {"enum": [6, "foo", [], true, {"foo": 12}]}}}}),
        );
        let Kind::Struct(mixed) = &by_name(&s, "Mixed").kind else { panic!("a struct") };
        let Ty::Named(field) = &mixed.fields[0].ty else { panic!("{:?}", mixed.fields[0].ty) };
        let Kind::Union(union) = &s.get(*field).kind else { panic!("a union") };
        let unit = |t: &Ty| matches!(t, Ty::Named(id) if matches!(s.get(*id).kind, Kind::Const(_)));
        let consts = union.variants.iter().filter(|variant| unit(&variant.ty));
        assert_eq!(consts.count(), 5);
    }

    #[test]
    fn a_component_pointing_at_a_bundled_schema_names_it() {
        let doc = Doc::new(json!({"openapi": "3.2.0",
            "components": {"schemas": {
                "Page": {"$ref": "#/x-ext/a1"},
                "Item": {"$ref": "#/x-ext/b2"},
                "Again": {"$ref": "#/x-ext/b2"},
                "Gadget": {"$ref": "#/x-ext/c3"},
                "Product": {"$ref": "#/x-ext/d4"}
            }},
            "x-ext": {
                "a1": {"type": "object", "properties": {"items": {"type": "array", "items": {"$ref": "#/x-ext/b2"}}}},
                "b2": {"type": "object", "properties": {"id": {"type": "string"}}},
                "c3": {"type": "object", "properties": {"kind": {"type": "string"}}},
                "d4": {"oneOf": [{"$ref": "#/x-ext/b2"}, {"$ref": "#/x-ext/c3"}], "discriminator": {"propertyName": "kind"}}
            }
        }));
        let mut d = Diagnostics::default();
        let mut b = Builder::new(&doc, &mut d, Reserved::default());
        b.components();
        let s = b.finish();
        let names: Vec<&Name> = s.defs.iter().map(|d| &d.name).collect();
        assert_eq!(names, ["Page", "Item", "Again", "Gadget", "Product"].map(Name::new).iter().collect::<Vec<_>>());
        let Kind::Struct(page) = &by_name(&s, "Page").kind else { panic!() };
        assert_eq!(page.fields[0].ty, Ty::Vec(Box::new(Ty::Named(TypeId(1)))));
        assert_matches!(by_name(&s, "Again").kind, Kind::Alias(Ty::Named(TypeId(1))));
        // The implicit discriminator mapping is the name of the component and not the pointer of the bundler.
        let Kind::Union(product) = &by_name(&s, "Product").kind else { panic!() };
        let variants: Vec<(Name, &[String])> =
            product.variants.iter().map(|v| (v.name.clone(), v.tags.as_slice())).collect();
        assert_eq!(
            variants,
            [(Name::new("Item"), &["Item".to_owned()][..]), (Name::new("Gadget"), &["Gadget".to_owned()][..])]
        );
    }

    /// A union of unions: each layer contains the two before it, so 2^40 paths reach the bottom. Each kind is found
    /// once, and `Top` tells a text from a number however deep the text is.
    #[test]
    fn a_union_of_unions_finds_each_kind_once() {
        let mut schemas = serde_json::Map::new();
        schemas.insert("U0".into(), json!({"type": "string", "minLength": 1}));
        schemas.insert("U1".into(), json!({"type": "string", "maxLength": 9}));
        for i in 2..40 {
            let r = |j: usize| json!({"$ref": format!("#/components/schemas/U{j}")});
            schemas.insert(format!("U{i}"), json!({"anyOf": [r(i - 1), r(i - 2)]}));
        }
        schemas.insert("Top".into(), json!({"anyOf": [{"$ref": "#/components/schemas/U39"}, {"type": "integer"}]}));
        let s = build(&Value::Object(schemas));
        let Kind::Union(top) = &by_name(&s, "Top").kind else { panic!("{:?}", by_name(&s, "Top").kind) };
        assert_eq!(top.mode, UnionMode::ByKind);
    }

    #[test]
    fn struct_fields_and_inline_types() {
        let s = build(&json!({"Product": {"type": "object", "required": ["id"], "properties": {
            "id": {"type": "integer", "format": "int64"},
            "kind": {"type": "string", "enum": ["book", "gadget"]},
            "nick": {"type": ["string", "null"]}
        }}}));
        let Kind::Struct(st) = &by_name(&s, "Product").kind else { panic!() };
        assert_eq!(st.fields[0].ty, Ty::I64);
        assert!(st.fields[0].required);
        assert_matches!(by_name(&s, "ProductKind").kind, Kind::StrEnum(_) | Kind::IntEnum(_));
        assert_eq!(st.fields[2].ty, Ty::Option(Box::new(Ty::String)));
    }

    #[test]
    fn discriminated_union() {
        let s = build(&json!({
            "Product": {"oneOf": [{"$ref": "#/components/schemas/Book"}, {"$ref": "#/components/schemas/Gadget"}],
                    "discriminator": {"propertyName": "kind", "mapping": {"d": "#/components/schemas/Book"}}},
            "Book": {"type": "object", "properties": {"kind": {"type": "string"}}},
            "Gadget": {"type": "object", "properties": {"kind": {"type": "string"}}}
        }));
        let Kind::Union(u) = &by_name(&s, "Product").kind else { panic!() };
        assert_eq!(u.mode, UnionMode::Tagged { property: "kind".into(), default: None });
        assert_eq!(u.variants[0].tags, ["d"]);
        assert_eq!(u.variants[1].tags, ["Gadget"]);
    }

    #[test]
    fn recursion_is_boxed() {
        let s = build(&json!({"Node": {"type": "object", "properties": {
            "next": {"$ref": "#/components/schemas/Node"},
            "children": {"type": "array", "items": {"$ref": "#/components/schemas/Node"}}
        }}}));
        let Kind::Struct(st) = &by_name(&s, "Node").kind else { panic!() };
        assert_matches!(st.fields[0].ty, Ty::Indirect(_));
        assert_matches!(st.fields[1].ty, Ty::Vec(_));
    }

    #[test]
    fn all_of_merges() {
        let s = build(&json!({
            "Base": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}},
            "Ext": {"allOf": [{"$ref": "#/components/schemas/Base"}, {"properties": {"x": {"type": "boolean"}}}]}
        }));
        let Kind::Struct(st) = &by_name(&s, "Ext").kind else { panic!() };
        assert_eq!(st.fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>(), ["id", "x"].map(Name::new));
        assert!(st.fields[0].required);
    }

    #[test]
    fn kinds_and_recursive_alias() {
        let s = build(&json!({
            "Id": {"type": ["string", "integer"]},
            "Tree": {"type": "array", "items": {"$ref": "#/components/schemas/Tree"}}
        }));
        let Kind::Union(u) = &by_name(&s, "Id").kind else { panic!() };
        assert_eq!(u.mode, UnionMode::ByKind);
        assert_matches!(by_name(&s, "Tree").kind, Kind::Newtype(_));
    }
}
