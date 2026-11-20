//! Reading schemas into the type graph: the `Builder` and the schema keywords it understands.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::cycles::break_cycles;
use super::rewrite::{FALLBACK, conditional_union};
use super::{
    Access, Checked, Constraints, EnumValue, Evaluated, EvaluatedItems, Field, Format, JsonKind, Kind, Leftover, Rest,
    Struct, Ty, TypeDef, TypeId, TypeSpace, Union, UnionMode, Variant, XmlHint,
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

/// Nested schemas typed before the rest becomes `serde_json::Value`.
const MAX_TYPE_DEPTH: usize = 48;
/// The most `prefixItems` a tuple takes: std implements the traits up to 12.
const MAX_TUPLE: usize = 12;

/// Keywords that a typed check holds only in some shapes: in any other, generation stops and says so.
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

    /// Returns the checks that judge the value as JSON (`not`, `dependentSchemas`, `if`, a counted `oneOf`, keywords
    /// beside a union, a tuple), and no other.
    pub(crate) fn json_checks(&self) -> Self {
        Self {
            not: self.not.clone(),
            dependent_schemas: self.dependent_schemas.clone(),
            conditions: self.conditions.clone(),
            one_of: self.one_of.clone(),
            any_of: self.any_of.clone(),
            pattern_properties: self.pattern_properties.clone(),
            closed_to: self.closed_to.clone(),
            also: self.also.clone(),
            enum_values: self.enum_values.clone(),
            const_value: self.const_value.clone(),
            unevaluated: self.unevaluated.clone(),
            unevaluated_items: self.unevaluated_items.clone(),
            names_schema: self.names_schema,
            json_contains: self.json_contains,
            json_dependents: self.json_dependents.clone(),
            tuple: self.tuple.clone(),
            ..Self::default()
        }
    }

    /// Returns these constraints without the checks that judge the value as JSON ([`Self::json_checks`]).
    pub(crate) fn without_json(&self) -> Self {
        Self {
            not: vec![],
            dependent_schemas: vec![],
            conditions: vec![],
            one_of: vec![],
            any_of: vec![],
            pattern_properties: vec![],
            closed_to: None,
            also: vec![],
            enum_values: None,
            const_value: None,
            unevaluated: None,
            unevaluated_items: None,
            names_schema: None,
            json_contains: None,
            json_dependents: vec![],
            tuple: None,
            ..self.clone()
        }
    }

    /// Is there anything to check?
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Keywords of one schema object without `$ref` and `allOf`.
    fn own(s: &Value) -> Self {
        let u = |k: &str| count_of(s, k);
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
            never: *s == Value::Bool(false),
            min_properties: u("minProperties"),
            max_properties: u("maxProperties"),
            decimal128: s.get("format").and_then(Value::as_str) == Some("decimal128")
                && s.as_object().is_some_and(|o| type_set(o).0.iter().any(|t| t == "number")),
            items: None,
            property_names: None,
            dependent_required: vec![],
            // A struct holds the names that it declares. A map has none: its names are checked.
            required: if json::object(s, "properties").is_none_or(serde_json::Map::is_empty) {
                required_names(s).map(str::to_owned).collect()
            } else {
                vec![]
            },
            format: None,
            not: vec![],
            dependent_schemas: vec![],
            enum_values: None,
            const_value: None,
            unevaluated: None,
            unevaluated_items: None,
            names_schema: None,
            conditions: vec![],
            contains: None,
            json_contains: None,
            json_dependents: vec![],
            one_of: vec![],
            any_of: vec![],
            pattern_properties: vec![],
            closed_to: None,
            tuple: None,
            also: vec![],
        }
    }

    /// Fills keywords this one lacks from `other` (an `allOf` member or a `$ref` target).
    fn merge(&mut self, other: Self) {
        macro_rules! take {
            ($($f:ident),*) => { $( if self.$f.is_none() { self.$f = other.$f; } )* };
        }
        take!(min_length, max_length, pattern, minimum, maximum, exclusive_minimum, exclusive_maximum, multiple_of);
        take!(enum_values, const_value, unevaluated, unevaluated_items, names_schema, json_contains);
        take!(min_items, max_items, min_properties, max_properties, items, property_names, contains, tuple);
        self.unique_items |= other.unique_items;
        self.never |= other.never;
        self.decimal128 |= other.decimal128;
        self.dependent_required.extend(other.dependent_required);
        self.json_dependents.extend(other.json_dependents);
        self.required.extend(other.required);
        if self.format.is_none() {
            self.format = other.format;
        }
        self.not.extend(other.not);
        self.dependent_schemas.extend(other.dependent_schemas);
        self.conditions.extend(other.conditions);
        if self.any_of.is_empty() {
            self.any_of = other.any_of;
        }
        if self.one_of.is_empty() {
            self.one_of = other.one_of;
        }
        self.pattern_properties.extend(other.pattern_properties);
        if self.closed_to.is_none() {
            self.closed_to = other.closed_to;
        }
        self.also.extend(other.also);
    }
}

