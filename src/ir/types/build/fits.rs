//! Whether a `default` decodes as the type generated for its schema.
//!
//! Generated code decodes a default that no Rust literal writes (an object, a list) once, from its JSON text. A default
//! that this check proves decodes there. One that it refuses fails generation. One that it cannot judge (a format, a
//! union, a keyword that only generated code checks) is no field default.

use serde_json::Value;

use super::Builder;
use super::keywords::{MAX_ALIAS_HOPS, regex};
use crate::ir::types::{Access, Constraints, Kind, Struct, Ty, TypeDef};
use crate::json;
use crate::json::join;
use crate::validate::unsatisfiable::less;

/// Whether a value decodes as a type.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fit {
    /// It decodes.
    Yes,
    /// It does not: the pointer of the refused value inside the default, and why.
    No(String, String),
    /// The generator cannot tell.
    Unknown,
}

impl Fit {
    /// Returns the first `No`, else `Unknown` when any is, else `Yes`.
    fn all(fits: impl IntoIterator<Item = Self>) -> Self {
        let mut out = Self::Yes;
        for fit in fits {
            match fit {
                Self::No(..) => return fit,
                Self::Unknown => out = Self::Unknown,
                Self::Yes => {}
            }
        }
        out
    }
}

impl Builder<'_> {
    /// Checks the default of each field. One that does not decode is an error. An object or a list stays when it
    /// decodes. A string, number or boolean stays when a Rust literal of the type writes it. Any other is dropped with
    /// a warning.
    pub(super) fn check_defaults(&mut self) {
        let mut verdicts = vec![];
        for (i, d) in self.defs.iter().enumerate() {
            let Kind::Struct(s) = &d.kind else { continue };
            for (j, f) in s.fields.iter().enumerate() {
                let Some(default) = &f.default else { continue };
                let fit = fits(&self.defs, &f.ty, &f.constraints, default, "", 0);
                let literal = json::is_scalar(default) && self.literal_type(&f.ty);
                verdicts.push((i, j, fit, default.is_array() || default.is_object(), literal));
            }
        }
        for (i, j, fit, decoded, literal) in verdicts {
            let Kind::Struct(s) = &mut self.defs[i].kind else { continue };
            let f = &mut s.fields[j];
            let at = f.examples.iter().find(|(at, _)| at.ends_with("/default")).map(|(at, _)| at.clone());
            let at = at.unwrap_or_default();
            let dropped = match (fit, decoded) {
                (Fit::No(inner, why), _) => {
                    self.diags.error(format!("{at}{inner}"), format!("the default does not decode: {why}"));
                    continue;
                }
                (Fit::Unknown, true) => "honig cannot check that this default decodes as the type of its field",
                (_, false) if !literal => "no Rust literal of the type of its field writes this default",
                (Fit::Yes | Fit::Unknown, _) => continue,
            };
            self.diags.warning(at, format!("{dropped}: the field has none and is optional"));
            (f.default, f.required) = (None, false);
        }
    }
}

/// Returns whether `v`, at `at` inside a default, decodes as `t` with the keywords `c` checked.
pub(crate) fn fits(defs: &[TypeDef], t: &Ty, c: &Constraints, v: &Value, at: &str, hops: usize) -> Fit {
    if hops > MAX_ALIAS_HOPS {
        return Fit::Unknown;
    }
    Fit::all([keywords(c, v, at), shape(defs, t, v, at, hops)])
}

/// Returns `No` for `v` at `at`.
fn refused(v: &Value, at: &str, why: &str) -> Fit {
    Fit::No(at.to_owned(), format!("`{v}` {why}"))
}

