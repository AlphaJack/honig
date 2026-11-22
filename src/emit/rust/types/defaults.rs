//! Spec defaults: their Rust expressions and the `Default` of a struct.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::syntax::{ident, lit, ty};

use crate::emit::rust::names::pascal;
use crate::emit::rust::space::Space;
use crate::ir::types::{Field, Kind, Struct, Ty};

/// Returns the literal `v` as an expression of type `t` (`qualify`: `types::` paths)
/// if it fits. That is scalars and string/integer enums through aliases.
pub(crate) fn default_expr(space: &Space<'_>, t: &Ty, v: &serde_json::Value, qualify: bool) -> Option<TokenStream> {
    if let Some(e) = literal_expr(space, t, v, qualify) {
        return Some(e);
    }
    // A default that a literal of the type would write but that is no value of it: the crate does not compile, and the
    // error says why. A type with no literal (a decimal, a library format) keeps no default instead.
    literal_type(space, t).then(|| {
        let why = lit(&format!("the `default` {v} of the description is no value of this field's type"));
        quote!(compile_error!(#why))
    })
}

/// Can a default of type `t` be written as a literal: a scalar, a string or integer enum, or a checked type of one?
pub(super) fn literal_type(space: &Space<'_>, t: &Ty) -> bool {
    match space.types.unalias(t) {
        Ty::String
        | Ty::Bool
        | Ty::I8
        | Ty::I16
        | Ty::I32
        | Ty::I64
        | Ty::U8
        | Ty::U16
        | Ty::U32
        | Ty::U64
        | Ty::F32
        | Ty::F64 => true,
        Ty::Named(id) => match &space.types.get(*id).kind {
            Kind::StrEnum(_) | Kind::IntEnum(_) => true,
            Kind::Checked(c) => literal_type(space, &c.base),
            Kind::Struct(_) | Kind::Union(_) | Kind::Alias(_) | Kind::Newtype(_) | Kind::Const(_) | Kind::Pending => {
                false
            }
        },
        Ty::Decimal
        | Ty::Bytes
        | Ty::Binary
        | Ty::Format(_)
        | Ty::Any
        | Ty::Unit
        | Ty::Vec(_)
        | Ty::Array(..)
        | Ty::Map(_)
        | Ty::Option(_)
        | Ty::Indirect(_)
        | Ty::Tuple(_) => false,
    }
}

/// Returns [`default_expr`] when the default is a literal of `t`.
pub(super) fn literal_expr(space: &Space<'_>, t: &Ty, v: &serde_json::Value, qualify: bool) -> Option<TokenStream> {
    use proc_macro2::Literal;
    let t = space.types.unalias(t);
    Some(match (t, v) {
        (Ty::String, serde_json::Value::String(x)) => {
            let x = lit(x);
            quote!(#x.to_owned())
        }
        (Ty::Bool, serde_json::Value::Bool(b)) => quote!(#b),
        (Ty::I8, serde_json::Value::Number(n)) => {
            let l = Literal::i8_suffixed(i8::try_from(n.as_i64()?).ok()?);
            quote!(#l)
        }
        (Ty::I16, serde_json::Value::Number(n)) => {
            let l = Literal::i16_suffixed(i16::try_from(n.as_i64()?).ok()?);
            quote!(#l)
        }
        (Ty::I32, serde_json::Value::Number(n)) => {
            let l = Literal::i32_suffixed(i32::try_from(n.as_i64()?).ok()?);
            quote!(#l)
        }
        (Ty::I64, serde_json::Value::Number(n)) => {
            let l = Literal::i64_suffixed(n.as_i64()?);
            quote!(#l)
        }
        (Ty::U8, serde_json::Value::Number(n)) => {
            let l = Literal::u8_suffixed(u8::try_from(n.as_u64()?).ok()?);
            quote!(#l)
        }
        (Ty::U16, serde_json::Value::Number(n)) => {
            let l = Literal::u16_suffixed(u16::try_from(n.as_u64()?).ok()?);
            quote!(#l)
        }
        (Ty::U32, serde_json::Value::Number(n)) => {
            let l = Literal::u32_suffixed(u32::try_from(n.as_u64()?).ok()?);
            quote!(#l)
        }
        (Ty::U64, serde_json::Value::Number(n)) => {
            let l = Literal::u64_suffixed(n.as_u64()?);
            quote!(#l)
        }
        (Ty::F64, serde_json::Value::Number(n)) => {
            let l = Literal::f64_suffixed(n.as_f64()?);
            quote!(#l)
        }
        (Ty::F32, serde_json::Value::Number(n)) => {
            #[expect(clippy::cast_possible_truncation, reason = "an f32 field's default is written as f32")]
            let l = Literal::f32_suffixed(n.as_f64()? as f32);
            quote!(#l)
        }
        (Ty::Named(id), _) => {
            let kind = &space.types.get(*id).kind;
            if let Kind::Checked(c) = kind {
                // Built without the checks: the test of the documented examples checks the default.
                let inner = literal_expr(space, &c.base, v, qualify)?;
                let t = ty(space, t, qualify);
                // Brace syntax: a twin of the type is declared as an alias of it, and no alias is a tuple constructor.
                return Some(quote!(#t { 0: #inner }));
            }
            return enum_default(kind, &ty(space, t, qualify), v);
        }
        _ => return None,
    })
}

/// Returns the default of an enum: the variant of a listed value. So a `const fn` can set it. A value the enum does not
/// list has no default.
pub(super) fn enum_default(kind: &Kind, t: &TokenStream, v: &serde_json::Value) -> Option<TokenStream> {
    let name = match (kind, v) {
        (Kind::StrEnum(values), serde_json::Value::String(x)) => {
            values.iter().find(|e| e.wire == *x).map(|e| pascal(&e.name))
        }
        (Kind::IntEnum(values), serde_json::Value::Number(n)) => {
            let wanted = n.as_i64()?;
            values.iter().find(|(_, value)| *value == wanted).map(|(name, _)| pascal(name))
        }
        _ => None,
    }?;
    let name = ident(&name);
    Some(quote!(#t::#name))
}

/// Returns the spec default of a field as an expression when it has one that fits.
pub(super) fn field_default(space: &Space<'_>, f: &Field) -> Option<TokenStream> {
    default_expr(space, &f.ty, f.default.as_ref()?, false)
}

/// Returns whether `f`'s `default_<field>` can be a `const fn`. A literal is; `to_owned` and an enum's `From` are not.
pub(super) fn const_default(space: &Space<'_>, f: &Field) -> bool {
    !matches!(space.types.unalias(&f.ty), Ty::String | Ty::Named(_))
}

/// Does `s` take no required argument? Then it has a `Default`. With one it has none: `..Default::default()` would
/// still compile after the description adds a required field, and the new field would start as an empty value.
pub(super) fn struct_default(space: &Space<'_>, s: &Struct) -> bool {
    !s.fields.iter().any(|f| is_arg(space, f))
}

/// Is `f` required with no spec default and more than one value? Then `new` accepts it as an argument.
pub(super) fn is_arg(space: &Space<'_>, f: &Field) -> bool {
    f.required && field_default(space, f).is_none() && !is_unit(space, f)
}

/// Is `f` a required `const`: a unit struct whose one value `new` and `Default` fill in?
pub(super) fn is_unit(space: &Space<'_>, f: &Field) -> bool {
    f.required
        && matches!(space.types.unalias(&f.ty), Ty::Named(c) if matches!(space.types.get(*c).kind, Kind::Const(_)))
}
