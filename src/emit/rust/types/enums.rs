//! Enums and unions: the types that contain one of several values.

use proc_macro2::TokenStream;
use quote::quote;

use super::super::syntax::{doc_opt, has_float, holds, ident, lit, ty};

use super::defaults::field_default;
use crate::emit::rust::names::pascal;
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::pascal_ident;
use crate::ir::types::{EnumValue, JsonKind, Kind, Ty, TypeDef, Union, UnionMode};

pub(super) fn str_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[EnumValue]) {
    if def.lenient {
        return open_str_enum(items, def, values);
    }
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let variants = values.iter().map(|v| {
        let d = doc_opt(v.doc.as_deref());

        let name = pascal_ident(&v.name);
        quote!(#d #name,)
    });
    // By value: the impls do not depend on the order of the variants, and a reorder of the description changes nothing.
    let mut by_wire: Vec<_> = values.iter().collect();
    by_wire.sort_by(|a, b| a.wire.cmp(&b.wire));
    let names: Vec<_> = by_wire.iter().map(|v| pascal_ident(&v.name)).collect();
    let wires: Vec<_> = by_wire.iter().map(|v| lit(&v.wire)).collect();
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum #n {
            #(#variants)*
        }
        ::honig::str_enum_impls!(#n { #(#names => #wires),* });
    });
}

/// The doc of the variant that a lenient client decodes an unlisted value into.
pub(super) const OTHER_DOC: &str =
    " A value that the description does not list: the server added it. It is sent back as it came.";

/// Emits the string enum of a lenient client's response: a value that it does not list is `Other`, so a value that
/// the server added decodes instead of failing the response. Its request copy is closed.
pub(super) fn open_str_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[EnumValue]) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let variants = values.iter().map(|v| {
        let (d, name) = (doc_opt(v.doc.as_deref()), pascal_ident(&v.name));
        quote!(#d #name,)
    });
    let names: Vec<_> = values.iter().map(|v| pascal_ident(&v.name)).collect();
    let wires: Vec<_> = values.iter().map(|v| lit(&v.wire)).collect();
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum #n {
            #(#variants)*
            #[doc = #OTHER_DOC]
            Other(String),
        }
        impl #n {
            #[must_use]
            pub fn as_str(&self) -> &str {
                match self {
                    #(Self::#names => #wires,)*
                    Self::Other(other) => other,
                }
            }
        }
        impl From<&str> for #n {
            fn from(s: &str) -> Self {
                match s {
                    #(#wires => Self::#names,)*
                    other => Self::Other(other.to_owned()),
                }
            }
        }
        impl std::fmt::Display for #n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl AsRef<str> for #n {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
        impl From<#n> for String {
            fn from(value: #n) -> Self {
                if let #n::Other(other) = value { other } else { value.as_str().to_owned() }
            }
        }
        impl serde::Serialize for #n {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
        impl<'de> serde::Deserialize<'de> for #n {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str> as serde::Deserialize>::deserialize(d)?;
                Ok(Self::from(&*s))
            }
        }
    });
}

/// Does the union have an `Other` variant? Only one that the schema leaves open does: a branch accepts any value
/// (`open`), or a kind that no variant covers is valid (`rest`).
pub(super) const fn has_other(u: &Union) -> bool {
    u.open || u.rest
}

/// Does `Other` contain a member that the server added? Only the response type of a lenient client's tagged union does:
/// its tag matches no variant. A union with a default variant decodes an unknown tag as that variant already.
pub(super) const fn reads_added(def: &TypeDef, u: &Union) -> bool {
    def.lenient && matches!(u.mode, UnionMode::Tagged { default: None, .. })
}

/// Returns the value that no variant of the union accepts: `Other` stores it when the union has one. Else decoding
/// fails.
pub(super) fn no_variant(u: &Union) -> TokenStream {
    if has_other(u) {
        quote!(Ok(Self::Other(v.into_value())))
    } else {
        quote!(Err(serde::de::Error::custom("matches no variant")))
    }
}