/// Returns whether `v` decodes as `t`, apart from the keywords beside `t`.
fn shape(defs: &[TypeDef], t: &Ty, v: &Value, at: &str, hops: usize) -> Fit {
    let none = Constraints::default();
    match (t, v) {
        (Ty::Option(_), Value::Null)
        | (Ty::Any, _)
        | (Ty::Unit, Value::Null)
        | (Ty::Bool, Value::Bool(_))
        | (Ty::String, Value::String(_))
        | (Ty::F32 | Ty::F64 | Ty::Decimal, Value::Number(_)) => Fit::Yes,
        (Ty::Option(x) | Ty::Indirect(x), _) => shape(defs, x, v, at, hops),
        (Ty::I8, _) => integer(v, at, i8::MIN.into(), i8::MAX.into()),
        (Ty::I16, _) => integer(v, at, i16::MIN.into(), i16::MAX.into()),
        (Ty::I32, _) => integer(v, at, i32::MIN.into(), i32::MAX.into()),
        (Ty::I64, _) => integer(v, at, i64::MIN.into(), i64::MAX.into()),
        (Ty::U8, _) => integer(v, at, 0, u8::MAX.into()),
        (Ty::U16, _) => integer(v, at, 0, u16::MAX.into()),
        (Ty::U32, _) => integer(v, at, 0, u32::MAX.into()),
        (Ty::U64, _) => integer(v, at, 0, u64::MAX.into()),
        (Ty::Vec(x), Value::Array(items)) => Fit::all(
            items.iter().enumerate().map(|(i, item)| fits(defs, x, &none, item, &join(at, &i.to_string()), hops)),
        ),
        (Ty::Array(x, n), Value::Array(items)) if items.len() == *n => shape(defs, &Ty::Vec(x.clone()), v, at, hops),
        (Ty::Tuple(types), Value::Array(items)) if items.len() == types.len() => Fit::all(
            types
                .iter()
                .zip(items)
                .enumerate()
                .map(|(i, (t, item))| fits(defs, t, &none, item, &join(at, &i.to_string()), hops)),
        ),
        (Ty::Array(_, n), Value::Array(_)) => refused(v, at, &format!("is not a list of {n} items")),
        (Ty::Tuple(types), Value::Array(_)) => refused(v, at, &format!("is not a list of {} items", types.len())),
        (Ty::Map(x), Value::Object(members)) => {
            Fit::all(members.iter().map(|(k, member)| fits(defs, x, &none, member, &join(at, k), hops)))
        }
        (Ty::Named(id), _) => named(defs, &defs[id.0], v, at, hops + 1),
        (Ty::Decimal | Ty::Bytes | Ty::Binary | Ty::Format(_), _) => Fit::Unknown,
        (Ty::Bool, _) => refused(v, at, "is not a boolean"),
        (Ty::String, _) => refused(v, at, "is not a string"),
        (Ty::F32 | Ty::F64, _) => refused(v, at, "is not a number"),
        (Ty::Unit, _) => refused(v, at, "is not null"),
        (Ty::Vec(_) | Ty::Array(..) | Ty::Tuple(_), _) => refused(v, at, "is not a list"),
        (Ty::Map(_), _) => refused(v, at, "is not an object"),
    }
}

/// Returns whether `v` is an integer from `min` to `max`. A number with a zero fractional part is one.
fn integer(v: &Value, at: &str, min: i128, max: i128) -> Fit {
    let Value::Number(n) = v else { return refused(v, at, "is not an integer") };
    #[expect(clippy::cast_possible_truncation, reason = "a float with a zero fractional part within the range of i64")]
    let whole = n
        .as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
        .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9e18).map(|f| f as i128));
    match whole {
        Some(i) if (min..=max).contains(&i) => Fit::Yes,
        Some(_) => refused(v, at, &format!("is not from {min} to {max}")),
        None if n.as_f64().is_some_and(|f| f.fract() != 0.0) => refused(v, at, "is not an integer"),
        None => Fit::Unknown,
    }
}

/// Returns whether `v` decodes as the type `def`.
fn named(defs: &[TypeDef], def: &TypeDef, v: &Value, at: &str, hops: usize) -> Fit {
    if def.rust_type.is_some() || def.json_string {
        return Fit::Unknown;
    }
    let kind = match &def.kind {
        Kind::Alias(t) | Kind::Newtype(t) => shape(defs, t, v, at, hops),
        Kind::Checked(c) => fits(defs, &c.base, &c.constraints, v, at, hops),
        Kind::Const(c) if same(c, v) => Fit::Yes,
        Kind::Const(c) => refused(v, at, &format!("is not the `const` `{c}`")),
        Kind::StrEnum(values) if v.as_str().is_some_and(|s| values.iter().any(|e| e.wire == s)) => Fit::Yes,
        Kind::IntEnum(values) if values.iter().any(|(_, i)| same(&Value::from(*i), v)) => Fit::Yes,
        Kind::StrEnum(_) | Kind::IntEnum(_) => refused(v, at, "is not one of the `enum` values"),
        Kind::Struct(s) => object(defs, s, def.lenient, v, at, hops),
        Kind::Union(_) | Kind::Pending => Fit::Unknown,
    };
    Fit::all([keywords(&def.own, v, at), kind])
}

/// Returns whether `v` decodes as the struct `s`. A `lenient` one accepts properties that it does not declare.
fn object(defs: &[TypeDef], s: &Struct, lenient: bool, v: &Value, at: &str, hops: usize) -> Fit {
    let Value::Object(members) = v else { return refused(v, at, "is not an object") };
    let fields = s.fields.iter().map(|f| match members.get(&f.wire) {
        Some(Value::Null) if !f.required || f.null_as_absent => Fit::Yes,
        Some(_) if f.access != Access::ReadWrite => Fit::Unknown,
        Some(member) => fits(defs, &f.ty, &f.constraints, member, &join(at, &f.wire), hops),
        None if f.required && f.default.is_none() => {
            refused(v, at, &format!("misses the required property `{}`", f.wire))
        }
        None => Fit::Yes,
    });
    let others = members.iter().filter(|(k, _)| !s.fields.iter().any(|f| f.wire == **k)).map(|(k, member)| {
        match &s.additional {
            _ if !s.patterns.is_empty() => Fit::Unknown,
            Some(t) => fits(defs, t, &s.additional_constraints, member, &join(at, k), hops),
            None if s.closed && lenient => Fit::Unknown,
            None if s.closed => refused(v, at, &format!("has the property `{k}` that its schema does not declare")),
            None => Fit::Yes,
        }
    });
    Fit::all(fields.chain(others))
}

