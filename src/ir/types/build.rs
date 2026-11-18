//! Reading schemas into the type graph: the `Builder` and the schema keywords it understands.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::cycles::break_cycles;
use super::rewrite::{FALLBACK, conditional_union, request_view};
use super::{
    Access, Checked, Constraints, EnumValue, Field, Format, JsonKind, Kind, Struct, Ty, TypeDef, TypeId, TypeSpace,
    Union, UnionMode, Variant, XmlHint,
};
use crate::diag::Diagnostics;
use crate::ir::names::{Name, Reserved, reserved, unique};
use crate::json;
use crate::json::{Doc, pointer};
use crate::json::{escape_token, join};

/// The longest array `minItems == maxItems` makes a fixed `[T; N]`: std implements the traits up to 32.
const MAX_FIXED_LEN: usize = 32;
/// `$ref` and alias chains followed before giving up: a chain may loop.
const MAX_ALIAS_HOPS: usize = 16;
/// Subschemas read for constraints or inlined for run-time checks before the rest is left unchecked.
const MAX_SCHEMA_DEPTH: usize = 8;

/// Where an inlining walk is: the dynamic scope that it entered, the pointers that it is expanding, the cycles that it
/// cut into `$defs`, whether it may cut them, and whether it met a `$dynamicRef`.
#[derive(Default)]
struct Walk {
    scope: Vec<String>,
    expanding: Vec<String>,
    cuts: Vec<String>,
    cut: bool,
    dynamic: bool,
}
/// Nested schemas typed before the rest becomes `serde_json::Value`.
const MAX_TYPE_DEPTH: usize = 48;
/// The most `prefixItems` a tuple takes: std implements the traits up to 12.
const MAX_TUPLE: usize = 12;
/// Aliases followed to find a type's JSON kind.
const MAX_KIND_DEPTH: usize = 32;

/// Keywords `runtime::check_schema` handles that typed checks cannot.
pub(crate) const UNTYPED_KEYWORDS: [&str; 14] = [
    "contains",
    "minContains",
    "maxContains",
    "patternProperties",
    "propertyNames",
    "dependentRequired",
    "dependentSchemas",
    "if",
    "then",
    "else",
    "not",
    "unevaluatedProperties",
    "unevaluatedItems",
    "dependencies",
];

impl Constraints {
    /// Returns the constraints of an array's items: none when it states none.
    #[must_use]
    pub(crate) fn item_constraints(&self) -> Self {
        self.items.as_deref().cloned().unwrap_or_default()
    }

    /// Is there anything to check?
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Keywords of one schema object without `$ref` and `allOf`.
    fn own(s: &Value) -> Self {
        let u = |k: &str| s.get(k).and_then(json::count);
        let n = |k: &str| s.get(k).and_then(Value::as_number).cloned();
        Self {
            min_length: u("minLength"),
            max_length: u("maxLength"),
            pattern: s.get("pattern").and_then(Value::as_str).map(String::from),
            minimum: n("minimum"),
            maximum: n("maximum"),
            exclusive_minimum: n("exclusiveMinimum"),
            exclusive_maximum: n("exclusiveMaximum"),
            multiple_of: n("multipleOf"),
            min_items: u("minItems"),
            max_items: u("maxItems"),
            unique_items: s.get("uniqueItems") == Some(&Value::Bool(true)),
            min_properties: u("minProperties"),
            max_properties: u("maxProperties"),
            decimal128: s.get("format").and_then(Value::as_str) == Some("decimal128")
                && s.as_object().is_some_and(|o| type_set(o).0.iter().any(|t| t == "number")),
            items: None,
            schema: None,
        }
    }

    /// Fills keywords this one lacks from `other` (an `allOf` member or a `$ref` target).
    fn merge(&mut self, other: Self) {
        macro_rules! take {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = other.$f; } )* };
        }
        take!(min_length, max_length, pattern, minimum, maximum, exclusive_minimum, exclusive_maximum, multiple_of);
        take!(min_items, max_items, min_properties, max_properties, items);
        self.unique_items |= other.unique_items;
        self.decimal128 |= other.decimal128;
        self.schema = match (self.schema.take(), other.schema) {
            (Some(a), Some(b)) => Some(serde_json::json!({ "allOf": [a, b] })),
            (a, b) => a.or(b),
        };
    }
}

/// Returns a property schema's `example` and `examples` (an array in JSON Schema).
pub(crate) fn property_examples(prop: &Value, ptr: &str) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> =
        prop.get("example").map(|v| (join(ptr, "example"), v.clone())).into_iter().collect();
    for (i, v) in json::items(prop.get("examples")) {
        out.push((join(&join(ptr, "examples"), &i.to_string()), v.clone()));
    }
    out
}

/// Returns the JSON kind of a literal value.
pub(crate) fn value_kind(v: &Value) -> JsonKind {
    match v {
        Value::Null => JsonKind::Null,
        Value::Bool(_) => JsonKind::Bool,
        Value::Number(n) if n.is_i64() || n.is_u64() => JsonKind::Integer,
        Value::Number(_) => JsonKind::Number,
        Value::String(_) => JsonKind::String,
        Value::Array(_) => JsonKind::Array,
        Value::Object(_) => JsonKind::Object,
    }
}

/// Returns `minItems == maxItems == N` for N from 1 to 32. It is an array of exactly N items.
pub(crate) fn fixed_len(o: &serde_json::Map<String, Value>) -> Option<usize> {
    let min = o.get("minItems").and_then(json::count)?;
    let n = usize::try_from(min).ok().filter(|n| (1..=MAX_FIXED_LEN).contains(n))?;
    (o.get("maxItems").and_then(Value::as_u64) == Some(min)).then_some(n)
}

/// Returns a string `format` with a type of its own.
pub(crate) fn string_format(format: &str) -> Option<Format> {
    Format::ALL.into_iter().find(|f| f.name() == format)
}

/// Returns the integer of a registered integer `format` (<https://spec.openapis.org/registry/format/>).
fn int_format(format: &str) -> Option<Ty> {
    Some(match format {
        "int8" => Ty::I8,
        "int16" => Ty::I16,
        "int32" => Ty::I32,
        "int64" | "double-int" => Ty::I64,
        "uint8" => Ty::U8,
        "uint16" => Ty::U16,
        "uint32" => Ty::U32,
        "uint64" | "uint" => Ty::U64,
        _ => return None,
    })
}

