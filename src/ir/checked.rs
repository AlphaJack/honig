//! Gives each value with checks that its type does not hold a checked type of its own.
//!
//! A field, a parameter, a body or a branch of a union can carry keywords that its type does not hold: `minItems` on a
//! list, `uniqueItems`, `minProperties`, the length of a format, or keywords that no type expresses (`if`, `not`,
//! `contains` and so on, as a whole schema). The value then becomes a [`Kind::Checked`] newtype of its own whose
//! `try_from` and decoding run them. A named type already holds its own keywords, so a value of one needs nothing
//! more.

use std::collections::{BTreeMap, BTreeSet};

use super::http::Http;
use super::names::{Name, unique};
use super::types::{Checked, Constraints, Kind, Ty, TypeDef, TypeId, TypeSpace, XmlHint};

/// Wraps every value with checks that its type does not hold in a checked type of its own.
pub(crate) fn wrap(types: &mut TypeSpace, http: &mut Http) {
    let mut taken = types.name_keys();
    // The shape of a split type names its fields' wrappers after the type itself: `ProductAttrs`, not
    // `ProductFieldsAttrs`.
    let public = split_own_checks(types, &mut taken);
    let mut space = Wrapper { types, taken: &mut taken };
    for i in 0..space.types.defs.len() {
        let owner = public.get(&i).cloned().unwrap_or_else(|| space.types.defs[i].name.clone());
        let mut kind = std::mem::replace(&mut space.types.defs[i].kind, Kind::Pending);
        match &mut kind {
            Kind::Struct(s) => {
                for f in &mut s.fields {
                    let c = std::mem::take(&mut f.constraints);
                    f.ty = space.wrap(&f.ty, &c, &owner.and(&f.name));
                    f.constraints = c;
                }
            }
            Kind::Union(u) => {
                for v in &mut u.variants {
                    let c = std::mem::take(&mut v.constraints);
                    v.ty = space.wrap(&v.ty, &c, &owner.and(&v.name));
                    v.constraints = c;
                }
            }
            Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Alias(_)
            | Kind::Newtype(_)
            | Kind::Const(_)
            | Kind::Checked(_)
            | Kind::Pending => {}
        }
        space.types.defs[i].kind = kind;
    }
    // The type of a schema that a check decodes the value as (`not`, `dependentSchemas`) checks all that the schema
    // says: a value that decodes as it is one that the schema allows.
    for n in 0..space.types.subschemas.len() {
        let (ty, c) = std::mem::replace(&mut space.types.subschemas[n], (Ty::Any, Constraints::default()));
        let ty = space.wrap(&ty, &c, &Name::new("Not"));
        space.types.subschemas[n] = (ty, Constraints::default());
    }
    for op in &mut http.operations {
        for p in &mut op.params {
            p.ty = space.wrap(&p.ty, &p.constraints, &op.name.and(&p.name));
        }
        let Some(b) = &mut op.body else { continue };
        let body = op.name.and(&Name::new("Body"));
        let one = b.contents.len() == 1;
        for (i, c) in b.contents.iter_mut().enumerate() {
            let own =
                if one { b.constraints.clone() } else { b.variant_constraints.get(i).cloned().unwrap_or_default() };
            let name = if one { body.clone() } else { body.and(&c.variant) };
            // The items of a body read as it arrives are checked one at a time. Its `minItems`, `maxItems` and
            // `uniqueItems` hold on the items as they arrive (`Content::item_limits`).
            if c.streams() {
                if let Ty::Vec(item) = &c.ty {
                    let item = space.wrap(item, &own.item_constraints(), &name.and(&Name::new("Item")));
                    c.ty = Ty::Vec(Box::new(item));
                }
                continue;
            }
            // An optional body is an `Option` of its content: the content type takes the checks.
            c.ty = space.wrap(&c.ty, &own, &name);
        }
    }
}