/// Puts the keywords of an object that generated code checks into `c`: `dependentRequired` over properties that the
/// struct declares is a check of its fields, and `propertyNames` that checks only text is a check of each name.
/// Returns whether each was typed, so the run-time check of the whole schema can leave it out.
fn typed_object_keywords(s: &Value, c: &mut Constraints) -> (bool, bool, bool) {
    let dependents = typed_dependents(s);
    let names = typed_property_names(s);
    let pattern = typed_pattern_map(s).filter(|_| names.is_none());
    let typed = (dependents.is_some(), names.is_some(), pattern.is_some());
    // Without `type` the value may be any, or the union of an implied shape: the JSON object is checked.
    match dependents {
        Some((d, undeclared)) if undeclared || s.get("type").is_none() => c.json_dependents = d,
        Some((d, _)) => c.dependent_required = d,
        None => {}
    }
    c.property_names =
        names.or_else(|| pattern.map(|p| Box::new(Constraints { pattern: Some(p), ..Constraints::default() })));
    typed
}

/// Does `s` become the tagged union of its `if`/`then` on one property's constants (`conditional_union`)? Only a
/// schema of `type: object` without a union of its own does: on any other the conditions are checks of their schemas.
fn typed_conditional(s: &Value) -> bool {
    s.as_object().is_some_and(|o| {
        is_object(o) && !o.contains_key("oneOf") && !o.contains_key("anyOf") && conditional_union(o).is_some()
    })
}

/// Are the items of `s` checked one by one ([`Constraints::tuple`])? A tuple (`prefixItems`, maybe closed by
/// `items: false`) is, and so is `items` without `type`: the union of the implied shape holds the list, the check each
/// item.
fn checked_items(s: &Value) -> bool {
    let typeless_items = s.get("type").is_none() && s.get("items").is_some_and(Value::is_object);
    s.get("prefixItems").is_some() || s.get("items") == Some(&Value::Bool(false)) || typeless_items
}

/// Returns why no generated check holds `s`: its keywords that no type holds, and the shapes that none does. Empty
/// when every keyword is typed. `shapes` says whether a `$dynamicRef` varies and whether keywords beside an implied
/// shape are of no one kind ([`mixed`]).
fn untyped(s: &Value, scoped: bool, shapes: [bool; 2], conditional_key: &dyn Fn(&str) -> bool) -> Vec<&'static str> {
    let reasons = [
        ("keywords without a `type` of their own", typeless_assertions(s) && !scoped),
        ("a `$dynamicRef` whose anchor another resource that reaches it declares", shapes[0]),
        ("`enum` or `const` beside an implied shape", shapes[1]),
    ];
    (s.as_object().into_iter())
        .flat_map(|o| UNTYPED_KEYWORDS.iter().filter(|k| o.contains_key(**k) && !conditional_key(k)))
        .copied()
        .chain(reasons.iter().filter(|(_, on)| *on).map(|(why, _)| *why))
        .collect()
}

/// Whether the schema `o` takes an object: its `type` names `object`.
fn is_object(o: &serde_json::Map<String, Value>) -> bool {
    type_set(o).0.iter().any(|t| t == "object")
}

/// Whether the schema `o` may take a value of kind `kind`: its `type` names it, or it has no `type`. Without one, its
/// value is a `serde_json::Value`, whose checks apply a keyword to values of its kind only.
fn may_be(o: &serde_json::Map<String, Value>, kind: &str) -> bool {
    !o.contains_key("type") || type_set(o).0.iter().any(|t| t == kind)
}

