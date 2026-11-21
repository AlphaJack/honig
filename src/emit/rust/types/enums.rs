//! Enums and unions: the types that hold one of several values.

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
    let names: Vec<_> = values.iter().map(|v| pascal_ident(&v.name)).collect();
    let wires: Vec<_> = values.iter().map(|v| lit(&v.wire)).collect();
    let allowed = lit(&one_of(values.iter().map(|v| v.wire.clone())));
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum #n {
            #(#variants)*
        }
        impl #n {
            #[must_use]
            pub const fn as_str(&self) -> &'static str {
                match self {
                    #(Self::#names => #wires,)*
                }
            }
        }
        impl TryFrom<&str> for #n {
            type Error = runtime::BuildError;
            fn try_from(s: &str) -> Result<Self, runtime::BuildError> {
                match s {
                    #(#wires => Ok(Self::#names),)*
                    other => Err(runtime::BuildError::value(format!(#allowed))),
                }
            }
        }
        runtime::str_enum_impls!(#n);
    });
}

/// The doc of the variant that a lenient client reads an unlisted value into.
pub(super) const OTHER_DOC: &str =
    " A value that the description does not list: the server added it. It is sent back as it came.";

/// Emits the string enum of a lenient client's response: a value that it does not list is `Other`, so a value that
/// the server added reads instead of failing the response. Its request copy is closed.
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

/// Does the union have an `Other` variant? Only one that the schema leaves open does: a branch takes any value
/// (`open`), or a kind that no variant covers is valid (`rest`).
pub(super) const fn has_other(u: &Union) -> bool {
    u.open || u.rest
}

/// Does `Other` hold a member that the server added? Only the response type of a lenient client's tagged union does:
/// its tag matches no variant. A union with a default variant reads an unknown tag as that variant already.
pub(super) const fn reads_added(def: &TypeDef, u: &Union) -> bool {
    def.lenient && matches!(u.mode, UnionMode::Tagged { default: None, .. })
}

/// Returns the value that no variant of the union takes: `Other` holds it when the union has one. Else decoding fails.
pub(super) fn no_variant(u: &Union) -> TokenStream {
    if has_other(u) {
        quote!(Ok(Self::Other(v.into_value())))
    } else {
        quote!(Err(serde::de::Error::custom("matches no variant")))
    }
}

pub(super) fn union(
    items: &mut Vec<TokenStream>,
    space: &Space<'_>,
    aliases: &std::collections::BTreeMap<usize, usize>,
    def: &TypeDef,
    u: &Union,
) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let eq = (!u.variants.iter().any(|v| has_float(space, &v.ty))).then(|| quote!(Eq,));
    let variants = u.variants.iter().map(|v| {
        let (name, t) = (pascal_ident(&v.name), ty(space, &v.ty, false));
        quote!(#name(#t),)
    });
    let ser = union_serialize(space, aliases, u);
    let deserialize = union_deserialize_impl(space, &n, def, u);
    // Only a union that the schema leaves open holds a value that no variant takes, or a lenient client's member that
    // the server added. A closed one fails to decode it.
    let (other, other_ser) = if has_other(u) || reads_added(def, u) {
        (quote!(Other(serde_json::Value),), quote!(Self::Other(v) => serde::Serialize::serialize(v, s),))
    } else {
        (TokenStream::new(), TokenStream::new())
    };
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, #eq)]
        pub enum #n {
            #(#variants)*
            #other
        }
        impl serde::Serialize for #n {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                match self {
                    #ser
                    #other_ser
                }
            }
        }
        #deserialize
    });
}