/// `cyclic`: a value of the union can contain another one, so an untagged one keeps its decisions ([`untagged`]).
pub(super) fn union(items: &mut Vec<TokenStream>, space: &Space<'_>, cyclic: bool, def: &TypeDef, u: &Union) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let eq = (!u.variants.iter().any(|v| has_float(space, &v.ty))).then(|| quote!(Eq,));
    let variants = u.variants.iter().map(|v| {
        let (name, t) = (pascal_ident(&v.name), ty(space, &v.ty, false));
        quote!(#name(#t),)
    });
    let memo = cyclic && u.mode == UnionMode::Untagged;
    let deserialize = union_deserialize_impl(space, &n, def, u, memo);
    // Only a union that the schema leaves open stores a value that no variant accepts, or a lenient client's member
    // that the server added. A closed one fails to decode it.
    let open = has_other(u) || reads_added(def, u);
    let other = open.then(|| quote!(Other(serde_json::Value),));
    let ser = union_serialize(space, &n, u, open);
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, #eq)]
        pub enum #n {
            #(#variants)*
            #other
        }
        #ser
        #deserialize
    });
}

/// Returns `impl Deserialize`. Tagged unions also implement `::honig::__private::TaggedUnion`. So a
/// discriminator sent first streams straight into the variant.
pub(super) fn union_deserialize_impl(
    space: &Space<'_>,
    n: &proc_macro2::Ident,
    def: &TypeDef,
    u: &Union,
    memo: bool,
) -> TokenStream {
    let de = union_deserialize(space, def, u, memo);
    // A buffered number is an `f64`. So a union that contains a `Decimal` decodes from the JSON text instead. An
    // untagged one pays one parse per branch that it tries. A tagged one parses its tag and then parses the one branch.
    let exact = u.variants.iter().any(|v| space.has_decimal(&v.ty));
    if !exact
        && u.mode == UnionMode::ByKind
        && let Some(m) = by_kind_macro(space, n, u)
    {
        return m;
    }
    let property = match &u.mode {
        UnionMode::Tagged { property, .. } if !exact => property,
        UnionMode::Tagged { .. } | UnionMode::ByKind | UnionMode::Untagged => {
            let buffer = if exact {
                quote!(::honig::__private::Content::exact(d)?)
            } else {
                quote!(<::honig::__private::Content<'de> as serde::Deserialize>::deserialize(d)?)
            };
            if memo {
                return remembered(n, u, &buffer, &de);
            }
            return quote! {
                impl<'de> serde::Deserialize<'de> for #n {
                    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                        let v = #buffer;
                        #de
                    }
                }
            };
        }
    };
    let tagged: Vec<_> = u.variants.iter().filter(|v| !v.tags.is_empty()).collect();
    let all: Vec<_> = tagged.iter().flat_map(|v| v.tags.iter().map(|t| lit(t))).collect();
    let knows = if all.is_empty() { quote!(false) } else { quote!(matches!(tag, #(#all)|*)) };
    let arms = tagged.iter().map(|v| {
        let tags = v.tags.iter().map(|t| lit(t));
        let (name, t) = (pascal_ident(&v.name), ty(space, &v.ty, false));
        let read = match holds(space, &v.ty) {
            Some(h) => quote!(::honig::__private::formats::read::<_, #t, #h>(d)),
            None => quote!(<#t as serde::Deserialize>::deserialize(d)),
        };
        quote!(#(#tags)|* => #read.map(Self::#name),)
    });
    let property = lit(property);
    let tag_arg = if all.is_empty() { quote!(_tag) } else { quote!(tag) };
    let d_arg = if all.is_empty() { quote!(_d) } else { quote!(d) };
    quote! {
        ::honig::tagged_deserialize!(#n);
        impl<'de> ::honig::__private::TaggedUnion<'de> for #n {
            const TAG: &'static str = #property;
            fn knows(tag: &str) -> bool {
                #knows
            }
            fn from_tag<D: serde::Deserializer<'de>>(#tag_arg: &str, #d_arg: D) -> Result<Self, D::Error> {
                match #tag_arg {
                    #(#arms)*
                    _ => Err(serde::de::Error::custom("unknown discriminator")),
                }
            }
            fn from_content(v: ::honig::__private::Content<'de>) -> Result<Self, serde_json::Error> {
                #de
            }
        }
    }
}

/// Returns `impl Deserialize` of an untagged union whose values can contain one another. It decides each value once
/// ([`::honig::__private::Memo`]): a decision made before tries only the branch that won, or fails as it failed.
fn remembered(n: &proc_macro2::Ident, u: &Union, buffer: &TokenStream, de: &TokenStream) -> TokenStream {
    let arms = u.variants.iter().enumerate().map(|(i, v)| {
        let name = pascal_ident(&v.name);
        quote!(Self::#name(_) => #i,)
    });
    let other = has_other(u).then(|| {
        let i = u.variants.len();
        quote!(Self::Other(_) => #i,)
    });
    // Only an open union keeps the value: its `Other` stores it.
    let (value, arg) = if u.open {
        (quote!(::honig::__private::Content<'_>), quote!(v))
    } else {
        (quote!(&::honig::__private::Content<'_>), quote!(&v))
    };
    quote! {
        impl #n {
            /// Decodes `v`. `won`: the branch that a decision made before chose, the only one tried.
            fn decide<E: serde::de::Error>(v: #value, won: Option<usize>) -> Result<Self, E> {
                #de
            }

            /// Returns the index of the branch of `self`.
            const fn branch(&self) -> usize {
                match self {
                    #(#arms)*
                    #other
                }
            }
        }
        impl<'de> serde::Deserialize<'de> for #n {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = #buffer;
                let memo = ::honig::__private::Memo::of::<Self>(&v);
                let decided = memo.decided();
                if let Some(Err(e)) = &decided {
                    return Err(serde::de::Error::custom(e));
                }
                let out = Self::decide(#arg, decided.and_then(Result::ok));
                memo.decide(out.as_ref().map(Self::branch).map_err(ToString::to_string));
                out
            }
        }
    }
}

/// Does the variant type declare the discriminator itself: a required string property, or one whose `const` is `tag`?
/// Then it serializes directly without a buffer.
pub(super) fn declares_tag(space: &Space<'_>, t: &Ty, property: &str, tag: &str) -> bool {
    let t = if let Ty::Indirect(inner) = t { inner } else { t };
    let Ty::Named(id) = t else { return false };
    let holds = |f: &Ty| {
        *f == Ty::String
            || matches!(f, Ty::Named(c) if matches!(&space.types.get(*c).kind, Kind::Const(serde_json::Value::String(v)) if v == tag))
    };
    matches!(&space.types.get(*id).kind, Kind::Struct(s) if s.fields.iter().any(|f| f.wire == property && f.required && holds(&f.ty)))
}

/// Returns `Serialize` of the union `n` through `::honig::union_serialize!`: each variant as its value, and a variant
/// whose struct does not carry its tag with the tag put in.
pub(super) fn union_serialize(space: &Space<'_>, n: &proc_macro2::Ident, u: &Union, other: bool) -> TokenStream {
    let (mut direct, mut tagged) = (vec![], vec![]);
    for v in &u.variants {
        let name = pascal_ident(&v.name);
        match (&u.mode, v.tags.first()) {
            // The struct carries the tag: its value is the caller's to keep right.
            (UnionMode::Tagged { property, .. }, Some(tag)) if !declares_tag(space, &v.ty, property, tag) => {
                tagged.push(quote!(#name => #tag));
            }
            _ => direct.push(name),
        }
    }
    // Sorted: the impls do not depend on the order of the variants, and a reorder of the description changes nothing.
    direct.sort_by_key(ToString::to_string);
    tagged.sort_by_key(ToString::to_string);
    direct.extend(other.then(|| ident("Other")));
    let tagged = match &u.mode {
        UnionMode::Tagged { property, .. } if !tagged.is_empty() => quote!(#property { #(#tagged),* }),
        UnionMode::Tagged { .. } | UnionMode::ByKind | UnionMode::Untagged => TokenStream::new(),
    };
    quote!(::honig::union_serialize!(#n { #(#direct),* } #tagged);)
}

/// Returns the buffered value `v` decoded into variant `name` of type `t`. A library type has its text checked first.
pub(super) fn conv(space: &Space<'_>, name: &str, t: &Ty) -> TokenStream {
    let name = ident(name);
    match holds(space, t) {
        Some(h) => {
            let t = ty(space, t, false);
            quote!(v.decode_into(|c: ::honig::__private::formats::Validated<#t, #h>| Self::#name(c.into_inner())))
        }
        None => quote!(v.decode_into(Self::#name)),
    }
}

/// Returns the buffered value `v` decoded as `t` without consuming it. A library type has its text checked first.
pub(super) fn decode_ref(space: &Space<'_>, t: &Ty) -> TokenStream {
    let rust = ty(space, t, false);
    match holds(space, t) {
        Some(h) => {
            quote!(v.decode_ref::<::honig::__private::formats::Validated<#rust, #h>>().map(::honig::__private::formats::Validated::into_inner))
        }
        None => quote!(v.decode_ref::<#rust>()),
    }
}

pub(super) fn union_deserialize(space: &Space<'_>, def: &TypeDef, u: &Union, memo: bool) -> TokenStream {
    match &u.mode {
        UnionMode::Tagged { property, default } => {
            let property = lit(property);
            // The default variant needs no tag of its own: its tags and unknown ones share the fallback
            // (clippy `match_same_arms`).
            let tagged: Vec<_> = u
                .variants
                .iter()
                .enumerate()
                .filter(|(i, v)| !v.tags.is_empty() && *default != Some(*i))
                .map(|(_, v)| v)
                .collect();
            let picks = tagged.iter().enumerate().map(|(i, v)| {
                let tags = v.tags.iter().map(|t| lit(t));
                let i = proc_macro2::Literal::usize_unsuffixed(i);
                quote!(Some(#(#tags)|*) => #i,)
            });
            let arms = tagged.iter().enumerate().map(|(i, v)| {
                let (i, c) = (proc_macro2::Literal::usize_unsuffixed(i), conv(space, &pascal(&v.name), &v.ty));
                quote!(#i => #c,)
            });
            let fallback = match default {
                Some(i) => conv(space, &pascal(&u.variants[*i].name), &u.variants[*i].ty),
                None => no_variant(u),
            };
            let none = proc_macro2::Literal::usize_unsuffixed(tagged.len());
            // A lenient client decodes a tag that matches no variant as `Other`: the server added the member. A value
            // without a tag still fails.
            let (added_pick, added_arm) = if reads_added(def, u) {
                let added = proc_macro2::Literal::usize_unsuffixed(tagged.len() + 1);
                (quote!(Some(_) => #added,), quote!(#added => Ok(Self::Other(v.into_value())),))
            } else {
                (TokenStream::new(), TokenStream::new())
            };
            quote! {
                let which = match v.tag(#property).as_deref() {
                    #(#picks)*
                    #added_pick
                    _ => #none,
                };
                match which {
                    #(#arms)*
                    #added_arm
                    _ => #fallback,
                }
            }
        }
        UnionMode::ByKind => by_kind(space, u),
        UnionMode::Untagged => untagged(space, u, memo),
    }
}

/// Returns what rules each branch out before it decodes: the JSON kind of the value `value` and the fields of a
/// struct. [`untagged`] names each `fit_<i>` and computes it once.
fn guards(space: &Space<'_>, u: &Union, value: &TokenStream) -> Vec<Option<TokenStream>> {
    (u.variants.iter())
        .map(|v| {
            // serde lets a struct decode an array by position. So a branch of a known kind only sees values of
            // that kind.
            let kind = match space.kind_of(&v.ty) {
                JsonKind::Object => Some(rtkind::K::Object.path()),
                JsonKind::Array => Some(rtkind::K::Array.path()),
                JsonKind::String => Some(rtkind::K::String.path()),
                JsonKind::Bool => Some(rtkind::K::Bool.path()),
                JsonKind::Null | JsonKind::Integer | JsonKind::Number | JsonKind::Any => None,
            }
            .map(|k| quote!(v.kind() == #k));
            let inner = if let Ty::Indirect(i) = &v.ty { i.as_ref() } else { &v.ty };
            match (kind, fits(space, inner, value)) {
                (Some(k), Some(f)) => Some(quote!(#k && #f)),
                (k, f) => k.or(f),
            }
        })
        .collect()
}

/// Tries each branch in the order of the variant names. `anyOf` has no order, so a reorder of the branches in the
/// description changes no decoded value.
/// `memo`: a decision made before names the one branch to try, `won` ([`remembered`]).
pub(super) fn untagged(space: &Space<'_>, u: &Union, memo: bool) -> TokenStream {
    // A remembered closed union borrows the value ([`remembered`]).
    let value = if memo && !u.open { quote!(v) } else { quote!(&v) };
    let guards = guards(space, u, &value);
    let fit = |i: usize| quote::format_ident!("fit_{i}");
    let lets = guards.iter().enumerate().filter_map(|(i, g)| {
        let (name, g) = (fit(i), g.as_ref()?);
        Some(quote!(let #name = #g;))
    });
    // A closed `oneOf` counts its matches: exactly one must accept the value. An open one has a branch that accepts any
    // value, which its whole schema judges instead.
    let counts = u.exclusive && !u.open;
    let mut order: Vec<(usize, &crate::ir::types::Variant)> = u.variants.iter().enumerate().collect();
    order.sort_by_key(|(_, v)| pascal(&v.name));
    let scores = (!counts).then(|| kept_scores(space, &order, &value)).flatten();
    let tries = order.into_iter().map(|(i, v)| {
        let (name, t) = (pascal_ident(&v.name), ty(space, &v.ty, false));
        // An open union always decodes because its `Other` accepts anything. As a branch it matches only through one
        // of its own variants. So the branches after it still get their turn. A boxed one too.
        let (inner, boxed) = if let Ty::Indirect(i) = &v.ty { (i.as_ref(), true) } else { (&v.ty, false) };
        let union = matches!(inner, Ty::Named(id) if {
            let d = space.types.get(*id);
            matches!(&d.kind, Kind::Union(x) if has_other(x) || reads_added(d, x))
        });
        let attempt = if union {
            let it = ty(space, inner, false);
            let x = if boxed { quote!(x.as_ref()) } else { quote!(x) };
            quote! {
                match v.decode_ref::<#t>() {
                    Ok(x) if !matches!(#x, #it::Other(_)) => return Ok(Self::#name(x)),
                    Ok(_) => {}
                    Err(e) => { first.get_or_insert(e.to_string()); }
                }
            }
        } else {
            let decoded = decode_ref(space, &v.ty);
            // `oneOf`: every branch decodes and the matches are counted. The first one is the answer.
            let matched = if counts {
                quote! {{ matches += 1; found.get_or_insert(Self::#name(x)); }}
            } else {
                quote!(return Ok(Self::#name(x)))
            };
            quote! {
                match #decoded {
                    Ok(x) => #matched,
                    Err(e) => { first.get_or_insert(e.to_string()); }
                }
            }
        };
        // Decoding checks every keyword: the first branch that decodes is the answer. A branch whose shape fits but
        // that does not decode keeps its error, so that a closed union says what is wrong with the closest one.
        let won = memo.then(|| quote!(won.is_none_or(|w| w == #i)));
        let f = guards[i].is_some().then(|| fit(i)).map(|f| quote!(#f));
        let condition = match (won, f) {
            (Some(w), Some(f)) => Some(quote!(#w && #f)),
            (Some(c), None) | (None, Some(c)) => Some(c),
            (None, None) => None,
        };
        (condition, attempt)
    });
    // When none decodes, an open union has a branch that accepts any value: `Other` stores it.
    let end = if u.open {
        quote!(Ok(Self::Other(v.into_value())))
    } else {
        quote!(Err(serde::de::Error::custom(first.unwrap_or_else(|| "matches no variant".to_owned()))))
    };
    let (count, counted) = counting(counts);
    let tries = in_order(tries, scores);
    quote! {
        #(#lets)*
        let mut first: Option<String> = None;
        #count
        #tries
        #counted
        #end
    }
}

/// Returns the attempts, each `(condition, attempt)`, in the order of the names, or by `scores` when there are some:
/// the one that keeps the most members first.
fn in_order(
    tries: impl Iterator<Item = (Option<TokenStream>, TokenStream)>,
    scores: Option<Vec<TokenStream>>,
) -> TokenStream {
    let Some(scores) = scores else {
        let each = tries.map(|(c, attempt)| match c {
            Some(c) => quote!(if #c { #attempt }),
            None => quote!({ #attempt }),
        });
        return quote!(#(#each)*);
    };
    let arms = tries.enumerate().map(|(p, (c, attempt))| {
        let p = proc_macro2::Literal::usize_unsuffixed(p);
        let guard = c.map(|c| quote!(if #c));
        quote!(#p #guard => { #attempt })
    });
    let at = (0..scores.len()).map(proc_macro2::Literal::usize_unsuffixed);
    quote! {
        let mut ranked = [#((#scores, #at)),*];
        ranked.sort_by_key(|(kept, _)| ::std::cmp::Reverse(*kept));
        for (_, branch) in ranked {
            match branch { #(#arms)* _ => {} }
        }
    }
}

/// Returns, per branch in `order`, how many members of the value it keeps, when two branches or more are objects. An
/// `anyOf` tries the branch that keeps the most first: a value that matches several loses no member that one of them
/// declares. A tie keeps the order of the names.
fn kept_scores(
    space: &Space<'_>,
    order: &[(usize, &crate::ir::types::Variant)],
    value: &TokenStream,
) -> Option<Vec<TokenStream>> {
    let members: Vec<Option<(Vec<String>, bool)>> =
        order.iter().map(|(_, v)| object_members(space, &v.ty, 0)).collect();
    (members.iter().flatten().count() > 1).then(|| {
        members
            .into_iter()
            .map(|m| match m {
                Some((names, all)) => {
                    let names = names.iter().map(|n| lit(n));
                    quote!(::honig::__private::kept(#value, &[#(#names),*], #all))
                }
                None => quote!(0_usize),
            })
            .collect()
    })
}

/// Returns the names that the object type `t` declares, and whether it keeps every other member too (a map of the
/// rest). `None` for a type that is no struct.
fn object_members(space: &Space<'_>, t: &Ty, hops: usize) -> Option<(Vec<String>, bool)> {
    const MAX_HOPS: usize = 32;
    let t = if let Ty::Indirect(i) = t { i.as_ref() } else { t };
    let Ty::Named(id) = space.types.unalias(t) else { return None };
    match &space.types.get(*id).kind {
        Kind::Struct(s) => {
            let names = s.fields.iter().map(|f| f.wire.clone()).collect();
            Some((names, s.additional.is_some() || !s.patterns.is_empty()))
        }
        Kind::Checked(c) if hops < MAX_HOPS => object_members(space, &c.base, hops + 1),
        Kind::Checked(_)
        | Kind::StrEnum(_)
        | Kind::IntEnum(_)
        | Kind::Union(_)
        | Kind::Alias(_)
        | Kind::Newtype(_)
        | Kind::Const(_)
        | Kind::Pending => None,
    }
}

/// Returns what a `oneOf` that `counts` declares before its branches and decides after them: how many decoded and the
/// first, which is the answer when it is the only one. Empty when it accepts the first branch that decodes.
pub(super) fn counting(counts: bool) -> (TokenStream, TokenStream) {
    if !counts {
        return (TokenStream::new(), TokenStream::new());
    }
    (
        quote!(let mut matches = 0_usize; let mut found: Option<Self> = None;),
        quote! {
            match (matches, found) {
                (1, Some(one)) => return Ok(one),
                (n, Some(_)) => return Err(serde::de::Error::custom(format!("matches {n} of `oneOf`, not exactly one"))),
                (_, None) => {}
            }
        },
    )
}

/// `::honig::__private::fits(v, ...)` of the value `v` (`&v` or a reference) for a struct branch: the fields that must be
/// present and the kinds that each may have.
/// It never refuses a value that the branch decodes: a kind that a type decodes from more than one JSON kind (a
/// `Decimal`, an untyped value, an open union) is not checked, an integer may come as a number, and an optional or
/// nullable field may be absent or `null`. `None` when there is nothing to check.
pub(super) fn fits(space: &Space<'_>, t: &Ty, v: &TokenStream) -> Option<TokenStream> {
    use rtkind::K;
    let Ty::Named(id) = t else { return None };
    let Kind::Struct(s) = &space.types.get(*id).kind else { return None };
    let checks: Vec<TokenStream> = s
        .fields
        .iter()
        .filter_map(|f| {
            let optional = space.types.nullable(&f.ty).is_some();
            let present = f.required && field_default(space, f).is_none() && !optional;
            let mut kinds: Vec<K> = match space.kind_of(&f.ty) {
                _ if reads_several_kinds(space, &f.ty, 0) => vec![],
                JsonKind::Any => vec![],
                JsonKind::Null => vec![K::Null],
                JsonKind::Bool => vec![K::Bool],
                JsonKind::Integer | JsonKind::Number => vec![K::Integer, K::Number],
                JsonKind::String => vec![K::String],
                JsonKind::Array => vec![K::Array],
                JsonKind::Object => vec![K::Object],
            };
            if !kinds.is_empty() && (optional || !f.required) {
                kinds.push(K::Null);
            }
            (present || !kinds.is_empty()).then(|| {
                let (wire, kinds) = (lit(&f.wire), kinds.iter().map(|k| k.path()));
                quote!((#wire, #present, &[#(#kinds),*]))
            })
        })
        .collect();
    (!checks.is_empty()).then(|| quote!(::honig::__private::fits(#v, &[#(#checks),*])))
}

/// Whether `t` decodes from more JSON kinds than [`TypeSpace::kind_of`](crate::ir::types::TypeSpace::kind_of) names: a
/// `Decimal` decodes from a number or a string and bytes decode from an array or a string. Seen through options, boxes,
/// aliases and newtypes.
pub(super) fn reads_several_kinds(space: &Space<'_>, t: &Ty, depth: usize) -> bool {
    const MAX_DEPTH: usize = 32;
    match t {
        _ if depth > MAX_DEPTH => true,
        Ty::Decimal | Ty::Decimal128 | Ty::Bytes => true,
        Ty::Option(x) | Ty::Indirect(x) => reads_several_kinds(space, x, depth + 1),
        Ty::Named(id) => match &space.types.get(*id).kind {
            Kind::Alias(x) | Kind::Newtype(x) => reads_several_kinds(space, x, depth + 1),
            Kind::Struct(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Checked(_)
            | Kind::Const(_)
            | Kind::Union(_)
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
        | Ty::String
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

/// A variant of a union by kind: the JSON kinds that it takes, its name and its type.
type KindArm<'u> = (Vec<rtkind::K>, String, &'u Ty);

/// Returns the variant that each JSON kind decodes as, the first variant of a kind taking it, and the kinds that no
/// variant takes.
fn kind_arms<'u>(space: &Space<'_>, u: &'u Union) -> (Vec<KindArm<'u>>, Vec<rtkind::K>) {
    use rtkind::K;
    let mut kinds: Vec<(JsonKind, String, &Ty)> =
        u.variants.iter().map(|v| (space.kind_of(&v.ty), pascal(&v.name), &v.ty)).collect();
    kinds.sort_by_key(|(k, ..)| *k);
    let has_integer = kinds.iter().any(|(k, ..)| *k == JsonKind::Integer);
    let mut covered: Vec<K> = vec![];
    let mut arms = vec![];
    for (kind, name, vty) in kinds {
        let ks: Vec<K> = match kind {
            JsonKind::Null => vec![K::Null],
            JsonKind::Bool => vec![K::Bool],
            JsonKind::Integer => vec![K::Integer],
            // A number variant also accepts integers when no integer variant exists.
            JsonKind::Number if has_integer => vec![K::Number],
            JsonKind::Number => vec![K::Integer, K::Number],
            JsonKind::String => vec![K::String],
            JsonKind::Array => vec![K::Array],
            JsonKind::Object => vec![K::Object],
            JsonKind::Any => K::ALL.to_vec(),
        };
        let fresh: Vec<K> = ks.into_iter().filter(|k| !covered.contains(k)).collect();
        if !fresh.is_empty() {
            covered.extend(&fresh);
            arms.push((fresh, name, vty));
        }
    }
    let rest = K::ALL.iter().copied().filter(|k| !covered.contains(k)).collect();
    (arms, rest)
}

/// Returns `::honig::by_kind_deserialize!` of the union `n`: each JSON kind decodes as its variant. `None` where a
/// variant checks its text through a library format or the union is open: [`by_kind`] writes those.
fn by_kind_macro(space: &Space<'_>, n: &proc_macro2::Ident, u: &Union) -> Option<TokenStream> {
    let (arms, rest) = kind_arms(space, u);
    if u.open || arms.iter().any(|(_, _, t)| holds(space, t).is_some()) {
        return None;
    }
    let arms = arms.iter().map(|(ks, name, _)| {
        let (ks, v) = (ks.iter().map(|k| k.name()), ident(name));
        quote!(#(#ks)|* => #v)
    });
    let rest = (!rest.is_empty()).then(|| {
        let ks = rest.iter().map(|k| k.name());
        let fallback = if has_other(u) { quote!(Other) } else { quote!(none) };
        quote!(; #(#ks)|* => #fallback)
    });
    Some(quote!(::honig::by_kind_deserialize!(#n { #(#arms),* } #rest);))
}

/// Returns `match v.kind()`: every kind is named and there is no wildcard.
pub(super) fn by_kind(space: &Space<'_>, u: &Union) -> TokenStream {
    let (kinds, rest) = kind_arms(space, u);
    let arms = kinds.iter().map(|(ks, name, vty)| {
        let pats = ks.iter().map(|k| k.path());
        let c = if u.open {
            // An open union has a branch that accepts any value. A value of this kind that does not decode as the
            // variant, which checks its keywords, is that branch's: `Other` stores it.
            let (vn, decoded) = (ident(name), decode_ref(space, vty));
            quote!({
                match #decoded {
                    Ok(x) => Ok(Self::#vn(x)),
                    Err(_) => Ok(Self::Other(v.into_value())),
                }
            })
        } else {
            conv(space, name, vty)
        };
        quote!(#(#pats)|* => #c,)
    });
    let rest: Vec<TokenStream> = rest.iter().map(|k| k.path()).collect();
    let none = no_variant(u);
    let other = (!rest.is_empty()).then(|| quote!(#(#rest)|* => #none,));
    quote! {
        match v.kind() {
            #(#arms)*
            #other
        }
    }
}

/// The JSON kinds of `::honig::__private::Kind` as the emitter spells them.
pub(super) mod rtkind {
    use proc_macro2::TokenStream;
    use quote::quote;

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(super) enum K {
        Null,
        Bool,
        Integer,
        Number,
        String,
        Array,
        Object,
    }

    impl K {
        pub(super) const ALL: [Self; 7] =
            [Self::Null, Self::Bool, Self::Integer, Self::Number, Self::String, Self::Array, Self::Object];

        /// The variant name of `::honig::__private::Kind`.
        pub(super) fn name(self) -> proc_macro2::Ident {
            let path = self.path().to_string();
            quote::format_ident!("{}", path.rsplit(' ').next().unwrap_or_default())
        }

        pub(super) fn path(self) -> TokenStream {
            match self {
                Self::Null => quote!(::honig::__private::Kind::Null),
                Self::Bool => quote!(::honig::__private::Kind::Bool),
                Self::Integer => quote!(::honig::__private::Kind::Integer),
                Self::Number => quote!(::honig::__private::Kind::Number),
                Self::String => quote!(::honig::__private::Kind::String),
                Self::Array => quote!(::honig::__private::Kind::Array),
                Self::Object => quote!(::honig::__private::Kind::Object),
            }
        }
    }
}

/// Returns the message of a value that a closed enum does not list: `{other}` receives the value. Braces in the values
/// are escaped.
pub(super) fn one_of(values: impl Iterator<Item = String>) -> String {
    let allowed = values.map(|v| format!("`{v}`")).collect::<Vec<_>>().join(", ");
    format!("`{{other}}` is not one of {}", allowed.replace('{', "{{").replace('}', "}}"))
}