/// Returns the pattern of `s` when it is a map that generated code holds whole: `type: object` with one
/// `patternProperties` entry, no `properties` and `additionalProperties: false`. Its type is a map of the pattern's
/// schema ([`Builder::scalar`]), so each value decodes as that schema, and every name must match the pattern.
fn typed_pattern_map(s: &Value) -> Option<String> {
    let o = s.as_object().filter(|o| is_object(o))?;
    let patterns = o.get("patternProperties")?.as_object().filter(|p| p.len() == 1)?;
    let closed = o.get("additionalProperties") == Some(&Value::Bool(false));
    (closed && !o.contains_key("properties")).then(|| patterns.keys().next().cloned()).flatten()
}

/// Returns `patternProperties` of `s` when generated code checks it: on an object whose `additionalProperties` is
/// absent, `true` or `false`. A schema there would type the names that patterns match too. `None` leaves it to the
/// run-time check of the whole schema.
fn typed_patterns(s: &Value) -> Option<&serde_json::Map<String, Value>> {
    let o = s.as_object().filter(|o| may_be(o, "object"))?;
    let extra = o.get("additionalProperties");
    o.get("patternProperties")?.as_object().filter(|_| extra.is_none_or(|e| e.is_boolean() || e.is_object()))
}

/// Returns `propertyNames` of `s` when generated code checks it: a schema of `pattern`, `minLength` and `maxLength`
/// only, maybe with `type: string`, on a schema of `type: object`. A name is a string, so that type asserts nothing.
/// `None` leaves any other schema to the run-time check of the whole schema.
fn typed_property_names(s: &Value) -> Option<Box<Constraints>> {
    // Only an object has names to check: a schema without `type: object` may take any value, which no map holds.
    s.as_object().filter(|o| is_object(o))?;
    let names = s.get("propertyNames")?.as_object()?;
    json::text_only(names).then(|| Box::new(Constraints::own(&Value::Object(names.clone()))))
}

/// `dependentRequired`: each name that triggers it, with the names that it requires then.
type Dependents = Vec<(String, Vec<String>)>;

/// Returns `dependentRequired` of `s` when generated code checks it: on a struct, every name is a property that `s`
/// declares, whose absence shows ([`absence_shows`]); or one that it does not declare, which its struct keeps, and
/// whether one is (it is checked on the JSON object then). A map or a value without `type` keeps every name. `None`
/// leaves it to the run-time check of the whole schema.
fn typed_dependents(s: &Value) -> Option<(Dependents, bool)> {
    let deps = s.get("dependentRequired")?.as_object()?;
    // Without properties the value is a map or a `serde_json::Value`, which keeps every name it was sent.
    let props = json::object(s, "properties").filter(|p| !p.is_empty());
    let seen = absence_shows(s);
    if props.is_some() && seen.is_none() || !s.as_object().is_some_and(|o| may_be(o, "object")) {
        return None;
    }
    // A name that the struct does not declare is kept by it, and checked on the JSON object.
    let declared = |n: &str| props.is_some_and(|p| p.contains_key(n));
    let mut out = vec![];
    let mut undeclared = false;
    for (name, needs) in deps {
        let needs: Vec<String> =
            needs.as_array()?.iter().map(|n| n.as_str().map(String::from)).collect::<Option<_>>()?;
        for n in std::iter::once(name).chain(&needs) {
            if !declared(n) {
                undeclared |= props.is_some();
            } else if !seen.as_ref().is_some_and(|f| f(n)) {
                return None;
            }
        }
        out.push((name.clone(), needs));
    }
    Some((out, undeclared))
}

/// Returns whether the absence of a property of `s` shows in its struct: it is declared. A name that a dependent
/// keyword reads fills in no default ([`dependent_names`]). `None` when `s` declares no properties.
fn absence_shows(s: &Value) -> Option<impl Fn(&str) -> bool + '_> {
    let props = s.get("properties")?.as_object()?;
    // A name that `dependentRequired` or `dependentSchemas` reads keeps its absence: its default is not filled in.
    Some(move |n: &str| props.contains_key(n))
}