/// Returns whether `v` meets the keywords `c` that apply to its JSON kind. Keywords that this check does not judge
/// make it `Unknown`.
fn keywords(c: &Constraints, v: &Value, at: &str) -> Fit {
    let judged = Constraints {
        min_length: None,
        max_length: None,
        pattern: None,
        minimum: None,
        maximum: None,
        exclusive_minimum: None,
        exclusive_maximum: None,
        min_items: None,
        max_items: None,
        unique_items: false,
        never: false,
        min_properties: None,
        max_properties: None,
        items: None,
        required: vec![],
        enum_values: None,
        const_value: None,
        ..c.clone()
    };
    let no = |why: String| Fit::No(at.to_owned(), format!("`{v}` {why}"));
    let check = |refuses: bool, why: &dyn Fn() -> String| if refuses { no(why()) } else { Fit::Yes };
    let size = |n: usize, min: Option<u64>, max: Option<u64>, of: &str| {
        let out = min.is_some_and(|m| (n as u64) < m) || max.is_some_and(|m| (n as u64) > m);
        check(out, &|| format!("has {n} {of}: {}", bounds(min, max)))
    };
    let sized = match v {
        Value::String(s) => size(s.chars().count(), c.min_length, c.max_length, "characters"),
        Value::Array(items) => size(items.len(), c.min_items, c.max_items, "items"),
        Value::Object(members) => size(members.len(), c.min_properties, c.max_properties, "properties"),
        Value::Null | Value::Bool(_) | Value::Number(_) => Fit::Yes,
    };
    let listed = c.enum_values.as_ref().is_none_or(|values| values.iter().any(|x| same(x, v)));
    let constant = c.const_value.as_ref().filter(|x| !same(x, v));
    Fit::all([
        check(c.never, &|| "is refused by a `false` schema".to_owned()),
        check(!listed, &|| "is not one of the `enum` values".to_owned()),
        check(constant.is_some(), &|| format!("is not the `const` `{}`", constant.unwrap_or(&Value::Null))),
        sized,
        kind_keywords(c, v, at),
        if judged == Constraints::default() { Fit::Yes } else { Fit::Unknown },
    ])
}

/// Returns "from `min` to `max`", either side open.
fn bounds(min: Option<u64>, max: Option<u64>) -> String {
    match (min, max) {
        (Some(min), Some(max)) => format!("from {min} to {max} are allowed"),
        (Some(min), None) => format!("at least {min} are allowed"),
        (None, _) => format!("at most {} are allowed", max.unwrap_or_default()),
    }
}

/// Returns whether `v` meets the keywords of `c` beyond sizes: `pattern`, number bounds, `uniqueItems`, `items` and
/// `required`.
fn kind_keywords(c: &Constraints, v: &Value, at: &str) -> Fit {
    let no = |why: String| Fit::No(at.to_owned(), format!("`{v}` {why}"));
    match v {
        Value::String(s) => match c.pattern.as_deref().map(|p| (p, regex(p))) {
            Some((p, Some(re))) if re.find(s).is_none() => no(format!("does not match `{p}`")),
            Some((_, None)) => Fit::Unknown,
            _ => Fit::Yes,
        },
        Value::Number(n) => {
            let below = |b: &Option<serde_json::Number>, strict: bool| {
                b.as_ref().is_some_and(|b| less(n, b) || (strict && !less(b, n)))
            };
            let above = |b: &Option<serde_json::Number>, strict: bool| {
                b.as_ref().is_some_and(|b| less(b, n) || (strict && !less(n, b)))
            };
            if below(&c.minimum, false) || below(&c.exclusive_minimum, true) {
                no("is below the `minimum`".to_owned())
            } else if above(&c.maximum, false) || above(&c.exclusive_maximum, true) {
                no("is above the `maximum`".to_owned())
            } else {
                Fit::Yes
            }
        }
        Value::Array(items) => {
            let twins =
                c.unique_items && items.iter().enumerate().any(|(i, x)| items.iter().take(i).any(|y| same(x, y)));
            let each = c.items.as_deref().map(|items_c| {
                Fit::all(items.iter().enumerate().map(|(i, item)| keywords(items_c, item, &join(at, &i.to_string()))))
            });
            if twins { no("has two equal items".to_owned()) } else { each.unwrap_or(Fit::Yes) }
        }
        Value::Object(members) => match c.required.iter().find(|k| !members.contains_key(*k)) {
            Some(k) => no(format!("misses the required property `{k}`")),
            None => Fit::Yes,
        },
        Value::Null | Value::Bool(_) => Fit::Yes,
    }
}

