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
use crate::ir::types::{Constraints, Evaluated, EvaluatedItems, Kind, Leftover, Rest, Ty};

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

/// Returns `not` and `dependentSchemas` of `c` on `value` (an expression of type `&T`) at `path`. A value that decodes
/// as the type of a schema is one that the schema allows.
pub(super) fn subschema_checks(
    space: &Space<'_>,
    c: &Constraints,
    value: &TokenStream,
    path: &TokenStream,
) -> TokenStream {
    if c.json_checks().is_empty() {
        return TokenStream::new();
    }
    let ty = |n: usize| super::syntax::ty(space, &space.types.subschemas[n].0, false);
    let nots = c.not.iter().map(|&n| {
        let not = ty(n);
        quote! {
            if json.decode_ref::<#not>().is_ok() {
                runtime::violation(out, #path, "matches `not`");
            }
        }
    });
    let dependents = c.dependent_schemas.iter().map(|(name, n)| {
        let (dependent, wire) = (ty(*n), lit(name));
        let present = lit(&format!("`{name}` is present: "));
        quote! {
            if json.get(#wire).is_some()
                && let Err(e) = json.decode_ref::<#dependent>()
            {
                runtime::violation(out, #path, format!("{}{e}", #present));
            }
        }
    });
    let conditions = condition_checks(c, &ty, path);
    let branches = branch_count_checks(c, &ty, path);
    let patterns = pattern_checks(c, &ty, path);
    let tuple = tuple_checks(c, &ty, path);
    let also = c.also.iter().map(|&n| {
        let t = ty(n);
        quote! {
            if let Err(e) = json.decode_ref::<#t>() {
                runtime::violation(out, #path, e.to_string());
            }
        }
    });
    let values = value_checks(c, path);
    let unevaluated = c.unevaluated.as_ref().map(|(ev, leftover)| unevaluated_checks(ev, *leftover, &ty, path));
    let unevaluated_items =
        c.unevaluated_items.as_ref().map(|(ev, leftover)| unevaluated_item_checks(ev, *leftover, &ty, path));
    let names = names_schema_checks(c, &ty, path);
    let contains = json_contains_checks(c, &ty, path);
    let required =
        (!c.json_dependents.is_empty()).then(|| object_dependent_checks(&c.json_dependents, &quote!(json), path));
    // The checked type binds `json` to its whole value. A value further in reads its own.
    let own = own_buffer(value);
    quote! {
        {
            #own
            #(#nots)*
            #(#dependents)*
            #conditions
            #branches
            #patterns
            #tuple
            #(#also)*
            #values
            #unevaluated
            #unevaluated_items
            #names
            #contains
            #required
        }
    }
}

/// Returns the binding of `json` to `value` (an expression of type `&T`) as a buffer, or `None` when `value` is the
/// whole value of the checked type, which binds it already.
fn own_buffer(value: &TokenStream) -> Option<TokenStream> {
    (value.to_string() != "& value").then(|| {
        quote! {
            let json = runtime::to_value(#value).unwrap_or_default();
            let json = &runtime::Content::of(&json);
        }
    })
}

/// Returns `contains` of `c` on the items of `value` (a `&` of a list) at `path`: it counts the items of its buffer that
/// decode as the type of the schema, and holds the count to `minContains` and `maxContains`.
fn contains_checks(space: &Space<'_>, c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let Some((n, min, max)) = c.contains else { return TokenStream::new() };
    let ty = super::syntax::ty(space, &space.types.subschemas[n].0, false);
    let (own, bounds) = (own_buffer(value), contains_bounds(min, max, path));
    quote! {
        {
            #own
            let items = json.as_array().unwrap_or_default();
            let hits = items.iter().filter(|item| item.decode_ref::<#ty>().is_ok()).count();
            #bounds
        }
    }
}

/// Returns `contains` of a schema without `type` on the array in `json`, as [`contains_checks`] on a list.
fn json_contains_checks(c: &Constraints, ty: &dyn Fn(usize) -> TokenStream, path: &TokenStream) -> TokenStream {
    let Some((n, min, max)) = c.json_contains else { return TokenStream::new() };
    let (t, bounds) = (ty(n), contains_bounds(min, max, path));
    quote! {
        if let Some(items) = json.as_array() {
            let hits = items.iter().filter(|item| item.decode_ref::<#t>().is_ok()).count();
            #bounds
        }
    }
}

/// Returns the checks of `hits`, the items that match `contains`, against `minContains` and `maxContains`.
fn contains_bounds(min: u64, max: Option<u64>, path: &TokenStream) -> TokenStream {
    let at_least = (min > 0).then(|| {
        let min = Literal::u64_unsuffixed(min);
        quote! {
            if hits < #min {
                runtime::violation(out, #path, format!("must contain at least {} matching item(s), found {hits}", #min));
            }
        }
    });
    let at_most = max.map(|max| {
        let max = Literal::u64_unsuffixed(max);
        quote! {
            if hits > #max {
                runtime::violation(out, #path, format!("contains {hits} matching items, more than allowed"));
            }
        }
    });
    quote!(#at_least #at_most)
}

/// Returns `patternProperties` of `c` on the object in `json`: the value of each name that a pattern matches decodes as
/// the type of its schema. With `additionalProperties: false`, a name that is neither declared nor matched is refused.
fn pattern_checks(c: &Constraints, ty: &dyn Fn(usize) -> TokenStream, path: &TokenStream) -> TokenStream {
    if c.pattern_properties.is_empty() && c.closed_to.is_none() {
        return TokenStream::new();
    }
    let closed = c.closed_to.is_some();
    let patterns = c.pattern_properties.iter().map(|(pattern, n)| {
        let (schema, pattern) = (ty(*n), lit(pattern));
        let matches = quote!(runtime::pattern_matches({ static RE: runtime::Pattern = runtime::Pattern::new(); &RE }, #pattern, name));
        let refuse = quote!(runtime::violation(out, &runtime::InstancePath::Key(#path, name), e.to_string()););
        // Without `additionalProperties` nothing else happens on a match: one `if` holds both conditions.
        if closed {
            quote! {
                if #matches {
                    matched = true;
                    if let Err(e) = item.decode_ref::<#schema>() { #refuse }
                }
            }
        } else {
            quote!(if #matches && let Err(e) = item.decode_ref::<#schema>() { #refuse })
        }
    });
    let (start, refuse) = match &c.closed_to {
        Some((declared, extra)) => {
            // `&& !declared.contains(name)`, or nothing when no name is declared.
            let undeclared = (!declared.is_empty()).then(|| {
                let declared = declared.iter().map(|d| lit(d));
                quote!(&& ![#(#declared),*].contains(&name))
            });
            let other = match extra {
                None => quote! {
                    if !matched #undeclared {
                        runtime::violation(out, &runtime::InstancePath::Key(#path, name), "no value is allowed here");
                    }
                },
                Some(n) => {
                    let t = ty(*n);
                    quote! {
                        if !matched
                            #undeclared
                            && let Err(e) = item.decode_ref::<#t>()
                        {
                            runtime::violation(out, &runtime::InstancePath::Key(#path, name), e.to_string());
                        }
                    }
                }
            };
            (Some(quote!(let mut matched = false;)), Some(other))
        }
        None => (None, None),
    };
    quote! {
        if let Some(object) = json.as_object() {
            for (name, item) in object {
                #start
                #(#patterns)*
                #refuse
            }
        }
    }
}

/// Returns `unevaluatedProperties` on the object in `json`: each name that `ev` does not evaluate is refused, or decodes
/// as the type of the schema of `leftover`. A branch that evaluates names decodes the value once, before the names.
fn unevaluated_checks(
    ev: &Evaluated,
    leftover: Leftover,
    ty: &dyn Fn(usize) -> TokenStream,
    path: &TokenStream,
) -> TokenStream {
    // A schema that evaluates every name leaves none.
    if ev.all {
        return TokenStream::new();
    }
    let mut flags = vec![];
    let evaluated = evaluated_expr(ev, ty, &mut flags);
    let (binding, check) = match leftover {
        Leftover::Refused => (
            quote!(name),
            quote!(runtime::violation(out, &runtime::InstancePath::Key(#path, name), "is not allowed (unevaluatedProperties)");),
        ),
        Leftover::Schema(n) => {
            let t = ty(n);
            (
                quote!((name, item)),
                quote! {
                    if let Err(e) = item.decode_ref::<#t>() {
                        runtime::violation(out, &runtime::InstancePath::Key(#path, name), e.to_string());
                    }
                },
            )
        }
    };
    let names = match leftover {
        Leftover::Refused => quote!(object.map(|(name, _)| name)),
        Leftover::Schema(_) => quote!(object),
    };
    let body = match evaluated {
        Some(evaluated) => quote!(if !(#evaluated) { #check }),
        None => check,
    };
    quote! {
        if let Some(object) = json.as_object() {
            #(#flags)*
            for #binding in #names {
                #body
            }
        }
    }
}

/// Returns `unevaluatedItems` on the list in `json`: each item that `ev` does not evaluate is refused, or decodes as the
/// type of the schema of `leftover`.
fn unevaluated_item_checks(
    ev: &EvaluatedItems,
    leftover: Leftover,
    ty: &dyn Fn(usize) -> TokenStream,
    path: &TokenStream,
) -> TokenStream {
    if ev.all {
        return TokenStream::new();
    }
    let mut flags = vec![];
    let evaluated = evaluated_item_expr(ev, ty, &mut flags);
    let check = match leftover {
        Leftover::Refused => {
            quote!(runtime::violation(out, &runtime::InstancePath::Index(#path, i), "is not allowed (unevaluatedItems)");)
        }
        Leftover::Schema(n) => {
            let t = ty(n);
            quote! {
                if let Err(e) = item.decode_ref::<#t>() {
                    runtime::violation(out, &runtime::InstancePath::Index(#path, i), e.to_string());
                }
            }
        }
    };
    // Only `prefixItems` evaluated: the items after them are the rest.
    let (items, evaluated) = if ev.contains.is_empty() && ev.branches.is_empty() && ev.prefix > 0 {
        let prefix = Literal::usize_unsuffixed(ev.prefix);
        (quote!(items.iter().enumerate().skip(#prefix)), None)
    } else {
        (quote!(items.iter().enumerate()), evaluated)
    };
    let body = match &evaluated {
        Some(evaluated) => quote!(if !(#evaluated) { #check }),
        None => check,
    };
    // The item is bound only where a decode reads it.
    let uses_item =
        matches!(leftover, Leftover::Schema(_)) || evaluated.is_some_and(|e| e.to_string().contains("item"));
    let binding = if uses_item { quote!((i, item)) } else { quote!((i, _)) };
    quote! {
        if let Some(items) = json.as_array() {
            #(#flags)*
            for #binding in #items {
                #body
            }
        }
    }
}

/// Returns whether item `i` (`item`) is evaluated by `ev`, as an expression, or `None` when no item is. Each branch adds
/// its decode, run once, to `flags`.
fn evaluated_item_expr(
    ev: &EvaluatedItems,
    ty: &dyn Fn(usize) -> TokenStream,
    flags: &mut Vec<TokenStream>,
) -> Option<TokenStream> {
    if ev.all {
        return Some(quote!(true));
    }
    let mut parts = vec![];
    if ev.prefix > 0 {
        let prefix = Literal::usize_unsuffixed(ev.prefix);
        parts.push(quote!(i < #prefix));
    }
    for n in &ev.contains {
        let t = ty(*n);
        parts.push(quote!(item.decode_ref::<#t>().is_ok()));
    }
    for (n, decodes, branch) in &ev.branches {
        let Some(inner) = evaluated_item_expr(branch, ty, flags) else { continue };
        let flag = quote::format_ident!("matched_{}", flags.len());
        let t = ty(*n);
        flags.push(quote!(let #flag = json.decode_ref::<#t>().is_ok();));
        let flag = if *decodes { quote!(#flag) } else { quote!(!#flag) };
        parts.push(if branch.all { flag } else { quote!((#flag && (#inner))) });
    }
    either(&parts)
}

/// Returns `parts` joined by `||`, or `None` when there are none.
fn either(parts: &[TokenStream]) -> Option<TokenStream> {
    (!parts.is_empty()).then(|| quote!(#(#parts)||*))
}

/// Returns whether `name` is evaluated by `ev`, as an expression, or `None` when no name is. Each branch adds its
/// decode, run once, to `flags`.
fn evaluated_expr(
    ev: &Evaluated,
    ty: &dyn Fn(usize) -> TokenStream,
    flags: &mut Vec<TokenStream>,
) -> Option<TokenStream> {
    if ev.all {
        return Some(quote!(true));
    }
    let mut parts = vec![];
    if !ev.names.is_empty() {
        let names = ev.names.iter().map(|n| lit(n));
        parts.push(quote!(matches!(name, #(#names)|*)));
    }
    for p in &ev.patterns {
        let p = lit(p);
        parts.push(
            quote!(runtime::pattern_matches({ static RE: runtime::Pattern = runtime::Pattern::new(); &RE }, #p, name)),
        );
    }
    for (n, decodes, branch) in &ev.branches {
        let Some(inner) = evaluated_expr(branch, ty, flags) else { continue };
        let flag = quote::format_ident!("matched_{}", flags.len());
        let t = ty(*n);
        flags.push(quote!(let #flag = json.decode_ref::<#t>().is_ok();));
        let flag = if *decodes { quote!(#flag) } else { quote!(!#flag) };
        parts.push(if branch.all { flag } else { quote!((#flag && (#inner))) });
    }
    either(&parts)
}

/// Returns the checks of a value that no keyword needs to look at, or `None`. A type of yours keeps its own invariants
/// (see `live_types`): the schema where it is used is not checked either. `false` refuses a value that decoded; an
/// optional one that is absent passes, so `Option` looks inside first.
fn settled_checks(space: &Space<'_>, t: &Ty, c: &Constraints, path: &TokenStream) -> Option<TokenStream> {
    if space.is_external(t) {
        return Some(TokenStream::new());
    }
    (c.never && !matches!(t, Ty::Option(_) | Ty::Indirect(_)))
        .then(|| quote!(runtime::violation(out, #path, "no value is allowed here");))
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
    if let Some(settled) = settled_checks(space, t, c, path) {
        return settled;
    }
    // A Rust tuple holds its items by their types.
    let c = &if crate::ir::checked::rust_tuple(space.types, t, 0) {
        Constraints { tuple: None, ..c.clone() }
    } else {
        c.clone()
    };
    let json = c.json_checks();
    if !json.is_empty() {
        let held = crate::ir::checked::json_held_by_type(space.types, t, &json, 0);
        let typed = (!held).then(|| subschema_checks(space, c, value, path));
        let rest = checks(space, t, &c.without_json(), value, path);
        return quote!(#typed #rest);
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
            let contains = contains_checks(space, c, value, path);
            quote!(#len #items #contains)
        }
        // The length is the type's: only uniqueness, `contains` and the items are checked.
        Ty::Array(x, _) => {
            let items = item_checks(space, x, c, value, path);
            let contains = contains_checks(space, c, value, path);
            quote!(#items #contains)
        }
        // A value of a named type in the map checks itself where it decodes.
        Ty::Map(_) => map_checks(c, value, path),
        Ty::Named(id) => match &space.types.get(*id).kind {
            Kind::Alias(x) => checks(space, x, c, value, path),
            // The fields of a struct hold `dependentRequired` together: the checked type of its shape runs it.
            Kind::Struct(s)
                if !c.dependent_required.is_empty()
                    || c.property_names.is_some()
                    || c.min_properties.is_some()
                    || c.max_properties.is_some() =>
            {
                let names = c.property_names.as_deref().map(|n| struct_name_checks(space, s, n, value, path));
                let dependents = dependent_checks(space, s, c, value, path);
                let count = struct_count_checks(space, s, c, value, path);
                quote!(#names #dependents #count)
            }
            // The union of an implied shape (`Object` or `Array`, and `Other`): keywords of one kind beside the shape and
            // the names of an object, on the value by its kind.
            Kind::Union(u) if u.rest || c.property_names.is_some() => implied_checks(c, value, path),
            // A checked type holds its own keywords: other ones beside them are checked on its base.
            Kind::Checked(own) if own.constraints != *c => {
                checks(space, &own.base, c, &quote!(std::ops::Deref::deref(#value)), path)
            }
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
        Ty::Any => any_checks(c, value, path),
        Ty::Bool | Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Unit | Ty::Tuple(_) => TokenStream::new(),
    }
}

/// Returns `propertyNames` on the name in `name`, at the path of that name.
fn name_check(names: &Constraints, path: &TokenStream) -> TokenStream {
    string_checks(names, &quote!(name), &quote!(&runtime::InstancePath::Key(#path, name)))
}

/// Returns the keywords of one kind beside an implied shape (`required` beside `properties`, `minimum` beside
/// `items`) and `propertyNames`, checked on the JSON value of the union by its kind.
fn implied_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let by_kind = any_checks(c, &quote!(&json), path);
    let names = c.property_names.as_deref().map(|n| {
        let names = name_checks(n, &quote!(object.keys()), path);
        quote!(if let serde_json::Value::Object(object) = &json { #names })
    });
    if by_kind.is_empty() && names.is_none() {
        return TokenStream::new();
    }
    quote! {
        {
            let json = runtime::to_value(#value).unwrap_or_default();
            #by_kind
            #names
        }
    }
}

/// Returns `propertyNames` on each name that `keys` (an iterator of `&String`) yields.
fn name_checks(names: &Constraints, keys: &TokenStream, path: &TokenStream) -> TokenStream {
    let check = name_check(names, path);
    quote! {
        for name in #keys {
            #check
        }
    }
}

/// Returns `propertyNames` on the names that a value of `shape` has: each declared property that is present and each
/// name of its map of other properties.
fn struct_name_checks(
    space: &Space<'_>,
    shape: &crate::ir::types::Struct,
    names: &Constraints,
    value: &TokenStream,
    path: &TokenStream,
) -> TokenStream {
    let r = receiver(value);
    let declared = shape.fields.iter().map(|f| {
        let wire = lit(&f.wire);
        let n = super::syntax::snake_ident(&f.name);
        let present = if f.required {
            None
        } else if space.types.nullable(&f.ty).is_some() {
            Some(quote!(!#r.#n.is_absent()))
        } else {
            Some(quote!(#r.#n.is_some()))
        };
        let check = name_check(names, path);
        let check = quote! {{
            let name = #wire;
            #check
        }};
        match present {
            Some(present) => quote!(if #present { #check }),
            None => check,
        }
    });
    let extra = shape.additional.is_some().then(|| {
        let field = super::syntax::ident(&super::syntax::extra_field(shape));
        name_checks(names, &quote!(#r.#field.keys()), path)
    });
    let patterns = (0..shape.patterns.len()).map(|i| {
        let field = super::syntax::pattern_field(shape, i);
        name_checks(names, &quote!(#r.#field.keys()), path)
    });
    quote!(#(#declared)* #extra #(#patterns)*)
}

/// Returns `minProperties` and `maxProperties` on a value of `shape` (`value` is a `&` of it): its declared properties
/// that are present, and the names that it does not declare.
fn struct_count_checks(
    space: &Space<'_>,
    shape: &crate::ir::types::Struct,
    c: &Constraints,
    value: &TokenStream,
    path: &TokenStream,
) -> Option<TokenStream> {
    if c.min_properties.is_none() && c.max_properties.is_none() {
        return None;
    }
    let r = receiver(value);
    // The required ones are always present: one number for all of them.
    let required = shape.fields.iter().filter(|f| f.required).count();
    let optional = shape.fields.iter().filter(|f| !f.required).map(|f| {
        let n = super::syntax::snake_ident(&f.name);
        if space.types.nullable(&f.ty).is_some() {
            quote!(usize::from(!#r.#n.is_absent()))
        } else {
            quote!(usize::from(#r.#n.is_some()))
        }
    });
    let maps =
        (shape.additional.is_some().then(|| super::syntax::ident(&super::syntax::extra_field(shape))).into_iter())
            .chain((0..shape.patterns.len()).map(|i| super::syntax::pattern_field(shape, i)))
            .map(|field| quote!(#r.#field.len()));
    let terms: Vec<TokenStream> = (required > 0)
        .then(|| {
            let n = Literal::usize_unsuffixed(required);
            quote!(#n)
        })
        .into_iter()
        .chain(optional)
        .chain(maps)
        .collect();
    let count = if terms.is_empty() { quote!(0) } else { quote!(#(#terms)+*) };
    let (min, max) = (opt_u64(c.min_properties), opt_u64(c.max_properties));
    Some(quote! {
        runtime::check_len(out, #path, #count, #min, #max, "properties");
    })
}

/// Returns `dependentRequired` on the fields of `shape` (`value` is a `&` of it): when a property is present, so are the
/// properties that it requires.
fn dependent_checks(
    space: &Space<'_>,
    shape: &crate::ir::types::Struct,
    c: &Constraints,
    value: &TokenStream,
    path: &TokenStream,
) -> TokenStream {
    let r = receiver(value);
    // How the property `wire` shows that it is absent: `None` for a required field, which always is present. The IR
    // keeps no name whose absence a default hides.
    let absent = |wire: &str| -> Option<(TokenStream, TokenStream)> {
        let f = shape.fields.iter().find(|f| f.wire == wire).filter(|f| !f.required)?;
        let n = super::syntax::snake_ident(&f.name);
        Some(if space.types.nullable(&f.ty).is_some() {
            (quote!(#r.#n.is_absent()), quote!(!#r.#n.is_absent()))
        } else {
            (quote!(#r.#n.is_none()), quote!(#r.#n.is_some()))
        })
    };
    let checks = c.dependent_required.iter().flat_map(|(name, needs)| {
        let when = absent(name);
        needs.iter().filter_map(move |need| {
            // A required property is always there: nothing to check.
            let (_, has) = absent(need)?;
            let message = lit(&format!("`{need}` is required when `{name}` is present"));
            let ok = match &when {
                Some((missing, _)) => quote!(#missing || #has),
                None => has,
            };
            Some(quote!(runtime::check(out, #path, #ok, #message);))
        })
    });
    quote!(#(#checks)*)
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
                none.get_or_insert_with(|| {
                    lit(&format!("`{keyword}: {given}` leaves no value of this integer type, not even {{got}}"))
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
        return quote!({
            let got = #val;
            runtime::violation(out, #path, format!(#msg));
        });
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

/// Returns the range and `multipleOf` of a `Decimal` value (an `&Decimal` expression), checked exactly on its digits,
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
    // `format: decimal128`: the digits that IEEE 754 decimal128 holds.
    let limits = c.decimal128.then(|| quote!(runtime::check_decimal128(out, #path, #r.as_str());));
    quote!(#range #multiple #limits)
}

/// Checks of a `serde_json::Value` (`value` is a `&` of one): each keyword on values of its kind only, as JSON Schema
/// applies `minimum` to numbers and `minLength` to strings.
fn any_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let format = c.format.as_deref().map(|f| {
        let f = lit(f);
        quote!(runtime::check_format(out, #path, text, #f);)
    });
    let text = string_checks(c, &quote!(text.as_str()), path);
    let text = quote!(#text #format);
    let number = decimal_checks(c, &quote!(&text), path);
    let count = |min: Option<u64>, max: Option<u64>, unit: &str| {
        (min.is_some() || max.is_some()).then(|| {
            let (min, max) = (opt_u64(min), opt_u64(max));
            quote!(runtime::check_len(out, #path, n.len(), #min, #max, #unit);)
        })
    };
    let unique = c.unique_items.then(|| quote!(runtime::check_unique(out, #path, n);));
    let items = count(c.min_items, c.max_items, "items");
    let items = (items.is_some() || unique.is_some()).then(|| quote!(#items #unique));
    let required = required_checks(c, &quote!(|name| n.contains_key(name)), path);
    let properties = count(c.min_properties, c.max_properties, "properties");
    let properties = (properties.is_some() || !required.is_empty()).then(|| quote!(#properties #required));
    // Each kind as (its pattern, its checks): a kind without checks passes.
    let kinds = [
        (
            quote!(serde_json::Value::String(text)),
            quote!(serde_json::Value::String(_)),
            (!text.is_empty()).then_some(text),
        ),
        (
            quote!(serde_json::Value::Number(n)),
            quote!(serde_json::Value::Number(_)),
            (!number.is_empty()).then(|| quote!(let text = n.to_string(); #number)),
        ),
        (quote!(serde_json::Value::Array(n)), quote!(serde_json::Value::Array(_)), items),
        (quote!(serde_json::Value::Object(n)), quote!(serde_json::Value::Object(_)), properties),
    ];
    let checked: Vec<(&TokenStream, &TokenStream)> =
        kinds.iter().filter_map(|(bound, _, checks)| checks.as_ref().map(|c| (bound, c))).collect();
    match checked.as_slice() {
        [] => TokenStream::new(),
        [(pattern, checks)] => quote!(if let #pattern = #value { #checks }),
        _ => {
            let arms = checked.iter().map(|(pattern, checks)| quote!(#pattern => { #checks }));
            let passed = kinds.iter().filter(|(_, _, checks)| checks.is_none()).map(|(_, unbound, _)| unbound);
            quote! {
                match #value {
                    #(#arms)*
                    serde_json::Value::Null | serde_json::Value::Bool(_) #(| #passed)* => {}
                }
            }
        }
    }
}

/// Bounds past this have no whole `i128` worth writing: they are left unchecked.
const MAX_WHOLE: f64 = 1e30;

/// Returns `required` of `c` on an object whose names `has` (a closure from `&str` to `bool`) looks up.
fn required_checks(c: &Constraints, has: &TokenStream, path: &TokenStream) -> TokenStream {
    if c.required.is_empty() {
        return TokenStream::new();
    }
    let names = c.required.iter().map(|n| lit(n));
    quote!(runtime::check_required(out, #path, #has, &[#(#names),*]);)
}

/// Returns the checks of `c`'s tuple on the list in `json`: each leading item decodes as the type of its schema, then
/// the rest as `items` says.
fn tuple_checks(c: &Constraints, ty: &dyn Fn(usize) -> TokenStream, path: &TokenStream) -> TokenStream {
    let Some((prefix, rest)) = &c.tuple else { return TokenStream::new() };
    let decodes = |n: usize, at: &TokenStream| {
        let t = ty(n);
        quote! {
            if let Err(e) = item.decode_ref::<#t>() {
                runtime::violation(out, &runtime::InstancePath::Index(#path, #at), e.to_string());
            }
        }
    };
    let leading = prefix.iter().enumerate().map(|(i, &n)| {
        let item = if i == 0 { quote!(items.first()) } else { quote!(items.get(#i)) };
        let (i, t) = (Literal::usize_unsuffixed(i), ty(n));
        quote! {
            if let Some(item) = #item
                && let Err(e) = item.decode_ref::<#t>()
            {
                runtime::violation(out, &runtime::InstancePath::Index(#path, #i), e.to_string());
            }
        }
    });
    let len = Literal::usize_unsuffixed(prefix.len());
    let rest = match rest {
        Rest::Any => None,
        Rest::Refused => Some(quote! {
            if items.len() > #len {
                runtime::violation(out, &runtime::InstancePath::Index(#path, #len), "no value is allowed here");
            }
        }),
        Rest::Schema(n) => {
            let check = decodes(*n, &quote!(i));
            Some(quote!(for (i, item) in items.iter().enumerate().skip(#len) { #check }))
        }
    };
    // A value of another kind has no items to check.
    quote! {
        let items = json.as_array().unwrap_or_default();
        #(#leading)*
        #rest
    }
}

/// Returns the checks of `c` on the map `value`: its size, its names and the names it must have.
fn map_checks(c: &Constraints, value: &TokenStream, path: &TokenStream) -> TokenStream {
    let r = receiver(value);
    let len = (c.min_properties.is_some() || c.max_properties.is_some()).then(|| {
        let (min, max) = (opt_u64(c.min_properties), opt_u64(c.max_properties));
        quote!(runtime::check_len(out, #path, #r.len(), #min, #max, "properties");)
    });
    let names = c.property_names.as_deref().map(|n| name_checks(n, &quote!(#r.keys()), path));
    let required = required_checks(c, &quote!(|name| #r.contains_key(name)), path);
    let dependents = object_dependent_checks(&c.dependent_required, &r, path);
    quote!(#len #names #required #dependents)
}

/// Returns `dependentRequired` on the map or JSON object `object`: when a name is present, each that it needs is.
fn object_dependent_checks(
    dependents: &[(String, Vec<String>)],
    object: &TokenStream,
    path: &TokenStream,
) -> TokenStream {
    let checks = dependents.iter().map(|(name, needs)| {
        let needs = needs.iter().map(|need| {
            let (message, need) = (lit(&format!("`{need}` is required when `{name}` is present")), lit(need));
            quote!(runtime::check(out, #path, #object.contains_key(#need), #message);)
        });
        let name = lit(name);
        quote!(if #object.contains_key(#name) { #(#needs)* })
    });
    quote!(#(#checks)*)
}

/// Returns `enum` and `const` of `c` on `json` when no type holds their values: the value is compared as JSON.
fn value_checks(c: &Constraints, path: &TokenStream) -> TokenStream {
    let one_of_values = c.enum_values.as_ref().map(|values| {
        let values = values.iter().map(super::syntax::json_tokens);
        quote! {
            static VALUES: std::sync::OnceLock<Vec<serde_json::Value>> = std::sync::OnceLock::new();
            if !VALUES.get_or_init(|| vec![#(#values),*]).iter().any(|v| runtime::content_equal(json, v)) {
                runtime::violation(out, #path, "is not one of the allowed values");
            }
        }
    });
    let constant = c.const_value.as_ref().map(|v| {
        let (message, v) = (lit(&format!("must be {v}")), super::syntax::json_tokens(v));
        quote! {
            static VALUE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
            if !runtime::content_equal(json, VALUE.get_or_init(|| #v)) {
                runtime::violation(out, #path, #message);
            }
        }
    });
    quote!(#one_of_values #constant)
}

/// Returns each `if` of `c` on `json`: a value that decodes as the type of `if` decodes as that of `then`, any other as
/// that of `else`.
fn condition_checks(c: &Constraints, ty: &dyn Fn(usize) -> TokenStream, path: &TokenStream) -> TokenStream {
    let conditions = c.conditions.iter().map(|&(cond, then, otherwise)| {
        // Each branch as (the `let` that its error binds, the violation).
        let branch = |b: usize, name: &str| {
            let (b, name) = (ty(b), lit(&format!("`{name}`: ")));
            (
                quote!(let Err(e) = json.decode_ref::<#b>()),
                quote!(runtime::violation(out, #path, format!("{}{e}", #name));),
            )
        };
        let cond = ty(cond);
        let matched = quote!(json.decode_ref::<#cond>().is_ok());
        match (then.map(|b| branch(b, "then")), otherwise.map(|b| branch(b, "else"))) {
            (Some((then, v)), Some((otherwise, w))) => quote! {
                if #matched {
                    if #then { #v }
                } else if #otherwise {
                    #w
                }
            },
            (Some((then, v)), None) => quote!(if #matched && #then { #v }),
            (None, Some((otherwise, w))) => quote!(if !#matched && #otherwise { #w }),
            (None, None) => TokenStream::new(),
        }
    });
    quote!(#(#conditions)*)
}

/// Returns `propertyNames` of `c` when no text check holds it: each name of the object in `json`, as a JSON string,
/// decodes as the type of the schema.
fn names_schema_checks(c: &Constraints, ty: &dyn Fn(usize) -> TokenStream, path: &TokenStream) -> TokenStream {
    let Some(n) = c.names_schema else { return TokenStream::new() };
    let t = ty(n);
    quote! {
        if let Some(object) = json.as_object() {
            for (name, _) in object {
                if runtime::Content::Str(name.into()).decode_ref::<#t>().is_err() {
                    runtime::violation(out, &runtime::InstancePath::Key(#path, name), "is not an allowed property name");
                }
            }
        }
    }
}

/// Returns the counts of the branches of a union that its type cannot judge alone, on `json`: exactly one of `oneOf`,
/// one at least of `anyOf`.
fn branch_count_checks(c: &Constraints, ty: &dyn Fn(usize) -> TokenStream, path: &TokenStream) -> TokenStream {
    let one_of = (!c.one_of.is_empty()).then(|| {
        let branches = c.one_of.iter().map(|&n| ty(n));
        quote! {
            let hits = [#(json.decode_ref::<#branches>().is_ok()),*]
                .into_iter()
                .filter(|hit| *hit)
                .count();
            if hits != 1 {
                runtime::violation(out, #path, format!("matches {hits} of `oneOf`, not exactly one"));
            }
        }
    });
    // One branch: the value fails it. Several: it decodes as none.
    let any_of = match c.any_of.as_slice() {
        [] => None,
        [n] => {
            let t = ty(*n);
            Some(quote! {
                if json.decode_ref::<#t>().is_err() {
                    runtime::violation(out, #path, "matches none of `anyOf`");
                }
            })
        }
        branches => {
            let branches = branches.iter().map(|&n| ty(n));
            Some(quote! {
                if !(#(json.decode_ref::<#branches>().is_ok())||*) {
                    runtime::violation(out, #path, "matches none of `anyOf`");
                }
            })
        }
    };
    quote!(#one_of #any_of)
}