/// Returns `impl Deserialize`. Tagged unions also implement `runtime::TaggedUnion`. So a
/// discriminator sent first streams straight into the variant.
pub(super) fn union_deserialize_impl(
    space: &Space<'_>,
    n: &proc_macro2::Ident,
    def: &TypeDef,
    u: &Union,
) -> TokenStream {
    let de = union_deserialize(space, def, u);
    // A buffered number is an `f64`. So a union that holds a `Decimal` decodes from the JSON text instead. An
    // untagged one pays one parse per branch that it tries. A tagged one reads its tag and then parses the one branch.
    let exact = u.variants.iter().any(|v| space.has_decimal(&v.ty));
    let property = match &u.mode {
        UnionMode::Tagged { property, .. } if !exact => property,
        UnionMode::Tagged { .. } | UnionMode::ByKind | UnionMode::Untagged => {
            let buffer = if exact {
                quote!(runtime::Content::exact(d)?)
            } else {
                quote!(<runtime::Content<'de> as serde::Deserialize>::deserialize(d)?)
            };
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
            Some(h) => quote!(runtime::formats::read::<_, #t, #h>(d)),
            None => quote!(<#t as serde::Deserialize>::deserialize(d)),
        };
        quote!(#(#tags)|* => #read.map(Self::#name),)
    });
    let property = lit(property);
    let tag_arg = if all.is_empty() { quote!(_tag) } else { quote!(tag) };
    let d_arg = if all.is_empty() { quote!(_d) } else { quote!(d) };
    quote! {
        impl<'de> serde::Deserialize<'de> for #n {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                runtime::tagged::<D, Self>(d)
            }
        }
        impl<'de> runtime::TaggedUnion<'de> for #n {
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
            fn from_content(v: runtime::Content<'de>) -> Result<Self, serde_json::Error> {
                #de
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

pub(super) fn union_serialize(
    space: &Space<'_>,
    aliases: &std::collections::BTreeMap<usize, usize>,
    u: &Union,
) -> TokenStream {
    let arms = u.variants.iter().map(|v| {
        let (name, direct) = (pascal_ident(&v.name), quote!(serde::Serialize::serialize(v, s)));
        let same = Some(space.resolved(aliases, &v.ty));
        match (&u.mode, v.tags.first()) {
            // The struct carries the tag: its value is the caller's to keep right.
            (UnionMode::Tagged { property, .. }, Some(tag)) if declares_tag(space, &v.ty, property, tag) => {
                (quote!(Self::#name(v)), same, direct)
            }
            (UnionMode::Tagged { property, .. }, Some(tag)) => {
                let (property, tag) = (lit(property), lit(tag));
                let body = quote! {{
                    let mut j = serde_json::to_value(v).map_err(serde::ser::Error::custom)?;
                    if let serde_json::Value::Object(m) = &mut j {
                        m.insert(#property.to_owned(), serde_json::Value::String(#tag.to_owned()));
                    }
                    serde::Serialize::serialize(&j, s)
                }};
                // Its own tag: shared with no other arm.
                (quote!(Self::#name(v)), None, body)
            }
            _ => (quote!(Self::#name(v)), same, direct),
        }
    });
    let arms = super::super::syntax::shared_arms(arms.collect());
    quote!(#(#arms)*)
}

/// Returns the buffered value `v` decoded into variant `name` of type `t`. A library type has its text checked first.
pub(super) fn conv(space: &Space<'_>, name: &str, t: &Ty) -> TokenStream {
    let name = ident(name);
    match holds(space, t) {
        Some(h) => {
            let t = ty(space, t, false);
            quote!(v.decode_into(|c: runtime::formats::Validated<#t, #h>| Self::#name(c.into_inner())))
        }
        None => quote!(v.decode_into(Self::#name)),
    }
}

/// Returns the buffered value `v` decoded as `t` without consuming it. A library type has its text checked first.
pub(super) fn decode_ref(space: &Space<'_>, t: &Ty) -> TokenStream {
    let rust = ty(space, t, false);
    match holds(space, t) {
        Some(h) => {
            quote!(v.decode_ref::<runtime::formats::Validated<#rust, #h>>().map(runtime::formats::Validated::into_inner))
        }
        None => quote!(v.decode_ref::<#rust>()),
    }
}

pub(super) fn union_deserialize(space: &Space<'_>, def: &TypeDef, u: &Union) -> TokenStream {
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
            // A lenient client reads a tag that matches no variant as `Other`: the server added the member. A value
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
        UnionMode::Untagged => untagged(space, u),
    }
}

/// Tries each branch in the order of the variant names. `anyOf` has no order, so a reorder of the branches in the
/// description changes no decoded value.
pub(super) fn untagged(space: &Space<'_>, u: &Union) -> TokenStream {
    // What rules a branch out before it decodes: the JSON kind of the value and the fields of a struct. Each is
    // named `fit_<i>` and computed once.
    let guards: Vec<Option<TokenStream>> = u
        .variants
        .iter()
        .map(|v| {
            // serde lets a struct read an array by position. So a branch of a known kind only sees values of
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
            match (kind, fits(space, inner)) {
                (Some(k), Some(f)) => Some(quote!(#k && #f)),
                (k, f) => k.or(f),
            }
        })
        .collect();
    let fit = |i: usize| quote::format_ident!("fit_{i}");
    let lets = guards.iter().enumerate().filter_map(|(i, g)| {
        let (name, g) = (fit(i), g.as_ref()?);
        Some(quote!(let #name = #g;))
    });
    // A closed `oneOf` counts its matches: exactly one must take the value. An open one has a branch that takes any
    // value, which its whole schema judges instead.
    let counts = u.exclusive && !u.open;
    let mut order: Vec<(usize, &crate::ir::types::Variant)> = u.variants.iter().enumerate().collect();
    order.sort_by_key(|(_, v)| pascal(&v.name));
    let tries = order.into_iter().map(|(i, v)| {
        let (name, t) = (pascal_ident(&v.name), ty(space, &v.ty, false));
        // An open union always decodes because its `Other` takes anything. As a branch it matches only through one
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
        if guards[i].is_some() {
            let f = fit(i);
            quote!(if #f { #attempt })
        } else {
            quote!({ #attempt })
        }
    });
    // When none decodes, an open union has a branch that takes any value: `Other` holds it.
    let end = if u.open {
        quote!(Ok(Self::Other(v.into_value())))
    } else {
        quote!(Err(serde::de::Error::custom(first.unwrap_or_else(|| "matches no variant".to_owned()))))
    };
    let (count, counted) = counting(counts);
    quote! {
        #(#lets)*
        let mut first: Option<String> = None;
        #count
        #(#tries)*
        #counted
        #end
    }
}

/// Returns what a `oneOf` that `counts` declares before its branches and decides after them: how many decoded and the
/// first, which is the answer when it is the only one. Empty when it takes the first branch that decodes.
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

/// `runtime::fits(&v, ...)` for a struct branch: the fields that must be present and the kinds that each may have.
/// It never refuses a value that the branch decodes: a kind that a type reads from more than one JSON kind (a
/// `Decimal`, an untyped value, an open union) is not checked, an integer may come as a number, and an optional or
/// nullable field may be absent or `null`. `None` when there is nothing to check.
pub(super) fn fits(space: &Space<'_>, t: &Ty) -> Option<TokenStream> {
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
    (!checks.is_empty()).then(|| quote!(runtime::fits(&v, &[#(#checks),*])))
}

/// Whether `t` decodes from more JSON kinds than [`TypeSpace::kind_of`](crate::ir::types::TypeSpace::kind_of) names: a `Decimal` reads a number or a string
/// and bytes read an array or a string. Seen through options, boxes, aliases and newtypes.
pub(super) fn reads_several_kinds(space: &Space<'_>, t: &Ty, depth: usize) -> bool {
    const MAX_DEPTH: usize = 32;
    match t {
        _ if depth > MAX_DEPTH => true,
        Ty::Decimal | Ty::Bytes => true,
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

/// Returns `match v.kind()`: every kind is named and there is no wildcard.
pub(super) fn by_kind(space: &Space<'_>, u: &Union) -> TokenStream {
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
            // A number variant also takes integers when no integer variant exists.
            JsonKind::Number if has_integer => vec![K::Number],
            JsonKind::Number => vec![K::Integer, K::Number],
            JsonKind::String => vec![K::String],
            JsonKind::Array => vec![K::Array],
            JsonKind::Object => vec![K::Object],
            JsonKind::Any => K::ALL.to_vec(),
        };
        let fresh: Vec<K> = ks.into_iter().filter(|k| !covered.contains(k)).collect();
        if fresh.is_empty() {
            continue;
        }
        covered.extend(&fresh);
        let pats = fresh.iter().map(|k| k.path());
        let c = if u.open {
            // An open union has a branch that takes any value. A value of this kind that does not decode as the
            // variant, which checks its keywords, is that branch's: `Other` holds it.
            let (vn, decoded) = (ident(&name), decode_ref(space, vty));
            quote!({
                match #decoded {
                    Ok(x) => Ok(Self::#vn(x)),
                    Err(_) => Ok(Self::Other(v.into_value())),
                }
            })
        } else {
            conv(space, &name, vty)
        };
        arms.push(quote!(#(#pats)|* => #c,));
    }
    let rest: Vec<TokenStream> = K::ALL.iter().filter(|k| !covered.contains(k)).map(|k| k.path()).collect();
    let none = no_variant(u);
    let other = (!rest.is_empty()).then(|| quote!(#(#rest)|* => #none,));
    quote! {
        match v.kind() {
            #(#arms)*
            #other
        }
    }
}

/// The JSON kinds of `runtime::Kind` as the emitter spells them.
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

        pub(super) fn path(self) -> TokenStream {
            match self {
                Self::Null => quote!(runtime::Kind::Null),
                Self::Bool => quote!(runtime::Kind::Bool),
                Self::Integer => quote!(runtime::Kind::Integer),
                Self::Number => quote!(runtime::Kind::Number),
                Self::String => quote!(runtime::Kind::String),
                Self::Array => quote!(runtime::Kind::Array),
                Self::Object => quote!(runtime::Kind::Object),
            }
        }
    }
}

/// Returns the message of a value that a closed enum does not list: `{other}` takes the value. Braces in the values are
/// escaped.
pub(super) fn one_of(values: impl Iterator<Item = String>) -> String {
    let allowed = values.map(|v| format!("`{v}`")).collect::<Vec<_>>().join(", ");
    format!("`{{other}}` is not one of {}", allowed.replace('{', "{{").replace('}', "}}"))
}