/// Returns the names that `dependentRequired` or `dependentSchemas` of `o` reads: each that triggers one, and each
/// that one requires. Their struct fields fill in no default, so that their absence shows.
fn dependent_names(o: &serde_json::Map<String, Value>) -> BTreeSet<&str> {
    let required = json::object(o, "dependentRequired").into_iter().flatten();
    let needs = required.clone().flat_map(|(_, v)| json::strs(v.as_array().map_or(&[][..], Vec::as_slice)));
    let triggers = required.chain(json::object(o, "dependentSchemas").into_iter().flatten()).map(|(k, _)| k.as_str());
    triggers.chain(needs).collect()
}

/// Returns `dependentSchemas` of `s` when generated code checks it: each name is a property whose absence shows
/// ([`absence_shows`]), or one that `s` does not declare, which its struct keeps. `None` leaves it to the run-time
/// check of the whole schema.
fn typed_dependent_schemas(s: &Value) -> Option<&serde_json::Map<String, Value>> {
    let deps = s.get("dependentSchemas")?.as_object()?;
    let props = s.get("properties").and_then(Value::as_object);
    let seen = absence_shows(s);
    let shows = |n: &str| !props.is_some_and(|p| p.contains_key(n)) || seen.as_ref().is_some_and(|f| f(n));
    deps.keys().all(|n| shows(n)).then_some(deps)
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
    // `enum` with one value says what `const` does: `enum: [true]` is a flag that may only be `true`.
    let one_value = || match o.get("enum")?.as_array()?.as_slice() {
        [v] if !o.contains_key("const") => Some(v),
        _ => None,
    };
    let c = o.get("const").or_else(one_value).filter(|c| json::is_scalar(c) && !c.is_null())?;
    // Composition, conditions and object shapes keep their own typing. A `const` beside an `enum` is both.
    let shaped = ["oneOf", "anyOf", "allOf", "properties", "if", "not"].iter().any(|k| o.contains_key(*k))
        || (o.contains_key("const") && o.contains_key("enum"));
    (!shaped).then_some(c)
}

/// Does `o` list values that no Rust type holds: an empty `enum`, one of other than strings or integers, or a `const`
/// of other than a string? A scalar constant is a unit type that decodes its one value, so it holds its keyword.
fn untyped_values(o: &serde_json::Map<String, Value>) -> bool {
    let untyped = |v: &Value| !v.is_string() && !v.is_null();
    scalar_const(o).is_none()
        && ((o.get("enum").and_then(Value::as_array).is_some_and(|e| e.is_empty() || e.iter().any(untyped))
            && int_enum_values(o).is_none())
            || o.get("const").is_some_and(untyped))
}

/// Adds property `name` of schema `schema` to `props`. A property that another `allOf` member declared already must
/// meet both schemas: it becomes their `allOf`, which its field types and checks as one. The same schema twice is one.
fn both(props: &mut serde_json::Map<String, Value>, name: &str, schema: &Value) {
    match props.get_mut(name) {
        Some(existing) if existing == schema || implied(schema, existing) => {}
        Some(existing) if implied(existing, schema) => *existing = schema.clone(),
        Some(existing) => *existing = serde_json::json!({"allOf": [existing.take(), schema.clone()]}),
        None => {
            props.insert(name.to_owned(), schema.clone());
        }
    }
}

/// Does `narrow` imply all that `wide` says? `wide` says only its `type` (and annotations), and every value of
/// `narrow` is of one of those types: its own `type`s are among them, or its `const` or `enum` values are.
/// `{const: image}` implies `{type: string}`, and `{type: string, maxLength: 5}` implies `{type: [string, null]}`.
fn implied(wide: &Value, narrow: &Value) -> bool {
    let (Some(w), Some(n)) = (wide.as_object(), narrow.as_object()) else { return false };
    if w.keys().any(|k| k != "type" && !ANNOTATIONS.contains(&k.as_str())) {
        return false;
    }
    let (wide_types, _) = type_set(w);
    if wide_types.is_empty() {
        return false;
    }
    let of_type = |v: &Value| {
        wide_types.iter().any(|t| match t.as_str() {
            "string" => v.is_string(),
            "integer" => v.is_i64() || v.is_u64(),
            "number" => v.is_number(),
            "boolean" => v.is_boolean(),
            "null" => v.is_null(),
            _ => false,
        })
    };
    let (narrow_types, _) = type_set(n);
    (!narrow_types.is_empty() && narrow_types.iter().all(|t| wide_types.contains(t)))
        || n.get("const").is_some_and(of_type)
        || n.get("enum").and_then(Value::as_array).is_some_and(|e| !e.is_empty() && e.iter().all(of_type))
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
    // `false` refuses every value: [`Constraints::never`].
    let Value::Object(o) = s else { return false };
    // The union of an implied shape holds its own keywords (an `Array` or `Object` variant and `Other`), and checks
    // those of one kind beside them by kind.
    let shape = inferred_kind(o).is_some();
    let held = |k: &str| {
        matches!(k, "items" | "prefixItems" | "additionalProperties")
            || (KIND_SCOPED.contains(&k) && !matches!(k, "enum" | "const"))
    };
    !o.contains_key("type")
        && json::object(o, "properties").is_none_or(serde_json::Map::is_empty)
        && ASSERTIONS.iter().any(|k| o.contains_key(*k) && !(shape && held(k)))
}