/// Returns a string, integer or number schema with keywords of its own for its type (`pattern`, `maxLength`,
/// `minimum`, `multipleOf`, ...) and no other typing (`enum`, `const`, a typed `format`, composition). Its type
/// holds only the values that they allow.
pub(crate) fn checked_scalar(o: &serde_json::Map<String, Value>) -> bool {
    let (types, _) = type_set(o);
    let mut non_null = types.iter().filter(|t| *t != "null");
    let (Some(t), None) = (non_null.next(), non_null.next()) else { return false };
    let shaped = ["oneOf", "anyOf", "allOf", "enum", "const", "if", "not", "contentEncoding", "contentMediaType"]
        .iter()
        .any(|k| o.contains_key(*k));
    if shaped {
        return false;
    }
    let format = o.get("format").and_then(Value::as_str);
    match t.as_str() {
        "string" => {
            let typed_format = format.is_some_and(|f| string_format(f).is_some() || matches!(f, "binary" | "byte"));
            !typed_format
                && (o.get("pattern").is_some_and(Value::is_string)
                    || ["minLength", "maxLength"].iter().any(|k| o.get(*k).and_then(json::count).is_some()))
        }
        "integer" | "number" => {
            ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf"]
                .iter()
                .any(|k| o.get(*k).is_some_and(Value::is_number))
                || (t == "number" && format == Some("decimal128"))
        }
        _ => false,
    }
}

/// Returns a `const` with a type of its own. That is a string, number or bool.
pub(crate) fn scalar_const(o: &serde_json::Map<String, Value>) -> Option<&Value> {
    let c = o.get("const").filter(|c| json::is_scalar(c))?;
    // Composition, conditions and object shapes keep their own typing.
    let shaped = ["oneOf", "anyOf", "allOf", "properties", "if", "not", "enum"].iter().any(|k| o.contains_key(*k));
    (!shaped).then_some(c)
}

/// Keywords that assert something about a value whatever its type.
const ASSERTIONS: [&str; 20] = [
    // Asserted (JSON Schema 2020-12 section 7.2.1 allows it): a value of another kind passes.
    "format",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "required",
    "enum",
    "const",
    "additionalProperties",
    "items",
    "prefixItems",
];

/// Returns whether a schema without `type` still asserts something: `{minimum: 1.1}`, `{enum: []}` or `false`. Its
/// value is `serde_json::Value`, whose type checks nothing. So the whole schema is checked at run time. A keyword such
/// as `minimum` applies to numbers only there, as JSON Schema says. `oneOf` and `anyOf` are left to `loose_union`: a
/// union of distinct types judges itself.
fn typeless_assertions(s: &Value) -> bool {
    match s {
        Value::Bool(b) => !b,
        Value::Object(o) => {
            !o.contains_key("type")
                && json::object(o, "properties").is_none_or(serde_json::Map::is_empty)
                && ASSERTIONS.iter().any(|k| o.contains_key(*k))
        }
        Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_) => false,
    }
}

/// Returns the pointer of `key` under the schema at `at`.
fn child_ptr(at: &str, key: &str) -> String {
    format!("{at}/{}", escape_token(key))
}

/// Does `s` or a subschema of it (not data such as `enum`) have a `$dynamicRef`?
fn holds_dynamic_ref(s: &Value) -> bool {
    match s {
        Value::Object(o) => {
            o.contains_key("$dynamicRef")
                || o.iter().any(|(k, v)| {
                    !matches!(k.as_str(), "enum" | "const" | "default" | "examples" | "example") && holds_dynamic_ref(v)
                })
        }
        Value::Array(a) => a.iter().any(holds_dynamic_ref),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

/// Returns the kind that object or array keywords imply on a schema without `type`: `object` for `properties`,
/// `additionalProperties` or `patternProperties`, `array` for `items` or `prefixItems`. `None` with a `type`, or when
/// a composition (`allOf`, `oneOf`, `anyOf`, `$ref`) or a value list (`enum`, `const`) decides the type instead.
fn inferred_kind(o: &serde_json::Map<String, Value>) -> Option<&'static str> {
    let decided = ["type", "allOf", "oneOf", "anyOf", "$ref", "enum", "const"];
    if decided.iter().any(|k| o.contains_key(*k)) {
        return None;
    }
    if ["properties", "additionalProperties", "patternProperties"].iter().any(|k| o.contains_key(*k)) {
        return Some("object");
    }
    ["items", "prefixItems"].iter().any(|k| o.contains_key(*k)).then_some("array")
}

/// Returns the `default` of a schema when it is a plain literal (string, number or bool) that the schema accepts. A
/// default is an annotation: one that breaks its own schema (`validate` warns) is not filled in. Filling it would make
/// a request that leaves the field out invalid.
pub(crate) fn literal_default(doc: &Doc, s: &Value) -> Option<Value> {
    let d = s.get("default").filter(|d| json::is_scalar(d))?;
    let mut own = s.clone();
    if let Some(o) = own.as_object_mut() {
        o.remove("default");
    }
    crate::validate::examples::satisfies(doc, &own, d).then(|| d.clone())
}

/// Returns a media type whose content is bytes and not text. It is not `text/*`, JSON or XML.
pub(crate) fn binary_media(media: &str) -> bool {
    let m = media.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    !(m.starts_with("text/") || m.ends_with("json") || m.ends_with("xml") || m.ends_with("+yaml"))
}

/// Returns `{type: string, contentMediaType: <json>, contentSchema: S}`: S carried as JSON text.
pub(crate) fn json_string_schema(o: &serde_json::Map<String, Value>) -> Option<&Value> {
    let media = o.get("contentMediaType")?.as_str()?;
    let json = media == "application/json" || media.ends_with("+json");
    (json && o.get("contentEncoding").is_none() && o.get("type").and_then(Value::as_str).is_none_or(|t| t == "string"))
        .then(|| o.get("contentSchema"))
        .flatten()
}

/// Returns the values of an `enum` that becomes an integer enum type. Every non-null value is an `i64`. The only `type`
/// is `integer` (and `null`).
pub(crate) fn int_enum_values(o: &serde_json::Map<String, Value>) -> Option<Vec<i64>> {
    let (types, _) = type_set(o);
    if !types.iter().all(|t| t == "integer" || t == "null") {
        return None;
    }
    let ints =
        o.get("enum")?.as_array()?.iter().filter(|v| !v.is_null()).map(Value::as_i64).collect::<Option<Vec<i64>>>()?;
    (!ints.is_empty()).then_some(ints)
}

/// Returns an integer enum. `x-enum-varnames` / `x-enumNames` (`openapi-generator` and `NSwag`)
/// name its variants when present. Otherwise they are `N1` and `Minus1`.
pub(crate) fn int_enum(o: &serde_json::Map<String, Value>, values: &[i64]) -> Kind {
    let names: Option<Vec<&str>> = ["x-enum-varnames", "x-enumNames", "x-enum-names"]
        .iter()
        .find_map(|k| o.get(*k).and_then(Value::as_array))
        .and_then(|a| a.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        .filter(|n| n.len() == values.len());
    let mut taken = reserved(&["Other"]);
    let variants = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let base = names.as_ref().map_or_else(
                || Name::new(&if *v < 0 { format!("Minus{}", v.unsigned_abs()) } else { format!("N{v}") }),
                |n| Name::new(n[i]),
            );
            (unique(base, &mut taken), *v)
        })
        .collect();
    Kind::IntEnum(variants)
}

