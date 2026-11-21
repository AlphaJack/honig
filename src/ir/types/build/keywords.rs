//! The schema keywords that the `Builder` reads: which constraints a schema holds and of which kind.

use std::collections::BTreeSet;

use serde_json::Value;

use super::super::rewrite::conditional_union;
use super::super::{Constraints, Evaluated, EvaluatedItems, Format, JsonKind, Kind, Ty};
use crate::ir::names::{Name, reserved, unique_sorted};
use crate::json;
use crate::json::join;

/// The longest array `minItems == maxItems` makes a fixed `[T; N]`: std implements the traits up to 32.
pub(super) const MAX_FIXED_LEN: usize = 32;
/// `$ref` and alias chains followed before giving up: a chain may loop.
pub(super) const MAX_ALIAS_HOPS: usize = 16;
/// Subschemas read for constraints or inlined for run-time checks before the rest is left unchecked.
pub(super) const MAX_SCHEMA_DEPTH: usize = 8;

/// Nested schemas typed in one go: a deeper one becomes a named type filled later from no depth, so the stack stays
/// bounded and nothing goes untyped.
pub(super) const MAX_TYPE_DEPTH: usize = 48;
/// The most `prefixItems` a tuple takes: std implements the traits up to 12.
pub(super) const MAX_TUPLE: usize = 12;

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
    pub(super) fn own(s: &Value) -> Self {
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
    pub(super) fn merge(&mut self, other: Self) {
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
pub(super) fn typed_object_keywords(s: &Value, c: &mut Constraints) -> (bool, bool, bool) {
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
pub(super) fn typed_conditional(s: &Value) -> bool {
    s.as_object().is_some_and(|o| {
        is_object(o) && !o.contains_key("oneOf") && !o.contains_key("anyOf") && conditional_union(o).is_some()
    })
}

/// Are the items of `s` checked one by one ([`Constraints::tuple`])? A tuple (`prefixItems`, maybe closed by
/// `items: false`) is, and so is `items` without `type`: the union of the implied shape holds the list, the check each
/// item.
pub(super) fn checked_items(s: &Value) -> bool {
    let typeless_items = s.get("type").is_none() && s.get("items").is_some_and(Value::is_object);
    s.get("prefixItems").is_some() || s.get("items") == Some(&Value::Bool(false)) || typeless_items
}

/// Returns why no generated check holds `s`: its keywords that no type holds, and the shapes that none does. Empty
/// when every keyword is typed. `shapes` says whether a `$dynamicRef` varies and whether keywords beside an implied
/// shape are of no one kind ([`mixed`]).
pub(super) fn untyped(
    s: &Value,
    scoped: bool,
    shapes: [bool; 2],
    conditional_key: &dyn Fn(&str) -> bool,
) -> Vec<&'static str> {
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

/// Whether the schema `o` takes an object: its `type` lists `object`.
pub(super) fn is_object(o: &serde_json::Map<String, Value>) -> bool {
    type_set(o).0.iter().any(|t| t == "object")
}

/// Whether the schema `o` may take a value of kind `kind`: its `type` lists it, or it has no `type`. Without one, its
/// value is a `serde_json::Value`, whose checks apply a keyword to values of its kind only.
pub(super) fn may_be(o: &serde_json::Map<String, Value>, kind: &str) -> bool {
    !o.contains_key("type") || type_set(o).0.iter().any(|t| t == kind)
}

/// Returns the pattern of `s` when it is a map that generated code holds whole: `type: object` with one
/// `patternProperties` entry, no `properties` and `additionalProperties: false`. Its type is a map of the pattern's
/// schema ([`super::Builder::scalar`]), so each value decodes as that schema, and every name must match the pattern.
pub(super) fn typed_pattern_map(s: &Value) -> Option<String> {
    let o = s.as_object().filter(|o| is_object(o))?;
    let patterns = o.get("patternProperties")?.as_object().filter(|p| p.len() == 1)?;
    let closed = o.get("additionalProperties") == Some(&Value::Bool(false));
    (closed && !o.contains_key("properties")).then(|| patterns.keys().next().cloned()).flatten()
}

/// Returns `patternProperties` of `s` when its struct holds them ([`crate::ir::types::Struct::patterns`]): an object, with properties of
/// its own or none, and `additionalProperties` absent, a bool or a schema. Every pattern compiles, so the
/// generator can tell which declared names it matches.
pub(super) fn struct_patterns(s: &Value) -> Option<&serde_json::Map<String, Value>> {
    let o = s.as_object().filter(|o| is_object(o))?;
    // A closed map of one pattern is a map of that pattern's type already ([`typed_pattern_map`]).
    if typed_pattern_map(s).is_some() {
        return None;
    }
    let typed_extra = matches!(o.get("additionalProperties"), None | Some(Value::Bool(_) | Value::Object(_)));
    let patterns = o.get("patternProperties")?.as_object().filter(|p| !p.is_empty())?;
    (typed_extra && patterns.keys().all(|p| regex(p).is_some())).then_some(patterns)
}

/// Returns `pattern` compiled as JSON Schema reads it: ECMA-262 with Unicode semantics, or without them for a pattern
/// that only compiles so, as the run-time check does.
pub(super) fn regex(pattern: &str) -> Option<regress::Regex> {
    regress::Regex::with_flags(pattern, "u").or_else(|_| regress::Regex::new(pattern)).ok()
}

/// Returns `patternProperties` of `s` when generated code checks it: on an object whose `additionalProperties` is
/// absent, `true` or `false`. A schema there would type the names that patterns match too. `None` leaves it to the
/// run-time check of the whole schema.
pub(super) fn typed_patterns(s: &Value) -> Option<&serde_json::Map<String, Value>> {
    let o = s.as_object().filter(|o| may_be(o, "object"))?;
    let extra = o.get("additionalProperties");
    o.get("patternProperties")?.as_object().filter(|_| extra.is_none_or(|e| e.is_boolean() || e.is_object()))
}

/// Returns `propertyNames` of `s` when generated code checks it: a schema of `pattern`, `minLength` and `maxLength`
/// only, maybe with `type: string`, on a schema of `type: object`. A name is a string, so that type asserts nothing.
/// `None` leaves any other schema to the run-time check of the whole schema.
pub(super) fn typed_property_names(s: &Value) -> Option<Box<Constraints>> {
    // Only an object has names to check: a schema without `type: object` may take any value, which no map holds.
    s.as_object().filter(|o| is_object(o))?;
    let names = s.get("propertyNames")?.as_object()?;
    json::text_only(names).then(|| Box::new(Constraints::own(&Value::Object(names.clone()))))
}

/// `dependentRequired`: each name that triggers it, with the names that it requires then.
pub(super) type Dependents = Vec<(String, Vec<String>)>;

/// Returns `dependentRequired` of `s` when generated code checks it, and whether it names a property that `s` does not
/// declare. Its struct keeps such a name, which is checked on the JSON object then. A map or a value without `type`
/// keeps every name. `None` leaves it to the run-time check of the whole schema.
pub(super) fn typed_dependents(s: &Value) -> Option<(Dependents, bool)> {
    let deps = s.get("dependentRequired")?.as_object()?;
    if !s.as_object().is_some_and(|o| may_be(o, "object")) {
        return None;
    }
    // Without properties the value is a map or a `serde_json::Value`, which keeps every name it was sent.
    let props = json::object(s, "properties").filter(|p| !p.is_empty());
    let mut out = vec![];
    let mut undeclared = false;
    for (name, needs) in deps {
        let needs: Vec<String> =
            needs.as_array()?.iter().map(|n| n.as_str().map(String::from)).collect::<Option<_>>()?;
        undeclared |= props.is_some_and(|p| std::iter::once(name).chain(&needs).any(|n| !p.contains_key(n)));
        out.push((name.clone(), needs));
    }
    Some((out, undeclared))
}

/// Returns the names that `dependentRequired` or `dependentSchemas` of `o` reads: each that triggers one, and each
/// that one requires. Their struct fields fill in no default, so that their absence shows.
pub(super) fn dependent_names(o: &serde_json::Map<String, Value>) -> BTreeSet<&str> {
    let required = json::object(o, "dependentRequired").into_iter().flatten();
    let needs = required.clone().flat_map(|(_, v)| json::strs(v.as_array().map_or(&[][..], Vec::as_slice)));
    let triggers = required.chain(json::object(o, "dependentSchemas").into_iter().flatten()).map(|(k, _)| k.as_str());
    triggers.chain(needs).collect()
}

/// Returns `dependentSchemas` of `s`: generated code checks each.
pub(super) fn typed_dependent_schemas(s: &Value) -> Option<&serde_json::Map<String, Value>> {
    s.get("dependentSchemas")?.as_object()
}

/// Returns a property schema's `example` and `examples` (an array in JSON Schema).
pub(crate) fn property_examples(prop: &Value, ptr: &str) -> Vec<(String, Value)> {
    // A default is a value of the schema too: the test of the documented examples checks it with them.
    let mut out: Vec<(String, Value)> =
        (["example", "default"].iter()).filter_map(|k| prop.get(*k).map(|v| (join(ptr, k), v.clone()))).collect();
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
pub(super) fn int_format(format: &str) -> Option<Ty> {
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

/// Returns a `const` with a type of its own: a unit struct that decodes only its value. That is any value but `null`.
pub(super) fn typed_const(o: &serde_json::Map<String, Value>) -> Option<&Value> {
    // `enum` with one value says what `const` does: `enum: [true]` is a flag that may only be `true`.
    let one_value = || match o.get("enum")?.as_array()?.as_slice() {
        [v] if !o.contains_key("const") => Some(v),
        _ => None,
    };
    let c = o.get("const").or_else(one_value).filter(|c| !c.is_null())?;
    // Composition, conditions and object shapes keep their own typing. A `const` beside an `enum` is both.
    let shaped = ["oneOf", "anyOf", "allOf", "properties", "if", "not"].iter().any(|k| o.contains_key(*k))
        || (o.contains_key("const") && o.contains_key("enum"));
    (!shaped).then_some(c)
}

/// Returns an `enum` of objects, lists or values of several kinds as the `anyOf` of one `const` per value: each is a unit
/// struct that decodes only its value, so the union decodes only a listed value. `null` is `{type: null}`. `None` when
/// a `type` beside it would refuse a listed value, or another keyword beside it shapes the value.
pub(super) fn values_union(o: &serde_json::Map<String, Value>) -> Option<serde_json::Map<String, Value>> {
    let values = o.get("enum")?.as_array().filter(|v| !v.is_empty())?;
    let (types, _) = type_set(o);
    let allowed = |v: &Value| {
        let kind = match v {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(n) if n.is_i64() || n.is_u64() => "integer",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        types.is_empty() || types.iter().any(|t| t == kind || (t == "number" && kind == "integer"))
    };
    let annotation = |k: &str| k == "enum" || k == "type" || ANNOTATIONS.contains(&k) || k.starts_with("x-");
    if !values.iter().all(allowed) || !o.keys().all(|k| annotation(k)) {
        return None;
    }
    let branches = values
        .iter()
        .map(|v| if v.is_null() { serde_json::json!({"type": "null"}) } else { serde_json::json!({"const": v}) });
    let mut u: serde_json::Map<String, Value> =
        o.iter().filter(|(k, _)| !matches!(k.as_str(), "enum" | "type")).map(|(k, v)| (k.clone(), v.clone())).collect();
    u.insert("anyOf".into(), Value::Array(branches.collect()));
    Some(u)
}

/// Does `o` list values that no Rust type holds: an empty `enum`, one of other than strings or integers, or a `const`
/// of other than a string? A constant is a unit type that decodes its one value, and a union of them a list of values
/// ([`values_union`]), so each holds its keyword.
pub(super) fn untyped_values(o: &serde_json::Map<String, Value>) -> bool {
    let untyped = |v: &Value| !v.is_string() && !v.is_null();
    typed_const(o).is_none()
        && values_union(o).is_none()
        && ((o.get("enum").and_then(Value::as_array).is_some_and(|e| e.is_empty() || e.iter().any(untyped))
            && int_enum_values(o).is_none())
            || o.get("const").is_some_and(untyped))
}

/// Adds property `name` of schema `schema` to `props`. A property that another `allOf` member declared already must
/// meet both schemas: it becomes their `allOf`, which its field types and checks as one. The same schema twice is one.
pub(super) fn both(props: &mut serde_json::Map<String, Value>, name: &str, schema: &Value) {
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
pub(super) fn implied(wide: &Value, narrow: &Value) -> bool {
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
pub(super) const ASSERTIONS: [&str; 20] = [
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
pub(super) fn typeless_assertions(s: &Value) -> bool {
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
pub(super) const KIND_SCOPED: [&str; 17] = [
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
pub(super) fn kind_scoped(s: &Value) -> bool {
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
pub(super) fn mixed(s: &Value) -> bool {
    s.as_object().is_some_and(|o| {
        inferred_kind(o).is_some()
            && ASSERTIONS.iter().any(|k| {
                let by_kind = KIND_SCOPED.contains(k) && !matches!(*k, "enum" | "const");
                !matches!(*k, "items" | "prefixItems" | "additionalProperties") && !by_kind && o.contains_key(*k)
            })
    })
}

/// Returns the count that keyword `k` of `s` sets (`minLength`, `maxContains`, ...).
pub(super) fn count_of(s: &Value, k: &str) -> Option<u64> {
    s.get(k).and_then(json::count)
}

/// Does schema `s` take every value: `true`, or an object of annotations only (`{}`, `{$comment: ...}`)?
pub(super) fn asserts_nothing(s: &Value) -> bool {
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
pub(super) fn scoped_keywords(s: &Value, c: &mut Constraints) {
    c.required = required_names(s).map(str::to_owned).collect();
    c.format = s.get("format").and_then(Value::as_str).map(str::to_owned);
}

/// Returns the names in `required` of `s`.
pub(super) fn required_names(s: &Value) -> impl Iterator<Item = &str> {
    json::strs(s.get("required").and_then(Value::as_array).map_or(&[][..], Vec::as_slice))
}

/// Returns the keywords beside a `oneOf` or `anyOf` as a schema of their own: they hold for whichever branch matches,
/// so the value also decodes as its type. `None` without such keywords.
pub(super) fn beside_union(s: &Value) -> Option<Value> {
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
pub(super) fn union_keyword(o: &serde_json::Map<String, Value>) -> Option<&'static str> {
    ["oneOf", "anyOf"].into_iter().find(|k| o.get(*k).is_some_and(Value::is_array))
}

/// Puts the `enum` and `const` of `s` into `c` when no Rust type holds their values: the value is compared as JSON.
pub(super) fn untyped_value_checks(s: &Value, c: &mut Constraints) {
    if let Some(o) = s.as_object().filter(|o| untyped_values(o)) {
        c.enum_values = o.get("enum").and_then(Value::as_array).cloned();
        c.const_value = o.get("const").cloned();
    }
}

/// What [`super::Builder::evaluation`] works out: the names ([`Evaluated`]) or the items ([`EvaluatedItems`]) that a schema
/// evaluates.
pub(super) trait Evaluation: Default {
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

/// Collects each reference under one of `keys` (`$ref`, `$dynamicRef`) of `s` and the subschemas that apply to its
/// value into `out`. Data such as `enum` holds none. `$defs` applies only where a reference reaches it, and that
/// reference is collected.
pub(super) fn references<'s>(s: &'s Value, keys: &[&str], out: &mut Vec<&'s str>) {
    match s {
        Value::Object(o) => {
            out.extend(keys.iter().filter_map(|k| o.get(*k).and_then(Value::as_str)));
            for (k, v) in o {
                match k.as_str() {
                    "enum" | "const" | "default" | "examples" | "example" | "$defs" | "definitions" => {}
                    // A map of names to schemas: a name such as `$defs` is no keyword there.
                    "properties" | "patternProperties" | "dependentSchemas" => {
                        v.as_object()
                            .into_iter()
                            .flat_map(serde_json::Map::values)
                            .for_each(|x| references(x, keys, out));
                    }
                    _ => references(v, keys, out),
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
pub(super) fn inferred_kind(o: &serde_json::Map<String, Value>) -> Option<&'static str> {
    let decided = ["type", "allOf", "oneOf", "anyOf", "$ref", "enum", "const"];
    if decided.iter().any(|k| o.contains_key(*k)) {
        return None;
    }
    if ["properties", "additionalProperties", "patternProperties"].iter().any(|k| o.contains_key(*k)) {
        return Some("object");
    }
    ["items", "prefixItems"].iter().any(|k| o.contains_key(*k)).then_some("array")
}

/// Returns the `default` of a schema when it is a plain literal (string, number or bool). It is written into the code
/// as it is: one of the wrong type does not compile, and one that breaks a keyword of its schema fails the test of the
/// documented examples, where it is checked as one ([`property_examples`]).
pub(crate) fn literal_default(s: &Value) -> Option<Value> {
    s.get("default").filter(|d| json::is_scalar(d)).cloned()
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
/// give the names of its variants when present. Otherwise they are `N1` and `Minus1`.
pub(crate) fn int_enum(o: &serde_json::Map<String, Value>, values: &[i64]) -> Kind {
    let names: Option<Vec<&str>> = ["x-enum-varnames", "x-enumNames", "x-enum-names"]
        .iter()
        .find_map(|k| o.get(*k).and_then(Value::as_array))
        .and_then(|a| a.iter().map(Value::as_str).collect::<Option<Vec<_>>>())
        .filter(|n| n.len() == values.len());
    let mut taken = reserved(&["Other"]);
    let bases = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let base = names.as_ref().map_or_else(
                || Name::new(&if *v < 0 { format!("Minus{}", v.unsigned_abs()) } else { format!("N{v}") }),
                |n| Name::new(n[i]),
            );
            (base, *v)
        })
        .collect();
    Kind::IntEnum(unique_sorted(bases, &mut taken).into_iter().zip(values.iter().copied()).collect())
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