/// Keywords that apply to one kind of value only and pass any other (JSON Schema 2020-12 section 7.2.1): checked on a
/// `serde_json::Value` by its kind.
const KIND_SCOPED: [&str; 17] = [
    "minLength",
    "maxLength",
    "pattern",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minItems",
    "maxItems",
    "minProperties",
    "maxProperties",
    "uniqueItems",
    "required",
    "format",
    "enum",
    "const",
];

/// Does a schema without `type` assert only [`KIND_SCOPED`] keywords? Its value is a `serde_json::Value` whose checks
/// apply each keyword to values of its kind, with no schema checked whole.
fn kind_scoped(s: &Value) -> bool {
    s.as_object().is_some_and(|o| {
        // A tuple and `additionalProperties` are checked on the JSON array or object (`tuple`, `closed_to`).
        let on_json = |k: &str| matches!(k, "items" | "prefixItems" | "additionalProperties");
        ASSERTIONS.iter().filter(|k| o.contains_key(**k)).all(|k| KIND_SCOPED.contains(k) || on_json(k))
            && json::object(o, "properties").is_none_or(serde_json::Map::is_empty)
    })
}

/// Keywords beside an implied shape hold for the `Other` variant too (`{enum: [...], properties: ...}`). Those of one
/// kind are checked on the value of the union by its kind (`implied_checks`); any other is not held. A `$ref` with
/// keywords beside it is the `allOf` of the two, which the merged type and its members hold.
fn mixed(s: &Value) -> bool {
    s.as_object().is_some_and(|o| {
        inferred_kind(o).is_some()
            && ASSERTIONS.iter().any(|k| {
                let by_kind = KIND_SCOPED.contains(k) && !matches!(*k, "enum" | "const");
                !matches!(*k, "items" | "prefixItems" | "additionalProperties") && !by_kind && o.contains_key(*k)
            })
    })
}

/// Returns the count that keyword `k` of `s` sets (`minLength`, `maxContains`, ...).
fn count_of(s: &Value, k: &str) -> Option<u64> {
    s.get(k).and_then(json::count)
}

/// Does schema `s` take every value: `true`, or an object of annotations only (`{}`, `{$comment: ...}`)?
fn asserts_nothing(s: &Value) -> bool {
    match s {
        Value::Bool(b) => *b,
        Value::Object(o) => o.keys().all(|k| {
            matches!(k.as_str(), "$comment" | "description" | "title" | "examples" | "deprecated")
                || k.starts_with("x-")
        }),
        Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_) => false,
    }
}

/// Puts the keywords of a schema without `type` that a typed value's type would hold into `c`: `required` for an object
/// and `format` for a string, each checked by its kind.
fn scoped_keywords(s: &Value, c: &mut Constraints) {
    c.required = required_names(s).map(str::to_owned).collect();
    c.format = s.get("format").and_then(Value::as_str).map(str::to_owned);
}

/// Returns the names in `required` of `s`.
fn required_names(s: &Value) -> impl Iterator<Item = &str> {
    json::strs(s.get("required").and_then(Value::as_array).map_or(&[][..], Vec::as_slice))
}