/// Keywords that only annotate and never change the type.
pub(crate) const ANNOTATIONS: &[&str] = &[
    "title",
    "description",
    "summary",
    "default",
    "examples",
    "example",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$comment",
    "$schema",
    "$id",
    "$anchor",
    "$dynamicAnchor",
    "xml",
    "externalDocs",
    "format",
    "discriminator",
    "$defs",
    "definitions",
    "contentMediaType",
    "contentEncoding",
];

/// Builds a `TypeSpace` from schemas.
#[derive(Debug)]
pub(crate) struct Builder<'a> {
    /// The document being converted.
    pub doc: &'a Doc,
    /// Where problems are reported.
    pub diags: &'a mut Diagnostics,
    /// Schema resources and their dynamic anchors: `$dynamicRef` in run-time checks.
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
    /// The generated crate's dependencies (`-` as `_`): an object-form `x-rust-type` names one of them.
    pub crates: Vec<String>,
    /// The description was Swagger 2.0: an optional property reads `null` as absent ([`Field::null_as_absent`]).
    pub from_v20: bool,
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
        }
    }

    /// Registers and convert every `components.schemas` entry and keep their names.
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
        // `x-rust-type: crate::domain::Money` reuses your type. So does typify's object form `{crate, version, path}`
        // (Oxide's `SledUuid`) when the crate is a dependency: `generate` checks that. An invalid path is ignored.
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
        // not by the referenced schema; items take the item schema's name.
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

    fn constraints_at(&self, s: &Value, depth: usize, at: Option<&str>) -> Constraints {
        if depth > MAX_SCHEMA_DEPTH {
            return Constraints::default();
        }
        let mut c = Constraints::own(s);
        // `if`/`then` on one property's constants becomes a tagged union (`conditional_union`): its variants hold the
        // conditions. Only conditions that it cannot type stay a run-time check.
        let typed_conditional = s.as_object().is_some_and(|o| conditional_union(o).is_some());
        let conditional_key = |k: &str| typed_conditional && matches!(k, "if" | "then" | "else");
        let conditional_member = |m: &Value| {
            typed_conditional
                && m.as_object().is_some_and(|m| m.contains_key("if") && m.keys().all(|k| conditional_key(k)))
        };
        // Non-string `enum`/`const` have no type of their own either. An integer enum is the exception. The
        // `validate` of its type rejects `kind: 7` when only 1, 2 and 3 are allowed.
        let untyped = |v: &Value| !v.is_string() && !v.is_null();
        let untyped_values = |o: &serde_json::Map<String, Value>| {
            (o.get("enum").and_then(Value::as_array).is_some_and(|e| e.iter().any(untyped))
                && int_enum_values(o).is_none())
                || o.get("const").is_some_and(untyped)
        };
        // A tuple (`prefixItems`, maybe closed by `items: false`) has no Rust type that checks its length and items.
        // A `$dynamicRef` anywhere inside resolves in the dynamic scope from here: only the run-time check of the whole
        // schema follows it there.
        let tuple =
            s.get("prefixItems").is_some() || s.get("items") == Some(&Value::Bool(false)) || holds_dynamic_ref(s);
        // Keywords beside an implied shape hold for the `Other` variant too (`{minimum: 30, properties: ...}`). A
        // `$ref` with keywords beside it joins two schemas that one struct does not hold.
        let mixed = s.as_object().is_some_and(|o| {
            // Keywords that assert nothing about the value: annotations and the XML mapping.
            let quiet = |k: &str| {
                matches!(
                    k,
                    "$ref"
                        | "$id"
                        | "$schema"
                        | "$comment"
                        | "$defs"
                        | "description"
                        | "title"
                        | "xml"
                        | "example"
                        | "examples"
                        | "default"
                        | "deprecated"
                        | "readOnly"
                        | "writeOnly"
                        | "externalDocs"
                )
            };
            (inferred_kind(o).is_some()
                && ASSERTIONS
                    .iter()
                    .any(|k| !matches!(*k, "items" | "prefixItems" | "additionalProperties") && o.contains_key(*k)))
                || (o.contains_key("$ref") && o.keys().any(|k| !quiet(k)))
        });
        if typeless_assertions(s)
            || tuple
            || mixed
            || self.loose_union(s)
            || self.lossy_all_of(s)
            || matches!(s, Value::Object(o) if UNTYPED_KEYWORDS.iter().any(|k| o.contains_key(*k) && !conditional_key(k)) || untyped_values(o))
        {
            // The resource of the schema starts the dynamic scope that a `$dynamicRef` resolves in.
            let scope: Vec<String> = at.map_or_else(Vec::new, |p| vec![self.dynamic.resource_of(p)]);
            let mut schema = self.inlined(s, at, scope);
            request_view(&mut schema);
            c.schema = Some(schema);
        }
        for m in s.get("allOf").and_then(Value::as_array).into_iter().flatten().filter(|m| !conditional_member(m)) {
            c.merge(self.constraints_at(m, depth + 1, None));
        }
        // One hop: a target that has keywords beside its own `$ref` (3.1) keeps them. Following the whole chain
        // would skip them.
        if let Some(p) = s.get("$ref").and_then(Value::as_str).and_then(|r| self.doc.local_pointer(r))
            && let Some(target) = pointer(&self.doc.root, &p)
        {
            c.merge(self.constraints_at(target, depth + 1, Some(&p)));
        }
        // `anyOf: [{type: integer, maximum: 10}, {type: null}]` is typed as its one other branch. So that branch's
        // keywords are the value's.
        if let [branch] = only_non_null_branch(s).as_slice() {
            c.merge(self.constraints_at(branch, depth + 1, None));
        }
        if c.items.is_none()
            && let Some(items) = s.get("items")
        {
            let ic = self.constraints_at(items, depth + 1, None);
            if !ic.is_empty() {
                c.items = Some(Box::new(ic));
            }
        }
        c
    }

    /// Returns an `allOf` that one merged type cannot hold. The members merge into one struct (or the type they share)
    /// with one schema per property and one value per keyword. So two members that declare the same property or the
    /// same keyword (two `maximum`s, two `additionalProperties`), or whose `type`s share none, would lose one of them:
    /// the whole schema is then checked at run time. `required` adds up, one shared `type` is kept and each member's
    /// untyped keywords are checked with it. So the usual `allOf: [$ref: Base, {type: object, properties: ...}]` and an
    /// `allOf` of `if`/`then` pairs stay typed.
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
        let mut names = BTreeSet::new();
        // The schema's own keywords beside `allOf` merge in too.
        let mut own = o.clone();
        own.remove("allOf");
        for m in std::iter::once(Value::Object(own)).chain(members.iter().cloned()) {
            let Some(mo) = m.as_object() else { continue };
            for (k, v) in mo {
                // A member's untyped keywords (`if`, `not`) are checked whole with that member already.
                let lossless = matches!(k.as_str(), "type" | "required" | "allOf" | "$ref")
                    || UNTYPED_KEYWORDS.contains(&k.as_str())
                    || ANNOTATIONS.contains(&k.as_str())
                    || k.starts_with("x-");
                if k == "properties" {
                    let fresh = v.as_object().into_iter().flatten().all(|(n, _)| names.insert(n.clone()));
                    if !fresh {
                        return true;
                    }
                } else if !lossless && !keys.insert(k.clone()) {
                    return true;
                }
            }
        }
        members.iter().any(|m| m.get("type").is_some()) && self.narrowed_types(&members).is_none()
    }

    /// Returns a `oneOf` or `anyOf` that its union type cannot judge alone: a branch without one `type` of its own, two
    /// branches of one type (`oneOf` means exactly one) or keywords beside the union (`type: string` next to
    /// `anyOf`). Branches of distinct types (`integer` or `string`) are judged by their variant. A discriminator does
    /// not change this (3.2: it "MUST NOT change the validation outcome").
    fn loose_union(&self, s: &Value) -> bool {
        let Some(o) = s.as_object() else { return false };
        let Some(branches) = ["oneOf", "anyOf"].iter().find_map(|k| o.get(*k)?.as_array().filter(|a| !a.is_empty()))
        else {
            return false;
        };
        let quiet = [
            "oneOf",
            "anyOf",
            "description",
            "title",
            "example",
            "examples",
            "deprecated",
            "default",
            "nullable",
            "discriminator",
        ];
        if o.keys().any(|k| !quiet.contains(&k.as_str()) && !k.starts_with("x-")) {
            return true;
        }
        // Every branch fixes the discriminating property to its own value: the tag picks the one branch a value
        // can match, and that variant checks the rest.
        if let Some(prop) = o.get("discriminator").and_then(|d| d.get("propertyName")).and_then(Value::as_str)
            && branches.iter().all(|b| self.pins_tag(b, prop, 0))
        {
            return false;
        }
        let mut types = BTreeSet::new();
        for b in branches {
            let Some((b, _)) = self.deref(b) else { return true };
            let Some(t) = b.get("type").and_then(Value::as_str) else { return true };
            if !types.insert(t.to_owned()) {
                return true;
            }
        }
        false
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
        let o = s.as_object()?;
        if has_ref_only(o) {
            return None;
        }
        let (types, _) = type_set(o);
        let non_null: Vec<&str> = types.iter().map(String::as_str).filter(|t| *t != "null").collect();
        if let Some(c) = scalar_const(o) {
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
        // too (2020-12 section 7.6.1). One variant holds the shape, `Other` the rest.
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
            return None;
        }
        if non_null.len() > 1 {
            return Some(self.multi_type(o, &non_null, ptr, name));
        }
        let is_object = non_null.first() == Some(&"object") || (non_null.is_empty() && o.contains_key("properties"));
        if is_object && (json::object(o, "properties").is_some_and(|p| !p.is_empty()) || only_empty(o)) {
            return Some(Kind::Struct(self.object(o, ptr, name)));
        }
        None
    }

    /// Returns the type expression for a schema at `ptr`. `hint` names any inline type.
    pub(crate) fn ty(&mut self, s: &Value, ptr: &str, hint: &Name) -> Ty {
        let Some(o) = s.as_object() else { return Ty::Any };
        if self.depth > MAX_TYPE_DEPTH {
            self.diags.warning(ptr, "schema nesting too deep; using an untyped value");
            return Ty::Any;
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
            // `$ref` with sibling constraints. Treat it as allOf [ref, rest].
            let mut rest = o.clone();
            rest.remove("$ref");
            let merged = serde_json::json!({"allOf": [{"$ref": r}, Value::Object(rest)]});
            return self.ty_non_null(&merged, ptr, hint);
        }
        if let Some(r) = o.get("$dynamicRef").and_then(Value::as_str) {
            // Bookended and not bound by a generic: it resolves in the dynamic scope, somewhere a type cannot know.
            // The run-time check judges the value.
            if r.starts_with("#/") && self.bookend(r).1.is_some() {
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
                // `literal_default` keeps only a default that meets the keywords.
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
        if scalar_const(o).is_some() || checked_scalar(o) || inferred_kind(o).is_some() {
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
            Some("string") => string_format(format).map_or(Ty::String, Ty::Format),
            Some("null") => Ty::Unit,
            Some("array") => self.array(o, ptr, hint),
            Some("object") | None => {
                if let Some(c) = o.get("const") {
                    return const_ty(c);
                }
                // One `patternProperties` schema (and no other extra keys
                // allowed) types every value; the key pattern is still checked.
                let single_pattern = o.get("patternProperties").and_then(Value::as_object).filter(|p| p.len() == 1);
                if let Some((pat, schema)) = single_pattern.and_then(|p| p.iter().next())
                    && !o.contains_key("properties")
                    && o.get("additionalProperties").is_none_or(|a| *a == Value::Bool(false))
                {
                    let at = join(&join(ptr, "patternProperties"), pat);
                    return Ty::Map(Box::new(self.ty(schema, &at, &hint.and(&Name::new("Value")))));
                }
                match o.get("additionalProperties") {
                    Some(ap @ Value::Object(m)) if !m.is_empty() => Ty::Map(Box::new(self.ty(
                        ap,
                        &join(ptr, "additionalProperties"),
                        &hint.and(&Name::new("Value")),
                    ))),
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

    /// Returns `s` with its references inlined for run-time checks, from the dynamic scope `scope`.
    ///
    /// A reference back to a schema that the walk is inside is a cycle: it points into `$defs` at the root, which
    /// holds each such target once. So a recursive schema stays its own size, and the run-time check follows it as
    /// deep as the value goes. A `$dynamicRef` resolves by where evaluation came from: with one in reach, cycles are
    /// inlined to `MAX_SCHEMA_DEPTH` as before.
    fn inlined(&self, s: &Value, at: Option<&str>, scope: Vec<String>) -> Value {
        let expanding = at.map(|p| vec![p.to_owned()]).unwrap_or_default();
        let mut walk = Walk { scope: scope.clone(), expanding, cut: true, ..Walk::default() };
        let mut root = self.inline_scoped(s, 0, at, &mut walk);
        if walk.dynamic {
            return self.inline_scoped(s, 0, at, &mut Walk { scope, ..Walk::default() });
        }
        let mut defs = serde_json::Map::new();
        let mut i = 0;
        while let Some(p) = walk.cuts.get(i).cloned() {
            let target = pointer(&self.doc.root, &p).cloned().unwrap_or(Value::Bool(true));
            walk.expanding = vec![p.clone()];
            walk.scope = vec![self.dynamic.resource_of(&p)];
            // The key is the pointer itself: the reference escapes it as one token, and a reader unescapes it.
            defs.insert(p.clone(), self.inline_scoped(&target, 1, Some(&p), &mut walk));
            i += 1;
        }
        if let (Value::Object(o), false) = (&mut root, defs.is_empty()) {
            o.insert("$defs".to_owned(), Value::Object(defs));
        }
        root
    }

    /// Returns `s` with every `$ref` replaced by its target for runtime checks. `depth` counts the references followed:
    /// past 8 a reference accepts anything, so a cycle ends. So does an unresolvable one. Nesting without references is
    /// copied whole: a document is finite, and a value deep inside it (`type: [string, null]`) stays what it says. `s`
    /// with every `$ref` replaced by its target for run-time checks, in a dynamic scope: `scope` is the schema
    /// resources that evaluation entered on its way here, outermost first, and `at` the pointer of `s` when it is
    /// known. A bookended `$dynamicRef` resolves to the outermost resource in scope with its `$dynamicAnchor` (2020-12
    /// core section 8.2.3.2). Others act as `$ref`.
    fn inline_scoped(&self, s: &Value, depth: usize, at: Option<&str>, walk: &mut Walk) -> Value {
        let child = |key: &str| at.map(|p| child_ptr(p, key));
        match s {
            Value::Object(o) if o.contains_key("$ref") || o.contains_key("$dynamicRef") => {
                let reference = o.get("$ref").or_else(|| o.get("$dynamicRef"));
                let statically = reference.and_then(Value::as_str).and_then(|r| self.doc.local_pointer(r));
                // A bookended `$dynamicRef`: the outermost resource in scope with its anchor, else where it points.
                let dynamically = o
                    .get("$dynamicRef")
                    .filter(|_| o.get("$ref").is_none())
                    .and_then(Value::as_str)
                    .and_then(|r| self.bookend(r).1)
                    .and_then(|name| {
                        walk.scope.iter().find_map(|r| self.dynamic.anchors.get(&(r.clone(), name.clone())))
                    })
                    .cloned();
                walk.dynamic |= o.contains_key("$dynamicRef");
                let target_ptr = dynamically.or(statically);
                let mut target = match target_ptr.as_deref().filter(|_| depth < MAX_SCHEMA_DEPTH) {
                    // A reference back to a schema that this walk is inside: a cycle. Expanding it again doubles the
                    // fragment at each turn. It becomes a reference into `$defs`, which holds the target once.
                    Some(p) if walk.cut && walk.expanding.iter().any(|e| e == p) => {
                        if !walk.cuts.iter().any(|c| c == p) {
                            walk.cuts.push(p.to_owned());
                        }
                        serde_json::json!({ "$ref": format!("#/$defs/{}", escape_token(p)) })
                    }
                    Some(p) => match pointer(&self.doc.root, p) {
                        Some(t) => {
                            // Following a reference enters the resource of its target, and leaves it after.
                            let resource = self.dynamic.resource_of(p);
                            let entered = walk.scope.last() != Some(&resource);
                            if entered {
                                walk.scope.push(resource);
                            }
                            walk.expanding.push(p.to_owned());
                            let inlined = self.inline_scoped(t, depth + 1, Some(p), walk);
                            walk.expanding.pop();
                            if entered {
                                walk.scope.pop();
                            }
                            inlined
                        }
                        None => Value::Bool(true),
                    },
                    None => Value::Bool(true),
                };
                // Siblings of `$ref` (3.1) apply too. The target joins them as an `allOf` member of the same object:
                // what it evaluates stays visible to their `unevaluated*` (2020-12 core 11.2).
                let mut rest = o.clone();
                for k in ["$ref", "$dynamicRef"] {
                    rest.remove(k);
                }
                if !rest.is_empty()
                    && let Value::Object(mut rest) = self.inline_object(&rest, depth, at, walk)
                {
                    match rest.get_mut("allOf") {
                        Some(Value::Array(members)) => members.push(target),
                        _ => {
                            rest.insert("allOf".to_owned(), Value::Array(vec![target]));
                        }
                    }
                    target = Value::Object(rest);
                }
                target
            }
            Value::Object(o) => self.inline_object(o, depth, at, walk),
            Value::Array(a) => Value::Array(
                a.iter()
                    .enumerate()
                    .map(|(i, v)| self.inline_scoped(v, depth, child(&i.to_string()).as_deref(), walk))
                    .collect(),
            ),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => s.clone(),
        }
    }

    /// Returns the keywords of a schema object inlined. A schema with an `$id` is a resource: evaluation enters it.
    fn inline_object(
        &self,
        o: &serde_json::Map<String, Value>,
        depth: usize,
        at: Option<&str>,
        walk: &mut Walk,
    ) -> Value {
        let entered = at.filter(|_| o.contains_key("$id")).map(String::from).filter(|p| walk.scope.last() != Some(p));
        if let Some(p) = &entered {
            walk.scope.push(p.clone());
        }
        let out = Value::Object(
            o.iter()
                // Annotations and examples do not validate anything.
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "description" | "title" | "example" | "examples" | "xml" | "externalDocs" | "discriminator"
                    )
                })
                .map(|(k, v)| {
                    let v = match (k.as_str(), v) {
                        // Data: `{"$ref": ...}` in an `enum` is a value to compare and not a reference.
                        ("enum" | "const" | "default", _) => v.clone(),
                        // Maps of schemas: a property may be called `$ref`.
                        (
                            "properties" | "patternProperties" | "$defs" | "definitions" | "dependentSchemas",
                            Value::Object(m),
                        ) => Value::Object(
                            m.iter()
                                .map(|(n, x)| {
                                    let p = at.map(|p| child_ptr(&child_ptr(p, k), n));
                                    (n.clone(), self.inline_scoped(x, depth, p.as_deref(), walk))
                                })
                                .collect(),
                        ),
                        _ => self.inline_scoped(v, depth, at.map(|p| child_ptr(p, k)).as_deref(), walk),
                    };
                    (k.clone(), v)
                })
                .collect(),
        );
        if entered.is_some() {
            walk.scope.pop();
        }
        out
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

    fn object(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, name: &Name) -> Struct {
        let required: BTreeSet<&str> =
            o.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let mut taken = BTreeSet::new();
        let mut fields = vec![];
        if let Some(Value::Object(props)) = o.get("properties") {
            for (wire, prop) in props {
                let fptr = join(&join(ptr, "properties"), wire);
                let ty = self.ty(prop, &fptr, &name.and(&Name::new(wire)));
                let read_only = prop.get("readOnly") == Some(&Value::Bool(true));
                let write_only = prop.get("writeOnly") == Some(&Value::Bool(true));
                let read_write = read_only || write_only;
                // A literal default makes an optional field always present. Whether `ty` can hold it
                // is known once every component is filled: `finish` drops the ones it cannot.
                let default =
                    literal_default(self.doc, prop).filter(|_| !required.contains(wire.as_str()) && !read_write);
                fields.push(Field {
                    name: unique(Name::new(wire), &mut taken),
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
        // Unknown keys of an open object land in `extra`. So a proxy or a
        // read-modify-write keeps them.
        let additional = match o.get("additionalProperties") {
            // A name that `patternProperties` matches is not additional, so its value need not fit
            // `additionalProperties`: the map holds any value and the run-time check judges each by its name.
            Some(Value::Object(m)) if !m.is_empty() && o.contains_key("patternProperties") => Some(Ty::Any),
            Some(ap @ Value::Object(m)) if !m.is_empty() => {
                Some(self.ty(ap, &join(ptr, "additionalProperties"), &name.and(&Name::new("Value"))))
            }
            Some(Value::Bool(true) | Value::Object(_)) => Some(Ty::Any),
            None if self.keep_unknown => Some(Ty::Any),
            _ => None,
        };
        // `additionalProperties: false` (JSON Schema 2020-12 section 10.3.2.3). An
        // undeclared property fails validation. `patternProperties` and
        // `unevaluatedProperties` admit more names. `runtime::check_schema` judges
        // those. A readOnly / writeOnly property is declared even where its
        // split type leaves it out. So such structs stay open too.
        // `patternProperties` and `unevaluatedProperties` judge the names a struct does not declare. The run-time
        // check sees the value as it re-serializes, so those names must survive decoding.
        let additional = additional.or_else(|| {
            (o.contains_key("patternProperties") || o.contains_key("unevaluatedProperties")).then_some(Ty::Any)
        });
        let closed = o.get("additionalProperties") == Some(&Value::Bool(false))
            && !o.contains_key("patternProperties")
            && !o.contains_key("unevaluatedProperties")
            && fields.iter().all(|f| f.access == Access::ReadWrite);
        Struct { fields, additional, closed }
    }

    /// Returns `allOf: [{oneOf: [A, B]}, C]` means `oneOf: [allOf: [A, C], allOf: [B, C]]`: each value is one branch
    /// and also the rest (Garage's `NodeRoleChange` is a role or a removal, each with the node `id`). Merging the
    /// members into one struct would lose the union. The union may sit in a nested `allOf` member (Meilisearch's
    /// `SearchResultWithIndex` is its `SearchResult` and an index uid). Each branch is titled after its `$ref` or its
    /// one property, so the variant keeps that name. A discriminator stays: [`branch_ref`] finds a branch's target
    /// inside its `allOf`.
    fn distributed_all_of(&self, o: &serde_json::Map<String, Value>, _ptr: &str) -> Option<Value> {
        let members = self.flat_members(o.get("allOf").and_then(Value::as_array)?, 0);
        let (i, key, union) = members.iter().enumerate().find_map(|(i, m)| {
            let (r, _) = self.deref(m)?;
            let key = ["oneOf", "anyOf"].into_iter().find(|k| r.get(*k).is_some_and(Value::is_array))?;
            Some((i, key, r))
        })?;
        let mut rest: Vec<Value> =
            members.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, m)| m.clone()).collect();
        // The schema's own object keywords beside `allOf` hold for every branch too.
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

    /// Returns the members of an `allOf` with the members of a nested pure `allOf` lifted out. Such a member's own
    /// object keywords stay as a member of their own. A member that is a union stays whole.
    fn flat_members(&self, members: &[Value], depth: usize) -> Vec<Value> {
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

    /// Returns `{properties: {name}, required: [name], oneOf: [A, B]}` means `allOf: [{properties, required},
    /// {oneOf}]`: the properties beside the union hold for each branch (Svix's `IngestSourceIn` has a `name` whatever
    /// its `type`). Typing the union alone would lose them.
    fn sibling_union(&self, o: &serde_json::Map<String, Value>, ptr: &str) -> Option<Value> {
        let key = ["oneOf", "anyOf"].into_iter().find(|k| o.get(*k).is_some_and(Value::is_array))?;
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
    fn all_of(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, name: &Name) -> Option<Kind> {
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
                    props.insert(k.clone(), v.clone());
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
                props.insert(k.clone(), v.clone());
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
            // Kept for `object`: the struct holds undeclared names for the run-time check to judge.
            merged.insert("unevaluatedProperties".into(), Value::Bool(true));
        }
        Some(Kind::Struct(self.object(&merged, ptr, name)))
    }

    /// Returns the `type` names that every `allOf` member allows (members without `type` allow all). This applies when
    /// a member declares one and some type is left.
    fn narrowed_types(&self, members: &[Value]) -> Option<Vec<String>> {
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
    fn union(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, name: &Name) -> Option<Kind> {
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
        let mut named = vec![];
        for (i, (b, bptr)) in variants.iter().enumerate() {
            // `null` is the `Other` fallback of the union and not the payload of a variant.
            // An inline branch with a `title` names its type after it. Others are numbered.
            let branch =
                b.get("title").and_then(Value::as_str).map_or_else(|| format!("Variant{}", i + 1), str::to_owned);
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
            let vname = vname.filter(|n| !n.is("other")).unwrap_or_else(|| Name::new(&format!("Variant{}", i + 1)));
            let constraints = if matches!(ty, Ty::Named(_)) { Constraints::default() } else { self.constraints(b) };
            named.push((vname, ty, constraints));
        }
        // Several array branches are told apart by what they hold (`NumberArray`, `DocumentArray`) and not as
        // `Array`, `Array2`. One alone stays `Array`.
        if named.iter().filter(|(n, ..)| n.is("array")).count() > 1 {
            for (n, ty, _) in &mut named {
                if n.is("array") {
                    *n = Name::new(&self.array_name(ty));
                }
            }
        }
        let mut taken = BTreeSet::new();
        let mut out = vec![];
        for (vname, ty, constraints) in named {
            out.push(Variant { name: unique(vname, &mut taken), ty, tags: vec![], constraints });
        }
        // A branch that any value passes (`{$comment: ...}` or the RFC 2557 example of 3.2) is what `Other` already
        // holds. Next to typed branches it would be a second catch-all that cannot be told apart.
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
        Some(Kind::Union(Union { variants: out, mode, open, rest: false }))
    }

    /// Returns what an array branch holds as the name of its variant: `NumberArray` for numbers, `NumberArrayArray` for
    /// arrays of them and `DocumentArray` for a named type.
    fn array_name(&self, ty: &Ty) -> String {
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
    fn name_by_tag(&mut self, out: &mut [Variant], parent: &Name, fallback: Option<usize>, hints: &[Option<String>]) {
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

    fn union_mode(
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

    fn flatten_all_of(&self, mut m: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
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

    fn space_kind(&self, ty: &Ty) -> JsonKind {
        kind_in(&self.defs, ty, 0)
    }

    /// Returns `type: [string, integer]` to union by JSON kind.
    fn multi_type(&mut self, o: &serde_json::Map<String, Value>, types: &[&str], ptr: &str, name: &Name) -> Kind {
        let mut variants = vec![];
        for t in types {
            let mut single = o.clone();
            single.insert("type".into(), Value::String(t.to_string()));
            let ty = self.ty(&Value::Object(single), &format!("{ptr}/type/{t}"), &name.and(&Name::new(t)));
            // The keywords sit on the schema with the `type` list. Whatever holds that schema checks them.
            variants.push(Variant { name: Name::new(t), ty, tags: vec![], constraints: Constraints::default() });
        }
        // integer + number overlap: integer must be tried first (ByKind handles it).
        let kinds: BTreeSet<JsonKind> = variants.iter().map(|v| self.space_kind(&v.ty)).collect();
        let mode = if kinds.len() == variants.len() { UnionMode::ByKind } else { UnionMode::Untagged };
        Kind::Union(Union { variants, mode, open: false, rest: false })
    }

    /// Returns finish. Resolve placeholders, drop defaults that the type of their field cannot hold and break cycles
    /// with `Box`.
    pub(crate) fn finish(mut self) -> TypeSpace {
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
        break_cycles(&mut space);
        space
    }
}

/// Picks the mode of a union while it is built. A variant whose type is still being built then has no kind yet, so the
/// union tries its variants in order. Once every type is built, such a union whose variants have distinct kinds picks
/// by kind, as one built later would. Repeated until nothing changes: a union's kind can depend on another union.
fn by_kind_when_known(defs: &mut [TypeDef]) {
    loop {
        let settle: Vec<usize> = (0..defs.len())
            .filter(|&i| {
                let Kind::Union(u) = &defs[i].kind else { return false };
                if u.mode != UnionMode::Untagged {
                    return false;
                }
                let kinds: Vec<JsonKind> = u.variants.iter().map(|v| kind_in(defs, &v.ty, 0)).collect();
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

/// Most string values an inline branch's variant is named after (`OptimizingOrDone`). More read as a list.
const MAX_HINT_VALUES: usize = 3;

/// Returns the `$ref` of a union branch, also when distributing an `allOf` wrapped it (`{allOf: [{$ref: X}, rest]}`).
fn branch_ref(b: &Value) -> Option<&str> {
    b.get("$ref").and_then(Value::as_str).or_else(|| {
        let first = b.get("allOf")?.as_array()?.first()?;
        first.get("$ref").and_then(Value::as_str)
    })
}

/// Returns the object keywords of a schema that apply beside its `allOf` or union: `type`, `properties`, `required` and
/// `additionalProperties`. `None` when it has no properties or requirements of its own.
fn own_object_keywords(o: Option<&serde_json::Map<String, Value>>) -> Option<Value> {
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
fn branch_hint(b: &Value) -> Option<String> {
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
fn only_non_null_branch(s: &Value) -> Vec<&Value> {
    let key = if s.get("oneOf").is_some() { "oneOf" } else { "anyOf" };
    s.get(key).and_then(Value::as_array).into_iter().flatten().filter(|b| !is_null_branch(b)).collect()
}

/// Returns a `oneOf` or `anyOf` branch that only `null` passes. It is the `Other` of the union and no variant of its
/// own.
fn is_null_branch(b: &Value) -> bool {
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
/// (a conditional union's fallback branch takes every other value).
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
    let vs = values
        .iter()
        .filter(|w| seen_wire.insert(**w))
        .map(|w| EnumValue { name: unique(Name::new(w), &mut taken), wire: w.to_string(), doc: None })
        .collect();
    Kind::StrEnum(vs)
}

/// Returns a schema that the description names: a component schema, or the schema of a component parameter or header.
fn named_by_description(ptr: &str) -> bool {
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

/// Returns the `type` names and whether `null` is allowed (also via `enum` / `const`).
/// The value of a `type` keyword for these names. That is one string or a list.
fn types_value(types: &[String]) -> Value {
    match types {
        [one] => Value::String(one.clone()),
        many => Value::Array(many.iter().cloned().map(Value::String).collect()),
    }
}

/// Returns an object that admits no property at all: `additionalProperties: false` with nothing declared and no
/// `patternProperties` or `unevaluatedProperties` to admit a name. Only `{}` is valid (Oxide's
/// `SubnetCreateDefaults`). It is an empty closed struct and not a map of anything.
fn only_empty(o: &serde_json::Map<String, Value>) -> bool {
    o.get("additionalProperties") == Some(&Value::Bool(false))
        && json::object(o, "properties").is_none_or(serde_json::Map::is_empty)
        && !o.contains_key("patternProperties")
        && !o.contains_key("unevaluatedProperties")
}

pub(crate) fn type_set(o: &serde_json::Map<String, Value>) -> (Vec<String>, bool) {
    let mut types: Vec<String> = match o.get("type") {
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(ts)) => json::strings(ts),
        _ => vec![],
    };
    if types.is_empty() {
        if o.contains_key("properties") || o.contains_key("additionalProperties") {
            types.push("object".into());
        } else if o.contains_key("items") || o.contains_key("prefixItems") {
            types.push("array".into());
        } else if let Some(Value::Array(values)) = o.get("enum") {
            let mut ks: Vec<String> = values
                .iter()
                .filter_map(|v| match v {
                    Value::String(_) => Some("string"),
                    Value::Bool(_) => Some("boolean"),
                    Value::Number(n) if n.is_f64() => Some("number"),
                    Value::Number(_) => Some("integer"),
                    Value::Null | Value::Array(_) | Value::Object(_) => None,
                })
                .map(String::from)
                .collect();
            ks.dedup();
            types = ks;
        }
    }
    let nullable = types.iter().any(|t| t == "null")
        || o.get("enum").and_then(Value::as_array).is_some_and(|e| e.iter().any(Value::is_null))
        || o.get("oneOf")
            .or_else(|| o.get("anyOf"))
            .and_then(Value::as_array)
            .is_some_and(|bs| bs.iter().any(|b| b.get("type").and_then(Value::as_str) == Some("null")));
    (types, nullable)
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

pub(crate) fn kind_in(defs: &[TypeDef], ty: &Ty, depth: usize) -> JsonKind {
    if depth > MAX_KIND_DEPTH {
        return JsonKind::Any;
    }
    match ty {
        Ty::Bool => JsonKind::Bool,
        Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => JsonKind::Integer,
        Ty::F32 | Ty::F64 | Ty::Decimal => JsonKind::Number,
        Ty::String | Ty::Bytes | Ty::Binary | Ty::Format(_) => JsonKind::String,
        Ty::Any => JsonKind::Any,
        Ty::Unit => JsonKind::Null,
        Ty::Vec(_) | Ty::Array(..) | Ty::Tuple(_) => JsonKind::Array,
        Ty::Map(_) => JsonKind::Object,
        Ty::Option(t) | Ty::Indirect(t) => kind_in(defs, t, depth + 1),
        Ty::Named(id) => match &defs[id.0].kind {
            Kind::Struct(_) => JsonKind::Object,
            Kind::StrEnum(_) => JsonKind::String,
            Kind::Checked(c) => kind_in(defs, &c.base, depth + 1),
            Kind::IntEnum(_) => JsonKind::Integer,
            Kind::Const(v) => value_kind(v),
            // A closed union whose variants share a kind has that kind. An open one takes any value.
            Kind::Union(u) if !u.open => {
                let mut kinds = u.variants.iter().map(|v| kind_in(defs, &v.ty, depth + 1));
                let first = kinds.next().unwrap_or(JsonKind::Any);
                if kinds.all(|k| k == first) { first } else { JsonKind::Any }
            }
            Kind::Union(_) | Kind::Pending => JsonKind::Any,
            Kind::Alias(t) | Kind::Newtype(t) => kind_in(defs, t, depth + 1),
        },
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

    /// A schema that refers back to itself is checked whole with the cycle in `$defs`. Its reference must reach the
    /// definition: a reader unescapes the token of `#/$defs/~1components~1schemas~1Rec` to the key.
    #[test]
    fn a_cycle_in_a_checked_schema_resolves_to_its_definition() {
        let rec = json!({
            "type": "array",
            "prefixItems": [{"type": "string"}, {"oneOf": [{"$ref": "#/components/schemas/Rec"}, {"type": "null"}]}],
            "items": false,
            "minItems": 2,
        });
        let doc = Doc::new(json!({"openapi": "3.2.0", "components": {"schemas": {"Rec": rec}}}));
        let mut d = Diagnostics::default();
        let b = Builder::new(&doc, &mut d, Reserved::default());
        let schema = b.constraints_at_ptr(&rec, "/components/schemas/Rec").schema.expect("checked whole");
        let defs = schema.get("$defs").and_then(Value::as_object).expect("the cycle is in `$defs`");
        assert_eq!(defs.keys().collect::<Vec<_>>(), ["/components/schemas/Rec"]);
        let mut out = vec![];
        runtime::__private::check_schema(&mut out, &runtime::InstancePath::Root, &json!(["a", null]), &schema);
        assert!(out.is_empty(), "`null` matches only the second branch: {out:?}");
        runtime::__private::check_schema(&mut out, &runtime::InstancePath::Root, &json!(["a", ["b", 1]]), &schema);
        assert!(!out.is_empty(), "the nested pair needs a string or `null` second");
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
