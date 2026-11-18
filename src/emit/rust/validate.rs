//! `runtime::Validate` impls: schema constraints checked on server requests.
//!
//! Numeric bounds are typed per field at generation time (integers get
//! integer bounds). So generated code compares in the own type of the field and
//! never casts. The `regress` feature of the runtime checks `pattern`.
//! `generate` turns it on when the description has one.

use std::collections::BTreeMap;

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use serde_json::Number;

use super::syntax::lit;
use crate::emit::rust::names::pascal;
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::http::{Content, Location, Operation};
use crate::ir::types::{Constraints, Kind, Struct, Ty, TypeDef};

/// What the generation of checks needs: the types, the pattern matchers and which
/// named types can report a violation at all.
#[derive(Clone, Copy)]
pub(super) struct Cx<'a> {
    pub space: &'a Space<'a>,
    /// Types declared as an alias of their first twin: index to the first.
    pub aliases: &'a BTreeMap<usize, usize>,
    /// Per type: can its `validate` report anything? A type whose checks only
    /// recurse into itself (a tree of plain nodes) cannot.
    pub live: &'a [bool],
}

/// Returns which named types can report a violation. It is a fixpoint from none. A type
/// turns live once its checks are non-empty given the live set so far.
pub(super) fn live_types(space: &Space<'_>, aliases: &BTreeMap<usize, usize>) -> Vec<bool> {
    // An `x-rust-type` (a type of yours) is never checked. Generated code cannot implement `runtime::Validate` for it
    // (the orphan rule of Rust). Its own constructors keep its invariants.
    let mut live: Vec<bool> = vec![false; space.types.defs.len()];
    loop {
        let cx = Cx { space, aliases, live: &live };
        let next: Vec<bool> = space
            .types
            .defs
            .iter()
            .enumerate()
            .map(|(i, d)| d.rust_type.is_none() && (live[i] || !type_body(cx, d).is_empty()))
            .collect();
        if next == live {
            return live;
        }
        live = next;
    }
}

/// Returns `impl runtime::Validate` for a named type.
pub(super) fn type_impl(cx: Cx<'_>, def: &TypeDef) -> Option<TokenStream> {
    // Without pattern checks a `Pattern` is a `String` alias: no impl of its own.
    if matches!(def.kind, Kind::Alias(_) | Kind::Pending) {
        return None;
    }
    Some(validate_impl(&pascal_ident(&def.name), &type_body(cx, def)))
}