/// Splits each struct or union whose own schema has keywords that no type holds (`if`, `not`, the one match of a
/// `oneOf`, ...) or that its fields hold together (`dependentRequired`, `propertyNames`). Its shape moves to a type of its own, `<Name>Fields` or `<Name>Variants`. Its name becomes a checked
/// type of that shape. So every value of it is checked once, by its type, and no use of it needs a wrapper.
///
/// Returns the name of the checked type of each shape, by the index of the shape.
fn split_own_checks(types: &mut TypeSpace, taken: &mut BTreeSet<String>) -> BTreeMap<usize, Name> {
    let mut public = BTreeMap::new();
    for i in 0..types.defs.len() {
        let def = &types.defs[i];
        let suffix = match &def.kind {
            Kind::Struct(_) => "Fields",
            Kind::Union(_) => "Variants",
            Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Alias(_)
            | Kind::Newtype(_)
            | Kind::Const(_)
            | Kind::Checked(_)
            | Kind::Pending => continue,
        };
        let own = &def.own;
        // The keywords that the struct or union holds as a whole, not each field or branch.
        let constraints = Constraints {
            schema: own.schema.clone(),
            dependent_required: own.dependent_required.clone(),
            property_names: own.property_names.clone(),
            not: own.not.clone(),
            dependent_schemas: own.dependent_schemas.clone(),
            conditions: own.conditions.clone(),
            ..Constraints::default()
        };
        if def.rust_type.is_some() || constraints.is_empty() {
            continue;
        }
        let raw = TypeId(types.defs.len());
        let name = unique(def.name.and(&Name::new(suffix)), taken);
        let (doc, xml) = (def.doc.clone(), def.xml.clone());
        let shape = TypeDef {
            name,
            doc,
            kind: std::mem::replace(&mut types.defs[i].kind, Kind::Pending),
            xml,
            rust_type: None,
            examples: vec![],
            deprecated: false,
            json_string: false,
            component: false,
            own: Constraints::default(),
            // The shape of a lenient response type reads what the server added, as the type did.
            lenient: types.defs[i].lenient,
        };
        public.insert(raw.0, types.defs[i].name.clone());
        types.defs.push(shape);
        types.defs[i].kind = Kind::Checked(Box::new(Checked { base: Ty::Named(raw), constraints }));
    }
    public
}

struct Wrapper<'a> {
    types: &'a mut TypeSpace,
    taken: &'a mut BTreeSet<String>,
}

impl Wrapper<'_> {
    /// Returns `t` with the value inside any `Option` in a checked type named `name` when `c` asks something that `t`
    /// does not hold. Otherwise `t` as it is.
    fn wrap(&mut self, t: &Ty, c: &Constraints, name: &Name) -> Ty {
        if let Ty::Option(x) = t {
            return Ty::Option(Box::new(self.wrap(x, c, name)));
        }
        if !needs_checks(self.types, t, c, 0) {
            return t.clone();
        }
        let id = TypeId(self.types.defs.len());
        // An inline type of the value often has its name already (`EventCharge` for `charge` of `Event`): the checked
        // one says what it adds.
        let name = if self.taken.contains(&name.key()) { name.and(&Name::new("Checked")) } else { name.clone() };
        self.types.defs.push(TypeDef {
            name: unique(name, self.taken),
            doc: None,
            kind: Kind::Checked(Box::new(Checked { base: t.clone(), constraints: c.clone() })),
            xml: XmlHint::default(),
            rust_type: None,
            examples: vec![],
            deprecated: false,
            json_string: false,
            component: false,
            own: Constraints::default(),
            lenient: false,
        });
        Ty::Named(id)
    }
}

/// Is `t` a type of yours (`x-rust-type`), maybe optional or boxed?
fn yours(types: &TypeSpace, t: &Ty) -> bool {
    match t {
        Ty::Option(x) | Ty::Indirect(x) => yours(types, x),
        Ty::Named(id) => types.get(*id).rust_type.is_some(),
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
        | Ty::Bytes
        | Ty::Binary
        | Ty::Format(_)
        | Ty::Any
        | Ty::Unit
        | Ty::Vec(_)
        | Ty::Array(..)
        | Ty::Tuple(_)
        | Ty::Map(_) => false,
    }
}

/// Keywords of a schema that a `const` or an enum type holds whole: its values, its `type` and annotations.
const ENUM_KEYWORDS: [&str; 13] = [
    "const",
    "enum",
    "type",
    "title",
    "description",
    "default",
    "example",
    "examples",
    "deprecated",
    "readOnly",
    "writeOnly",
    "nullable",
    "$comment",
];