/// Are `a` and `b` the same JSON value? Numbers compare by value: `1` is `1.0`.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => !less(x, y) && !less(y, x),
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::diag::Diagnostics;
    use crate::ir::names::{Name, Reserved};
    use crate::ir::types::{Builder, TypeId};
    use crate::json::Doc;

    /// Returns the fit of `value` as the type of the component `S` of `schemas`.
    fn fit(schemas: &Value, value: &Value) -> Fit {
        let doc = Doc::new(json!({"openapi": "3.2.0", "components": {"schemas": schemas}}));
        let mut diags = Diagnostics::default();
        let mut b = Builder::new(&doc, &mut diags, Reserved::default());
        b.components();
        let space = b.finish();
        let s = space.defs.iter().position(|d| d.name == Name::new("S")).expect("a component `S`");
        fits(&space.defs, &Ty::Named(TypeId(s)), &Constraints::default(), value, "", 0)
    }

    #[test]
    fn a_default_that_does_not_decode_is_an_error_at_its_member() {
        let doc = Doc::new(json!({"openapi": "3.2.0", "components": {"schemas": {"Frame": {"type": "object",
            "properties": {"options": {"type": "object", "properties": {"color": {"type": "string"}}, "default": {"color": 1}}}}}}}));
        let mut diags = Diagnostics::default();
        let mut b = Builder::new(&doc, &mut diags, Reserved::default());
        b.components();
        b.finish();
        let errors: Vec<_> =
            diags.iter().filter(|d| d.level == crate::diag::Level::Error).map(|d| d.pointer.as_str()).collect();
        assert_eq!(errors, ["/components/schemas/Frame/properties/options/default/color"]);
    }

    #[test]
    fn an_object_of_its_properties_fits() {
        let s =
            json!({"S": {"type": "object", "properties": {"color": {"type": "string"}, "size": {"type": "integer"}}}});
        assert_eq!(fit(&s, &json!({"color": "white", "size": 3.0})), Fit::Yes);
        assert_eq!(fit(&s, &json!({"color": "white", "extra": [1]})), Fit::Yes, "an open object ignores the rest");
    }

    #[test]
    fn a_member_of_the_wrong_type_is_refused_at_its_pointer() {
        let s = json!({"S": {"type": "object", "properties": {"color": {"type": "string"}}}});
        assert_eq!(fit(&s, &json!({"color": 1})), Fit::No("/color".to_owned(), "`1` is not a string".to_owned()));
    }

    #[test]
    fn required_closed_and_listed_keywords_are_checked() {
        let s = json!({"S": {"type": "object", "required": ["sku"], "additionalProperties": false,
            "properties": {"sku": {"type": "string", "pattern": "^[A-Z]+$"}, "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 1}}}});
        assert!(matches!(fit(&s, &json!({})), Fit::No(..)), "misses `sku`");
        assert!(matches!(fit(&s, &json!({"sku": "A", "x": 1})), Fit::No(..)), "declares no `x`");
        assert!(matches!(fit(&s, &json!({"sku": "a"})), Fit::No(..)), "breaks the pattern");
        assert!(matches!(fit(&s, &json!({"sku": "A", "tags": ["a", "b"]})), Fit::No(..)), "two tags");
        assert_eq!(fit(&s, &json!({"sku": "A", "tags": ["a"]})), Fit::Yes);
    }

    #[test]
    fn a_list_of_a_component_fits_through_its_reference() {
        let s = json!({"S": {"type": "array", "items": {"$ref": "#/components/schemas/Size"}}, "Size": {"type": "string", "enum": ["s", "m"]}});
        assert_eq!(fit(&s, &json!(["s", "m"])), Fit::Yes);
        assert_eq!(
            fit(&s, &json!(["l"])),
            Fit::No("/0".to_owned(), "`\"l\"` is not one of the `enum` values".to_owned())
        );
    }

    #[test]
    fn a_format_or_a_union_cannot_be_judged() {
        let date = json!({"S": {"type": "object", "properties": {"on": {"type": "string", "format": "date"}}}});
        assert_eq!(fit(&date, &json!({"on": "2026-01-01"})), Fit::Unknown);
        let union = json!({"S": {"oneOf": [{"type": "string"}, {"type": "integer"}]}});
        assert_eq!(fit(&union, &json!("a")), Fit::Unknown);
    }
}