/// Returns the checks in a named type's `validate`; empty when nothing can fail.
fn type_body(cx: Cx<'_>, def: &TypeDef) -> TokenStream {
    match &def.kind {
        Kind::Struct(s) => struct_checks(cx, s),
        Kind::Union(u) => {
            // Variants with nothing to check share one arm. So do variants of one type (clippy `match_same_arms`).
            let (mut checked, mut unchecked) = (vec![], vec![]);
            for v in &u.variants {
                let vn = pascal_ident(&v.name);
                let inner = checks(cx, &v.ty, &v.constraints, &quote!(x), &quote!(path));
                if inner.is_empty() {
                    unchecked.push(quote!(Self::#vn(_)));
                } else {
                    let same = cx.space.resolved(cx.aliases, &v.ty);
                    checked.push((quote!(Self::#vn(x)), Some(same), quote!({ #inner })));
                }
            }
            if checked.is_empty() {
                return TokenStream::new();
            }
            // `Other` is valid in an open union and, in a `rest` union, holds a kind that no variant covers. A closed
            // union has none: decoding refuses a value that no variant takes.
            if u.open || u.rest {
                unchecked.push(quote!(Self::Other(_)));
            }
            let arms = super::syntax::shared_arms(checked);
            let rest = unchecked_arm(&unchecked);
            quote! {
                match self {
                    #(#arms)*
                    #rest
                }
            }
        }
        Kind::Newtype(t) => checks(cx, t, &Constraints::default(), &quote!(&self.0), &quote!(path)),
        // Decoding already rejects any other value.
        Kind::StrEnum(_) | Kind::IntEnum(_) | Kind::Const(_) | Kind::Pattern(_) | Kind::Alias(_) | Kind::Pending => {
            TokenStream::new()
        }
    }
}

fn struct_checks(cx: Cx<'_>, s: &Struct) -> TokenStream {
    let fields = s.fields.iter().map(|f| {
        let name = snake_ident(&f.name);
        let p = lit(&f.wire);
        let path = quote!(&runtime::InstancePath::Key(path, #p));
        if f.required {
            checks(cx, &f.ty, &f.constraints, &quote!(&self.#name), &path)
        } else if let Some(inner) = cx.space.types.nullable(&f.ty) {
            let inner = checks(cx, inner, &f.constraints, &quote!(v), &path);
            if inner.is_empty() { inner } else { quote!(if let runtime::Nullable::Value(v) = &self.#name { #inner }) }
        } else {
            option_checks(cx, &f.ty, &f.constraints, &quote!(&self.#name), &path)
        }
    });
    quote!(#(#fields)*)
}

/// Checks of `value`, an `&Option<t>`. Nested options peel in one pattern
/// (`if let Some(Some(v))`) and never one `if let` inside another.
fn option_checks<'a>(
    cx: Cx<'a>,
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
            && let Kind::Alias(x) = &cx.space.types.get(*id).kind
            && matches!(x, Ty::Option(_) | Ty::Named(_))
        {
            // A named alias is a `type` item: its option peels too.
            t = x;
        } else {
            break;
        }
    }
    let inner = checks(cx, t, c, &quote!(v), path);
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

/// Returns `impl runtime::Validate for Params`: every parameter and the body.
pub(super) fn params_impl(cx: Cx<'_>, op: &Operation, boxed: &[bool]) -> TokenStream {
    let fields = op.params.iter().map(|p| {
        let loc = match p.location {
            Location::Path => "path",
            Location::Query | Location::QueryString => "query",
            Location::Header => "header",
            Location::Cookie => "cookie",
            // `/body/<property>/<header>`: the part the header travels in.
            Location::Part => "body",
        };
        let (loc, wire) = (lit(loc), lit(&p.wire));
        let name = snake_ident(&p.name);
        let at = match p.part.as_deref() {
            Some(part) => {
                let part = lit(part);
                quote!(&runtime::InstancePath::Key(&runtime::InstancePath::Key(&runtime::InstancePath::Key(path, #loc), #part), #wire))
            }
            None => quote!(&runtime::InstancePath::Key(&runtime::InstancePath::Key(path, #loc), #wire)),
        };
        checks(cx, &p.ty, &p.constraints, &quote!(&self.#name), &at)
    });
    let body = op.body.as_ref().map(|b| {
        let at = quote!(&runtime::InstancePath::Key(path, "body"));
        if let [c] = b.contents.as_slice() {
            if c.streams() {
                let each = arriving_checks(cx, c, &b.constraints, &quote!(body), &at);
                return match (each.is_empty(), b.required) {
                    (true, _) => each,
                    (false, true) => quote!({ let body = &self.body; #each }),
                    (false, false) => quote!(if let Some(body) = &self.body { #each }),
                };
            }
            let t = if b.required { c.ty.clone() } else { c.ty.clone().option() };
            return checks(cx, &t, &b.constraints, &quote!(&self.body), &at);
        }
        // A body enum (several media types). The value of each variant is checked. Variants checked alike share an
        // arm (clippy `match_same_arms`) because `Body::Json(x) | Body::Form(x)` binds `x` to the same type.
        let mut arms_for: Vec<String> = vec![];
        let mut groups: Vec<(String, TokenStream, Vec<TokenStream>)> = vec![];
        for (i, c) in b.contents.iter().enumerate() {
            let t = if boxed.get(i).copied().unwrap_or(false) {
                Ty::Indirect(Box::new(c.ty.clone()))
            } else {
                c.ty.clone()
            };
            let own = b.variant_constraints.get(i).cloned().unwrap_or_default();
            let inner = if c.streams() {
                arriving_checks(cx, c, &own, &quote!(x), &at)
            } else {
                checks(cx, &t, &own, &quote!(x), &at)
            };
            if inner.is_empty() {
                continue;
            }
            arms_for.push(pascal(&c.variant));
            let v = pascal_ident(&c.variant);
            let key = format!("{t:?} {inner}");
            match groups.iter_mut().find(|(k, ..)| *k == key) {
                Some((.., pats)) => pats.push(quote!(Body::#v(x))),
                None => groups.push((key, inner, vec![quote!(Body::#v(x))])),
            }
        }
        if groups.is_empty() {
            return TokenStream::new();
        }
        let arms = groups.iter().map(|(_, inner, pats)| quote!(#(#pats)|* => { #inner }));
        // Variants with nothing to check share one arm. It names them so that a variant added later is a decision and
        // not a wildcard.
        let unchecked: Vec<_> = b
            .contents
            .iter()
            .filter(|c| !arms_for.contains(&pascal(&c.variant)))
            .map(|c| {
                let v = pascal_ident(&c.variant);
                quote!(Body::#v(_))
            })
            .collect();
        let rest = unchecked_arm(&unchecked);
        let check = quote!(match body { #(#arms)* #rest });
        if b.required {
            quote!({ let body = &self.body; #check })
        } else {
            quote!(if let Some(body) = &self.body { #check })
        }
    });
    validate_impl(&quote!(Params), &quote!(#(#fields)* #body))
}

/// Returns the checks of each item for a sequential body (`runtime::Items`) as it arrives. A whole-body constraint
/// (`minItems`, `uniqueItems`) needs every item before the handler runs. So a streamed body skips it.
fn arriving_checks(
    cx: Cx<'_>,
    c: &Content,
    constraints: &Constraints,
    value: &TokenStream,
    path: &TokenStream,
) -> TokenStream {
    let Ty::Vec(item) = &c.ty else { return TokenStream::new() };
    let each = checks(cx, item.as_ref(), &constraints.item_constraints(), &quote!(item), &quote!(path));
    if each.is_empty() {
        return each;
    }
    quote!(#value.check_each(#path, out, |item, path, out| { #each });)
}

/// Returns one arm for the variants with nothing to check. It names them so that a variant added later is a decision
/// and not a wildcard.
fn unchecked_arm(patterns: &[TokenStream]) -> Option<TokenStream> {
    (!patterns.is_empty()).then(|| quote!(#(#patterns)|* => {}))
}

/// Returns `impl runtime::Validate for #name`; empty when nothing can fail (the trait's
/// default `validate` checks nothing).
fn validate_impl(name: &impl quote::ToTokens, body: &TokenStream) -> TokenStream {
    if body.is_empty() {
        return quote!(impl runtime::Validate for #name {});
    }
    quote! {
        impl runtime::Validate for #name {
            fn validate(&self, path: &runtime::InstancePath<'_>, out: &mut Vec<runtime::Violation>) {
                #body
            }
        }
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
fn string_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
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
fn checks(cx: Cx<'_>, t: &Ty, c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    // A type of yours keeps its own invariants (see `live_types`): the schema where it is used is not checked either.
    if cx.space.is_external(t) {
        return TokenStream::new();
    }
    // Keywords without a Rust type of their own. The whole fragment (refs
    // inlined) is checked on the serialized value. Typed checks are skipped.
    // It is JSON text parsed once: as Rust that builds the value, a large
    // fragment costs the compiler minutes.
    if let Some(schema) = &c.schema
        && !matches!(t, Ty::Option(_) | Ty::Indirect(_))
    {
        // The definitions that it reaches by pointer are in `runtime.rs` once for the whole module.
        let json = lit(&schema.to_string());
        let shared = if cx.space.types.shared.is_empty() { quote!(None) } else { quote!(runtime::schema_defs()) };
        return quote! {
            {
                static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
                let value = runtime::to_value(#value).unwrap_or_default();
                let schema = SCHEMA.get_or_init(|| serde_json::from_str(#json).unwrap_or_default());
                runtime::check_schema_in(out, #path, &value, schema, #shared);
            }
        };
    }
    match t {
        Ty::Option(x) => option_checks(cx, x, c, value, path),
        Ty::Indirect(x) => checks(cx, x, c, &quote!((#value).as_ref()), path),
        Ty::String => string_checks(c, value, path),
        // A format without its library is a `String`. The text is checked here as the type of the library would on
        // parsing.
        Ty::Format(f) if super::syntax::untyped(cx.space.libs, *f) => {
            let name = lit(f.name());
            let r = receiver(value);
            // Its length limits too. A `pattern` stays unchecked on a format as on the typed one.
            let own = string_checks(&Constraints { pattern: None, ..c.clone() }, value, path);
            quote!(runtime::check_format(out, #path, #r.as_str(), #name); #own)
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
            let items = item_checks(cx, x, c, value, path);
            quote!(#len #items)
        }
        // The length is the type's: only uniqueness and the items are checked.
        Ty::Array(x, _) => item_checks(cx, x, c, value, path),
        Ty::Map(x) => {
            let len = (c.min_properties.is_some() || c.max_properties.is_some()).then(|| {
                let (min, max) = (opt_u64(c.min_properties), opt_u64(c.max_properties));
                let r = receiver(value);
                quote!(runtime::check_len(out, #path, #r.len(), #min, #max, "properties");)
            });
            let nested = named_inside(cx, x).then(|| quote!(runtime::Validate::validate(#value, #path, out);));
            quote!(#len #nested)
        }
        Ty::Named(id) => match &cx.space.types.get(*id).kind {
            Kind::Alias(x) => checks(cx, x, c, value, path),
            Kind::Struct(_) | Kind::StrEnum(_) | Kind::IntEnum(_) | Kind::Union(_) | Kind::Newtype(_)
                if cx.live[id.0] =>
            {
                quote!(runtime::Validate::validate(#value, #path, out);)
            }
            Kind::Struct(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Union(_)
            | Kind::Newtype(_)
            | Kind::Const(_)
            | Kind::Pattern(_)
            | Kind::Pending => TokenStream::new(),
        },
        Ty::Bool | Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Any | Ty::Unit | Ty::Tuple(_) => TokenStream::new(),
    }
}

/// Returns `uniqueItems` and the checks of each item for a `Vec` or an array.
fn item_checks(cx: Cx<'_>, x: &Ty, c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let unique = c.unique_items.then(|| quote!(runtime::check_unique(out, #path, #value);));
    let ic = c.item_constraints();
    // The parent path is bound once for every item.
    let item = checks(cx, x, &ic, &quote!(v), &quote!(&runtime::InstancePath::Index(base, i)));
    let r = receiver(value);
    let each = (!item.is_empty()).then(|| quote!({ let base = #path; for (i, v) in #r.iter().enumerate() { #item } }));
    quote!(#unique #each)
}

/// Does a map value type reach a named type (with a `Validate` impl)?
fn named_inside(cx: Cx<'_>, t: &Ty) -> bool {
    let Ty::Named(id) = t else {
        // The positions of a tuple are checked where it is and not through `Validate`.
        return !matches!(t, Ty::Tuple(_)) && t.children().iter().any(|x| named_inside(cx, x));
    };
    match &cx.space.types.get(*id).kind {
        Kind::Alias(x) => named_inside(cx, x),
        Kind::Pending => false,
        Kind::Struct(_)
        | Kind::StrEnum(_)
        | Kind::IntEnum(_)
        | Kind::Union(_)
        | Kind::Newtype(_)
        | Kind::Const(_)
        | Kind::Pattern(_) => cx.live[id.0],
    }
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
fn integer_checks(t: &Ty, c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
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
                none.get_or_insert_with(|| {
                    lit(&format!("`{keyword}: {given}` leaves no value of this integer type, not even {{}}"))
                });
            }
            Within::Some(whole) => {
                let literal = Literal::i128_unsuffixed(whole);
                *slot = quote!(Some(#literal));
            }
        }
    }
    if let Some(msg) = none {
        // No value can be valid: the range and `multipleOf` add nothing.
        return quote!(runtime::violation(out, #path, format!(#msg, #val)););
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
            m if m > hi.max(-lo) => Some(quote!(runtime::check(out, #path, #val == 0, #msg);)),
            m => {
                let m = Literal::i128_unsuffixed(m);
                Some(quote!(runtime::check(out, #path, i128::from(#val) % #m == 0, #msg);))
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

fn float_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
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

/// Returns the range and `multipleOf` of a `Decimal` value (an `&Decimal` expression), checked exactly on its digits,
/// and the limits of IEEE 754 decimal128 on them for `format: decimal128`.
fn decimal_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
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
    // `format: decimal128`: the digits that IEEE 754 decimal128 holds.
    let limits = c.decimal128.then(|| quote!(runtime::check_decimal128(out, #path, #r.as_str());));
    quote!(#range #multiple #limits)
}

/// Bounds past this have no whole `i128` worth writing: they are left unchecked.
const MAX_WHOLE: f64 = 1e30;