/// Returns the keywords beside a `oneOf` or `anyOf` as a schema of their own: they hold for whichever branch matches,
/// so the value also decodes as its type. `None` without such keywords.
fn beside_union(s: &Value) -> Option<Value> {
    let o = s.as_object()?;
    let union = union_keyword(o).is_some();
    let quiet = |k: &str| {
        matches!(k, "oneOf" | "anyOf" | "discriminator" | "description" | "title" | "example" | "examples")
            || matches!(k, "deprecated" | "default" | "nullable")
            // They see what the branches evaluate: the schema with the union checks them.
            || matches!(k, "unevaluatedProperties" | "unevaluatedItems")
            || k.starts_with("x-")
    };
    let beside: serde_json::Map<String, Value> =
        o.iter().filter(|(k, _)| !quiet(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
    (union && !beside.is_empty()).then_some(Value::Object(beside))
}

/// Returns the keyword of the union that `o` holds: `oneOf` or `anyOf` with a list of branches.
fn union_keyword(o: &serde_json::Map<String, Value>) -> Option<&'static str> {
    ["oneOf", "anyOf"].into_iter().find(|k| o.get(*k).is_some_and(Value::is_array))
}

/// Puts the `enum` and `const` of `s` into `c` when no Rust type holds their values: the value is compared as JSON.
fn untyped_value_checks(s: &Value, c: &mut Constraints) {
    if let Some(o) = s.as_object().filter(|o| untyped_values(o)) {
        c.enum_values = o.get("enum").and_then(Value::as_array).cloned();
        c.const_value = o.get("const").cloned();
    }
}

/// What [`Builder::evaluation`] works out: the names ([`Evaluated`]) or the items ([`EvaluatedItems`]) that a schema
/// evaluates.
trait Evaluation: Default {
    /// What schema object `o` evaluates by its own keywords. `index` gives the type of a schema that it decodes as.
    fn own(o: &serde_json::Map<String, Value>, depth: usize, index: &dyn Fn(&Value) -> usize) -> Self;
    /// Adds what `other` evaluates.
    fn merge(&mut self, other: Self);
    /// What it evaluates only when the value decodes as a schema, or when it does not.
    fn branches(&mut self) -> &mut Vec<(usize, bool, Self)>;
}

impl Evaluation for Evaluated {
    fn own(o: &serde_json::Map<String, Value>, depth: usize, _: &dyn Fn(&Value) -> usize) -> Self {
        let keys = |k: &str| o.get(k).and_then(Value::as_object).into_iter().flat_map(serde_json::Map::keys).cloned();
        Self {
            names: keys("properties").collect(),
            patterns: keys("patternProperties").collect(),
            // `unevaluatedProperties` further in evaluates every name that its own schema leaves.
            all: o.contains_key("additionalProperties") || (depth > 0 && o.contains_key("unevaluatedProperties")),
            branches: vec![],
        }
    }

    fn merge(&mut self, other: Self) {
        self.names.extend(other.names);
        self.patterns.extend(other.patterns);
        self.all |= other.all;
        self.branches.extend(other.branches);
    }

    fn branches(&mut self) -> &mut Vec<(usize, bool, Self)> {
        &mut self.branches
    }
}

impl Evaluation for EvaluatedItems {
    fn own(o: &serde_json::Map<String, Value>, depth: usize, index: &dyn Fn(&Value) -> usize) -> Self {
        Self {
            prefix: o.get("prefixItems").and_then(Value::as_array).map_or(0, Vec::len),
            all: o.contains_key("items") || (depth > 0 && o.contains_key("unevaluatedItems")),
            contains: o.get("contains").map(index).into_iter().collect(),
            branches: vec![],
        }
    }

    fn merge(&mut self, other: Self) {
        self.prefix = self.prefix.max(other.prefix);
        self.all |= other.all;
        self.contains.extend(other.contains);
        self.branches.extend(other.branches);
    }

    fn branches(&mut self) -> &mut Vec<(usize, bool, Self)> {
        &mut self.branches
    }
}

/// Collects each reference under one of `keys` (`$ref`, `$dynamicRef`) of `s` and its subschemas (not data such as
/// `enum`) into `out`.
fn references<'s>(s: &'s Value, keys: &[&str], out: &mut Vec<&'s str>) {
    match s {
        Value::Object(o) => {
            out.extend(keys.iter().filter_map(|k| o.get(*k).and_then(Value::as_str)));
            for (k, v) in o {
                if !matches!(k.as_str(), "enum" | "const" | "default" | "examples" | "example") {
                    references(v, keys, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| references(x, keys, out)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
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
    /// The generated crate's dependencies (`-` as `_`): an object-form `x-rust-type` names one of them.
    pub crates: Vec<String>,
    /// The description was Swagger 2.0: an optional property reads `null` as absent ([`Field::null_as_absent`]).
    pub from_v20: bool,
    /// The `$ref` targets whose keywords are being read: one met again is a cycle, whose keywords its own type holds.
    reading: RefCell<Vec<String>>,
    /// The schemas that typed checks decode the value as (`not`, `dependentSchemas`), by index, with the pointer of
    /// each when it has one. `finish` builds a type for each.
    subschemas: RefCell<Vec<(Value, Option<String>)>>,
    /// The index of each queued subschema by its pointer and its text: queuing one looks it up in one step.
    subschema_index: RefCell<BTreeMap<(Option<String>, String), usize>>,
    /// The JSON kind of each type once it is final ([`Kinds`]).
    kinds: RefCell<Vec<Option<JsonKind>>>,
    /// Schemas that no generated check holds, by pointer, with what makes them so. `finish` reports each as an error.
    unchecked: RefCell<BTreeMap<String, String>>,
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
        // `x-rust-type: crate::domain::Money` reuses your type. So does typify's object form `{crate, version, path}`
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

    /// Puts `patternProperties` of `s` into `c`: each pattern with the type of its schema, and what `additionalProperties`
    /// takes of the other names.
    fn queue_patterns(
        &self,
        s: &Value,
        patterns: Option<&serde_json::Map<String, Value>>,
        at: Option<&str>,
        c: &mut Constraints,
    ) {
        // Without patterns and without `type`, the union of the implied shape holds the object, the check the other
        // names. With a `type`, the struct or map holds them, and a map of one pattern its names.
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
    /// other names that no pattern matches take.
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
    /// take. `None` for `unevaluatedProperties: true`, which takes every name.
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
    /// items take. `None` for `unevaluatedItems: true`, which takes every item.
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

    /// Returns what `s` evaluates ([`Evaluated`] of names, [`EvaluatedItems`] of items). With `queue`, the schema of each
    /// branch is queued for a type; without, nothing is. `None` when that is not known before the value is: a
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

    /// Queues `not`, and `if` with its `then` and `else` unless a tagged union holds them, as subschemas of `c`.
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
    /// one merged type cannot hold.
    fn queue_also(&self, s: &Value, at: Option<&str>, c: &mut Constraints) {
        if let Some(beside) = beside_union(s) {
            c.also.push(self.queue_subschema(&beside, None));
        }
        // Members that one merged type cannot hold: the value also decodes as each of them.
        if self.lossy_all_of(s) {
            let at = at.map(|p| join(p, "allOf"));
            for (i, m) in json::items(s.get("allOf")) {
                c.also.push(self.queue_subschema(m, at.as_deref().map(|a| join(a, &i.to_string()))));
            }
        }
    }

    fn constraints_at(&self, s: &Value, depth: usize, at: Option<&str>) -> Constraints {
        // Keywords further down belong to the named types on the way, which hold them. Only a schema checked whole
        // loses what it cuts (`inline_scoped`).
        if depth > MAX_SCHEMA_DEPTH {
            return Constraints::default();
        }
        let mut c = Constraints::own(s);
        let (dependents, names, pattern_map) = typed_object_keywords(s, &mut c);
        // What the names that nothing evaluated take is known before the value is.
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
        let patterns = (!pattern_map).then(|| typed_patterns(s)).flatten();
        // `if`/`then` on one property's constants becomes a tagged union (`conditional_union`): its variants hold the
        // conditions. Only conditions that it cannot type stay a run-time check.
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
                // On a schema whose `type` takes no array, `contains` asserts nothing.
                || matches!(k, "contains" | "minContains" | "maxContains")
                || (patterns.is_some() && k == "patternProperties")
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
            // No generated check holds them: generation stops with the keywords that it cannot check.
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
        // A target that is being read already is a cycle: its type holds its keywords where it recurs.
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
        // Beside `prefixItems`, `items` holds for the rest only: the tuple checks it.
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

    /// Returns an `allOf` that one merged type cannot hold. The members merge into one struct (or the type they share)
    /// with one value per keyword. So two members that set the same keyword (two `maximum`s, two
    /// `additionalProperties`), or whose `type`s share none, would lose one of them: the whole schema is then checked
    /// at run time. A property that two members declare is the `allOf` of its schemas, which its field holds. `required` adds up, one shared `type` is kept and each member's
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
                // that two members declare merges into the `allOf` of its schemas: its field holds both.
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
        // A branch that asserts nothing takes any value: an `anyOf` with one holds for every value, which its variant of
        // any value decodes.
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
        // value as it re-serializes, so the objects it holds keep the names that they do not declare.
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
                // checked. An open map holds any value under a name that the pattern does not match.
                let single_pattern = o.get("patternProperties").and_then(Value::as_object).filter(|p| p.len() == 1);
                if let Some((pat, schema)) = single_pattern.and_then(|p| p.iter().next())
                    && !o.contains_key("properties")
                    && o.get("additionalProperties") == Some(&Value::Bool(false))
                {
                    let at = join(&join(ptr, "patternProperties"), pat);
                    return Ty::Map(Box::new(self.ty(schema, &at, &hint.and(&Name::new("Value")))));
                }
                // Beside `patternProperties`, `additionalProperties` holds for the names that no pattern matches: the
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

    fn object(&mut self, o: &serde_json::Map<String, Value>, ptr: &str, name: &Name) -> Struct {
        let required: BTreeSet<&str> =
            o.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        let dependent = dependent_names(o);
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
                let default = literal_default(self.doc, prop)
                    .filter(|_| !required.contains(wire.as_str()) && !read_write && !dependent.contains(wire.as_str()));
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
            // Beside `patternProperties`, the schema holds only for names that no pattern matches: the check judges them.
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
        // A dependent keyword that reads a name the struct does not declare looks for that name. With
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
        Some(Kind::Union(Union { variants: out, mode, open, rest: false, exclusive: key == "oneOf" }))
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
        Kinds::remembering(&self.defs, &self.kinds).of(ty)
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
        Kind::Union(Union { variants, mode, open: false, rest: false, exclusive: false })
    }

    /// Returns finish. Resolve placeholders, drop defaults that the type of their field cannot hold and break cycles
    /// with `Box`.
    pub(crate) fn finish(mut self) -> TypeSpace {
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

/// Picks the mode of a union while it is built. A variant whose type is still being built then has no kind yet, so the
/// union tries its variants in order. Once every type is built, such a union whose variants have distinct kinds picks
/// by kind, as one built later would. Repeated until nothing changes: a union's kind can depend on another union.
fn by_kind_when_known(defs: &mut [TypeDef]) {
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
/// `patternProperties` or `unevaluatedProperties` to admit a name. Only `{}` is valid. It is an empty closed struct
/// and not a map of anything.
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
            // Each value's own type, once, in the order of the values. `null` is `nullable` below.
            for v in values {
                let t = match v {
                    Value::String(_) => "string",
                    Value::Bool(_) => "boolean",
                    Value::Number(n) if n.is_f64() => "number",
                    Value::Number(_) => "integer",
                    Value::Array(_) => "array",
                    Value::Object(_) => "object",
                    Value::Null => continue,
                };
                if !types.iter().any(|x| x == t) {
                    types.push(t.to_owned());
                }
            }
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
enum Found {
    /// Met again while it is found, the type is on a cycle and takes any value.
    Finding,
    Kind(JsonKind),
}

impl<'d> Kinds<'d> {
    pub(crate) fn new(defs: &'d [TypeDef]) -> Self {
        Self { defs, found: BTreeMap::new(), known: None, unsettled: false }
    }

    /// [`Kinds::new`] that reads and keeps the final kinds in `known`.
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
            // A closed union whose variants share a kind has that kind. An open one takes any value.
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

    /// An `enum` without `type` has the type of each of its values. A list or an object among them is one too: the
    /// `enum` is not typed as its first scalar, which would refuse the list.
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
        assert_eq!(mixed.fields[0].ty, Ty::Any, "{:?}", mixed.fields[0].ty);
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

    /// A union of unions: each layer holds the two before it, so 2^40 paths reach the bottom. Each kind is found once,
    /// and `Top` tells a text from a number however deep the text is.
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
