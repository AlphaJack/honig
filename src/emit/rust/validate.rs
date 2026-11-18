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
        Ty::F32 | Ty::F64 => float_checks(c, value, path),
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

/// Returns the range and `multipleOf` of an integer type, checked exactly. A bound becomes an integer of the type: a
/// fraction rounds inward. A bound that every value of the type meets is left out. One that no value meets refuses
/// every value: `maximum: -1` on a `uint8` leaves none. `multipleOf` is exact too, through its decimal text: an
/// integer is a multiple of `2.5` when it is one of `5`.
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
    let mut none = None;
    let mut bounds = [quote!(None), quote!(None), quote!(None), quote!(None)];
    let keywords = [
        ("minimum", c.minimum.as_ref(), Bound::AtLeast),
        ("maximum", c.maximum.as_ref(), Bound::AtMost),
        ("exclusiveMinimum", c.exclusive_minimum.as_ref(), Bound::Above),
        ("exclusiveMaximum", c.exclusive_maximum.as_ref(), Bound::Below),
    ];
    for (slot, (keyword, given, bound)) in bounds.iter_mut().zip(keywords) {
        let Some(given) = given else { continue };
        match bound.within(whole_bound(given, bound.rounds_up()), lo, hi) {
            Within::Every => {}
            Within::No => {
                none.get_or_insert_with(|| lit(&format!("`{keyword}: {given}` leaves no value of this integer type")));
            }
            Within::Some(whole) => {
                let literal = Literal::i128_unsuffixed(whole);
                *slot = quote!(Some(#literal));
            }
        }
    }
    if let Some(msg) = none {
        // No value can be valid: the range and `multipleOf` add nothing.
        return quote!(runtime::violation(out, #path, #msg););
    }
    let range = bounds
        .iter()
        .any(|b| b.to_string() != "None")
        .then(|| quote!(runtime::check_range(out, #path, #val, [#(#bounds),*]);));
    let multiple = c.multiple_of.as_ref().and_then(|m| {
        let msg = lit(&format!("is not a multiple of {m}"));
        match integer_modulus(m)? {
            // Every integer is a multiple of `1` and of `0.5`.
            1 => None,
            // Only `0` is a multiple of a modulus past every value of the type.
            m if m > hi.max(-lo) => Some(quote!(if #val != 0 { runtime::violation(out, #path, #msg); })),
            m => {
                let m = Literal::i128_unsuffixed(m);
                Some(quote!(if i128::from(#val) % #m != 0 { runtime::violation(out, #path, #msg); }))
            }
        }
    });
    quote!(#range #multiple)
}

/// Which side of a value a bound keyword holds, and whether it takes the bound itself.
#[derive(Clone, Copy)]
enum Bound {
    /// `minimum`: the value is the bound or more.
    AtLeast,
    /// `maximum`: the value is the bound or less.
    AtMost,
    /// `exclusiveMinimum`: the value is more than the bound.
    Above,
    /// `exclusiveMaximum`: the value is less than the bound.
    Below,
}

/// What a bound leaves of the values of an integer type.
enum Within {
    /// Every value meets it: it needs no check.
    Every,
    /// No value meets it.
    No,
    /// Some values meet it: it is checked as this bound.
    Some(i128),
}

impl Bound {
    /// Does the bound round a fraction up? An inclusive minimum and an exclusive maximum do: `minimum: 1.5` is `2`
    /// and `exclusiveMaximum: 1.5` is `< 2`.
    const fn rounds_up(self) -> bool {
        matches!(self, Self::AtLeast | Self::Below)
    }

    /// Returns what the whole bound `b` leaves of the values from `lo` to `hi`.
    const fn within(self, b: i128, lo: i128, hi: i128) -> Within {
        let (every, no) = match self {
            Self::AtLeast => (b <= lo, b > hi),
            Self::AtMost => (b >= hi, b < lo),
            Self::Above => (b < lo, b >= hi),
            Self::Below => (b > hi, b <= lo),
        };
        if every {
            Within::Every
        } else if no {
            Within::No
        } else {
            Within::Some(b)
        }
    }
}

/// Returns a bound as a whole number, rounded up or down. One past `MAX_WHOLE` in magnitude is past every integer type:
/// it stays past it.
fn whole_bound(n: &Number, up: bool) -> i128 {
    if let Some(i) = n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)) {
        return i;
    }
    let f = n.as_f64().unwrap_or_default();
    let r = if up { f.ceil() } else { f.floor() };
    if r >= MAX_WHOLE {
        i128::MAX
    } else if r <= -MAX_WHOLE {
        i128::MIN
    } else {
        whole(r)
    }
}

/// Returns the integer whose multiples are the integers that are multiples of `m`, from the decimal text of `m`. An
/// integer is a multiple of `p / 10^k` when it is one of `p` with its factors of 2 and 5 that `10^k` holds removed:
/// `2.5` is `25 / 10`, so `5`. `None` for a `multipleOf` that is not positive or does not fit an `i128`; a modulus past
/// `i128` is `i128::MAX`, past every integer type.
fn integer_modulus(m: &Number) -> Option<i128> {
    let (digits, scale) = crate::json::decimal_parts(m)?;
    if digits == 0 {
        return None;
    }
    if scale >= 0 {
        let modulus = (0..scale).try_fold(digits, |acc, _| acc.checked_mul(DECIMAL_BASE)).unwrap_or(u128::MAX);
        return Some(i128::try_from(modulus).unwrap_or(i128::MAX));
    }
    let mut modulus = digits;
    for factor in DECIMAL_BASE_FACTORS {
        for _ in 0..scale.unsigned_abs() {
            if !modulus.is_multiple_of(factor) {
                break;
            }
            modulus /= factor;
        }
    }
    Some(i128::try_from(modulus).unwrap_or(i128::MAX))
}

/// The base of decimal text, and its prime factors.
const DECIMAL_BASE: u128 = 10;
const DECIMAL_BASE_FACTORS: [u128; 2] = [2, 5];

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
    // Exact, through the decimal text of the value and of `multipleOf`: `0.07` is a multiple of `0.01`.
    let multiple = positive(c.multiple_of.as_ref()).map(|m| {
        let m = lit(&m.to_string());
        quote!(runtime::check_multiple(out, #path, #val, #m);)
    });
    quote!(#range #multiple)
}

/// Returns a `multipleOf` that is a positive number. JSON Schema allows no other.
fn positive(m: Option<&Number>) -> Option<&Number> {
    m.filter(|m| m.as_f64().is_some_and(|f| f.is_finite() && f > 0.0))
}

/// Returns the checks of a `Decimal` value (an `&Decimal` expression): its range and `multipleOf` exactly on its digits,
/// and the limits of IEEE 754 decimal128 on them for `format: decimal128`.
pub(super) fn decimal_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let r = receiver(value);
    let bound = |n: Option<&Number>| match n {
        Some(n) => {
            let t = lit(&n.to_string());
            quote!(Some(#t))
        }
        None => quote!(None),
    };
    let bounds = [&c.minimum, &c.maximum, &c.exclusive_minimum, &c.exclusive_maximum];
    let range = bounds.iter().any(|b| b.is_some()).then(|| {
        let [min, max, xmin, xmax] = bounds.map(|b| bound(b.as_ref()));
        quote!(runtime::check_decimal_range(out, #path, #r.as_str(), [#min, #max, #xmin, #xmax]);)
    });
    let multiple = positive(c.multiple_of.as_ref()).map(|m| {
        let m = lit(&m.to_string());
        quote!(runtime::check_multiple_text(out, #path, #r.as_str(), #m);)
    });
    let limits = c.decimal128.then(|| quote!(runtime::check_decimal128(out, #path, #r.as_str());));
    quote!(#range #multiple #limits)
}
/// Bounds past this have no whole `i128` worth writing: they are left unchecked.
const MAX_WHOLE: f64 = 1e30;
