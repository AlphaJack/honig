//! The checks of a checked type: the keywords of its schema that its base type does not hold, run by its `try_from`.
//!
//! Numeric bounds are typed per value at generation time (integers get integer bounds). So generated code compares in
//! the own type of the value and never casts. The `regress` feature of the runtime checks `pattern`. `generate` turns
//! it on when the description has one.

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use serde_json::Number;

use super::syntax::lit;
use crate::emit::rust::space::Space;
use crate::ir::types::{Constraints, Kind, Ty};

/// Checks of `value`, an `&Option<t>`. Nested options peel in one pattern
/// (`if let Some(Some(v))`) and never one `if let` inside another.
fn option_checks<'a>(
    space: &'a Space<'a>,
    mut t: &'a Ty,
    c: &Constraints,
    value: &TokenStream,
    path: &TokenStream,
) -> TokenStream {
    let mut pat = quote!(Some(v));
    loop {
        if let Ty::Option(x) = t {
            pat = quote!(Some(#pat));
            t = x;
        } else if let Ty::Named(id) = t
            && let Kind::Alias(x) = &space.types.get(*id).kind
            && matches!(x, Ty::Option(_) | Ty::Named(_))
        {
            // A named alias is a `type` item: its option peels too.
            t = x;
        } else {
            break;
        }
    }
    let inner = checks(space, t, c, &quote!(v), path);
    if inner.is_empty() { inner } else { quote!(if let #pat = #value { #inner }) }
}

/// Returns the value behind `value` (an `&T` expression) for a `Copy` type: a
/// leading `&` goes rather than emitting `*&` (clippy `deref_addrof`).
fn deref(value: &TokenStream) -> TokenStream {
    let mut tokens = value.clone().into_iter().peekable();
    match tokens.peek() {
        Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '&' => {
            tokens.next();
            tokens.collect()
        }
        _ => quote!(*#value),
    }
}

/// Returns `value` as a method receiver. Methods auto-borrow. So a leading `&` goes
/// (clippy `needless_borrow`). Other expressions get parentheses.
fn receiver(value: &TokenStream) -> TokenStream {
    let mut tokens = value.clone().into_iter().peekable();
    match tokens.peek() {
        Some(proc_macro2::TokenTree::Punct(p)) if p.as_char() == '&' => {
            tokens.next();
            tokens.collect()
        }
        Some(proc_macro2::TokenTree::Ident(_)) if value.clone().into_iter().count() == 1 => value.clone(),
        _ => quote!((#value)),
    }
}

/// Returns the length and pattern checks of a `String` value.
pub(super) fn string_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let pat = c.pattern.as_ref().map(|p| {
        let msg = lit(&format!("does not match `{p}`"));
        let p = lit(p);
        quote!(runtime::check(out, #path, runtime::pattern_matches({
            static RE: runtime::Pattern = runtime::Pattern::new();
            &RE
        }, #p, #value), #msg);)
    });
    if c.min_length.is_none() && c.max_length.is_none() {
        return quote!(#pat);
    }
    let (min, max) = (opt_u64(c.min_length), opt_u64(c.max_length));
    let r = receiver(value);
    // A value past `maxLength` is wrong already. Its pattern does not run on it: a pattern that backtracks takes
    // exponential time on a long value that almost matches.
    let pat = match (pat, c.max_length) {
        (Some(pat), Some(m)) => {
            let m = Literal::u64_unsuffixed(m);
            Some(quote!(if n <= #m { #pat }))
        }
        (pat, _) => pat,
    };
    quote! {
        let n = #r.chars().count();
        runtime::check_len(out, #path, n, #min, #max, "characters");
        #pat
    }
}

/// Checks of `value` (an expression of type `&T` for `t`) at `path`.
/// Empty when nothing can be violated.
pub(super) fn checks(
    space: &Space<'_>,
    t: &Ty,
    c: &Constraints,
    value: &TokenStream,
    path: &TokenStream,
) -> TokenStream {
    // A type of yours keeps its own invariants (see `live_types`): the schema where it is used is not checked either.
    if space.is_external(t) {
        return TokenStream::new();
    }
    // Keywords without a Rust type of their own. The whole fragment (refs
    // inlined) is checked on the serialized value. Typed checks are skipped.
    // It is JSON text parsed once: as Rust that builds the value, a large
    // fragment costs the compiler minutes.
    if let Some(schema) = &c.schema
        && !matches!(t, Ty::Option(_) | Ty::Indirect(_))
    {
        let json = lit(&schema.to_string());
        return quote! {
            {
                static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
                let value = runtime::to_value(#value).unwrap_or_default();
                runtime::check_schema(out, #path, &value, SCHEMA.get_or_init(|| serde_json::from_str(#json).unwrap_or_default()));
            }
        };
    }
    match t {
        Ty::Option(x) => option_checks(space, x, c, value, path),
        Ty::Indirect(x) => checks(space, x, c, &quote!((#value).as_ref()), path),
        Ty::String => string_checks(c, value, path),
        // A format without its library is a `runtime::Formatted` text: decoding checks the format. Its length limits
        // are checked here. A `pattern` stays unchecked on a format as on the typed one.
        Ty::Format(f) if super::syntax::untyped(space.libs, *f) => {
            string_checks(&Constraints { pattern: None, ..c.clone() }, value, path)
        }
        Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => {
            integer_checks(t, c, value, path)
        }
        Ty::F32 | Ty::F64 => float_checks(c, &float_value(t, value), path),
        Ty::Decimal => decimal_checks(c, value, path),
        Ty::Vec(x) => {
            let len = (c.min_items.is_some() || c.max_items.is_some()).then(|| {
                let (min, max) = (opt_u64(c.min_items), opt_u64(c.max_items));
                let r = receiver(value);
                quote!(runtime::check_len(out, #path, #r.len(), #min, #max, "items");)
            });
            let items = item_checks(space, x, c, value, path);
            quote!(#len #items)
        }
        // The length is the type's: only uniqueness and the items are checked.
        Ty::Array(x, _) => item_checks(space, x, c, value, path),
        // A value of a named type in the map checks itself where it decodes.
        Ty::Map(_) if c.min_properties.is_some() || c.max_properties.is_some() => {
            let (min, max) = (opt_u64(c.min_properties), opt_u64(c.max_properties));
            let r = receiver(value);
            quote!(runtime::check_len(out, #path, #r.len(), #min, #max, "properties");)
        }
        Ty::Named(id) => match &space.types.get(*id).kind {
            Kind::Alias(x) => checks(space, x, c, value, path),
            // Any other named type holds its own keywords: only a valid value decodes or is built.
            Kind::Struct(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Union(_)
            | Kind::Newtype(_)
            | Kind::Const(_)
            | Kind::Checked(_)
            | Kind::Pending => TokenStream::new(),
        },
        Ty::Map(_) | Ty::Bool | Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Any | Ty::Unit | Ty::Tuple(_) => {
            TokenStream::new()
        }
    }
}

/// Returns `uniqueItems` and the checks of each item for a `Vec` or an array.
fn item_checks(space: &Space<'_>, x: &Ty, c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let unique = c.unique_items.then(|| quote!(runtime::check_unique(out, #path, #value);));
    let ic = c.item_constraints();
    // The parent path is bound once for every item.
    let item = checks(space, x, &ic, &quote!(v), &quote!(&runtime::InstancePath::Index(base, i)));
    let r = receiver(value);
    let each = (!item.is_empty()).then(|| quote!({ let base = #path; for (i, v) in #r.iter().enumerate() { #item } }));
    quote!(#unique #each)
}

fn opt_u64(v: Option<u64>) -> TokenStream {
    match v {
        Some(n) => {
            let n = Literal::u64_unsuffixed(n);
            quote!(Some(#n))
        }
        None => quote!(None),
    }
}

/// Returns the value of a float as `float_checks` reads it. A decimal compares by its nearest `f64` (a range and not
/// the value).
pub(super) fn float_value(t: &Ty, value: &TokenStream) -> TokenStream {
    if *t == Ty::Decimal {
        let r = receiver(value);
        quote!(&#r.as_f64())
    } else {
        value.clone()
    }
}

/// Returns the range and `multipleOf` for an integer type with bounds as integers of
/// that type. Fractional bounds are rounded inward. Bounds that cannot be represented are dropped.
pub(super) fn integer_checks(t: &Ty, c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let val = deref(value);
    let ranges: [(Ty, i128, i128); 7] = [
        (Ty::I8, i8::MIN.into(), i8::MAX.into()),
        (Ty::I16, i16::MIN.into(), i16::MAX.into()),
        (Ty::I32, i32::MIN.into(), i32::MAX.into()),
        (Ty::U8, 0, u8::MAX.into()),
        (Ty::U16, 0, u16::MAX.into()),
        (Ty::U32, 0, u32::MAX.into()),
        (Ty::U64, 0, u64::MAX.into()),
    ];
    // Any other integer is an `i64`.
    let (lo, hi) = ranges
        .iter()
        .find(|(k, ..)| k == t)
        .map_or_else(|| (i64::MIN.into(), i64::MAX.into()), |&(_, lo, hi)| (lo, hi));
    let fits = |n: i128| (lo..=hi).contains(&n).then(|| Literal::i128_unsuffixed(n));
    let int = |n: &Number, up: bool| -> Option<i128> {
        n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)).or_else(|| {
            let f = n.as_f64()?;
            let r = if up { f.ceil() } else { f.floor() };
            (r.is_finite() && r.abs() < MAX_WHOLE).then(|| whole(r))
        })
    };
    let bound = |n: Option<&Number>, up: bool| -> TokenStream {
        match n.and_then(|n| int(n, up)).and_then(fits) {
            Some(l) => quote!(Some(#l)),
            None => quote!(None),
        }
    };
    let (min, max) = (bound(c.minimum.as_ref(), true), bound(c.maximum.as_ref(), false));
    let (xmin, xmax) = (bound(c.exclusive_minimum.as_ref(), false), bound(c.exclusive_maximum.as_ref(), true));
    let range = [&c.minimum, &c.maximum, &c.exclusive_minimum, &c.exclusive_maximum]
        .iter()
        .any(|b| b.is_some())
        .then(|| quote!(runtime::check_range(out, #path, #val, [#min, #max, #xmin, #xmax]);));
    let multiple =
        c.multiple_of.as_ref().and_then(Number::as_u64).filter(|m| *m > 1).and_then(|m| fits(m.into())).map(|m| {
            let msg = lit(&format!("is not a multiple of {m}"));
            quote!(if #val % #m != 0 { runtime::violation(out, #path, #msg); })
        });
    quote!(#range #multiple)
}

/// Returns an integral `f64` (finite and below 1e30 in magnitude) as an integer.
#[expect(clippy::cast_possible_truncation, reason = "callers pass finite integral values far inside i128")]
const fn whole(r: f64) -> i128 {
    r as i128
}

pub(super) fn float_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let val = deref(value);
    let bound = |n: Option<&Number>| match n.and_then(Number::as_f64).filter(|f| f.is_finite()) {
        Some(f) => {
            let l = Literal::f64_unsuffixed(f);
            quote!(Some(#l))
        }
        None => quote!(None),
    };
    let range =
        [&c.minimum, &c.maximum, &c.exclusive_minimum, &c.exclusive_maximum].iter().any(|b| b.is_some()).then(|| {
            let (min, max) = (bound(c.minimum.as_ref()), bound(c.maximum.as_ref()));
            let (xmin, xmax) = (bound(c.exclusive_minimum.as_ref()), bound(c.exclusive_maximum.as_ref()));
            quote!(runtime::check_range(out, #path, #val, [#min, #max, #xmin, #xmax]);)
        });
    let multiple = c.multiple_of.as_ref().and_then(Number::as_f64).filter(|m| m.is_finite() && *m > 0.0).map(|m| {
        let l = Literal::f64_unsuffixed(m);
        let msg = lit(&format!("is not a multiple of {m}"));
        quote! {
            let q = #val / #l;
            if (q - q.round()).abs() > 1e-9 { runtime::violation(out, #path, #msg); }
        }
    });
    quote!(#range #multiple)
}

/// Returns the checks of a `Decimal` value (an `&Decimal` expression): its range and `multipleOf` by its nearest `f64`,
/// and the limits of IEEE 754 decimal128 on its digits for `format: decimal128`.
pub(super) fn decimal_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let range = float_checks(c, &float_value(&Ty::Decimal, value), path);
    let r = receiver(value);
    let limits = c.decimal128.then(|| quote!(runtime::check_decimal128(out, #path, #r.as_str());));
    quote!(#range #limits)
}

/// Bounds past this have no whole `i128` worth writing: they are left unchecked.
const MAX_WHOLE: f64 = 1e30;