/// Does the type of `t` hold `schema` whole? A checked type of a struct or union holds its own schema
/// (`split_own_checks`). A `{const: physical}` without `type` is checked as a whole schema, and its unit struct holds it
/// already. So does an enum type for its `enum`.
/// Does the named type `t` check `json` (the checks of [`Constraints::json_checks`]) itself? A checked type of the same
/// schema does: a value of it needs them checked no second time.
pub(crate) fn json_held_by_type(types: &TypeSpace, t: &Ty, json: &Constraints, hops: usize) -> bool {
    match t {
        Ty::Option(x) | Ty::Indirect(x) => json_held_by_type(types, x, json, hops),
        Ty::Named(id) => match &types.get(*id).kind {
            Kind::Alias(x) if hops < MAX_ALIAS_HOPS => json_held_by_type(types, x, json, hops + 1),
            Kind::Checked(c) => c.constraints.json_checks() == *json,
            Kind::Alias(_)
            | Kind::Const(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Struct(_)
            | Kind::Union(_)
            | Kind::Newtype(_)
            | Kind::Pending => false,
        },
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
        | Ty::Bytes
        | Ty::Binary
        | Ty::Format(_)
        | Ty::Any
        | Ty::Unit
        | Ty::Vec(_)
        | Ty::Array(..)
        | Ty::Map(_)
        | Ty::Tuple(_) => false,
    }
}

fn held_by_type(types: &TypeSpace, t: &Ty, schema: &serde_json::Value, hops: usize) -> bool {
    let only_values = schema.as_object().is_some_and(|o| o.keys().all(|k| ENUM_KEYWORDS.contains(&k.as_str())));
    match t {
        Ty::Option(x) | Ty::Indirect(x) => held_by_type(types, x, schema, hops),
        Ty::Named(id) => match &types.get(*id).kind {
            Kind::Alias(x) if hops < MAX_ALIAS_HOPS => held_by_type(types, x, schema, hops + 1),
            Kind::Const(_) | Kind::StrEnum(_) | Kind::IntEnum(_) => only_values,
            Kind::Checked(c) => c.constraints.schema.as_ref() == Some(schema),
            Kind::Alias(_) | Kind::Struct(_) | Kind::Union(_) | Kind::Newtype(_) | Kind::Pending => false,
        },
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
        | Ty::Bytes
        | Ty::Binary
        | Ty::Format(_)
        | Ty::Any
        | Ty::Unit
        | Ty::Vec(_)
        | Ty::Array(..)
        | Ty::Tuple(_)
        | Ty::Map(_) => false,
    }
}

/// Aliases followed before a value is taken to need no checks: a chain may loop.
const MAX_ALIAS_HOPS: usize = 32;

/// Does `c` ask something of a value of `t` that `t` does not hold? A named type holds its own keywords. A `pattern`
/// on a format stays unchecked as on its library type.
pub(crate) fn needs_checks(types: &TypeSpace, t: &Ty, c: &Constraints, hops: usize) -> bool {
    // A type of yours (`x-rust-type`) keeps its own invariants: nothing is checked for it.
    if yours(types, t) {
        return false;
    }
    let json = c.json_checks();
    if !json.is_empty() && !json_held_by_type(types, t, &json, hops) {
        return true;
    }
    let c = &c.without_json();
    if let Some(schema) = &c.schema {
        if !held_by_type(types, t, schema, hops) {
            return true;
        }
        let rest = Constraints { schema: None, ..c.clone() };
        return needs_checks(types, t, &rest, hops);
    }
    let range = [&c.minimum, &c.maximum, &c.exclusive_minimum, &c.exclusive_maximum, &c.multiple_of]
        .iter()
        .any(|b| b.is_some());
    let length = c.min_length.is_some() || c.max_length.is_some();
    match t {
        Ty::Option(x) | Ty::Indirect(x) => needs_checks(types, x, c, hops),
        Ty::String => length || c.pattern.is_some(),
        Ty::Format(_) => length,
        Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::F32 | Ty::F64 => range,
        Ty::Decimal => range || c.decimal128,
        Ty::Vec(x) => {
            c.min_items.is_some()
                || c.max_items.is_some()
                || c.unique_items
                || c.contains.is_some()
                || needs_checks(types, x, &c.item_constraints(), hops)
        }
        // The length is the type's.
        Ty::Array(x, _) => {
            c.unique_items || c.contains.is_some() || needs_checks(types, x, &c.item_constraints(), hops)
        }
        Ty::Map(_) => c.min_properties.is_some() || c.max_properties.is_some() || c.property_names.is_some(),
        Ty::Named(id) => match &types.get(*id).kind {
            Kind::Alias(x) if hops < MAX_ALIAS_HOPS => needs_checks(types, x, c, hops + 1),
            Kind::Alias(_)
            | Kind::Struct(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Union(_)
            | Kind::Newtype(_)
            | Kind::Const(_)
            | Kind::Checked(_)
            | Kind::Pending => false,
        },
        Ty::Any => {
            [&c.minimum, &c.maximum, &c.exclusive_minimum, &c.exclusive_maximum, &c.multiple_of]
                .iter()
                .any(|b| b.is_some())
                || [c.min_length, c.max_length, c.min_items, c.max_items, c.min_properties, c.max_properties]
                    .iter()
                    .any(Option::is_some)
                || c.pattern.is_some()
        }
        Ty::Bool | Ty::Bytes | Ty::Binary | Ty::Unit | Ty::Tuple(_) => false,
    }
}
