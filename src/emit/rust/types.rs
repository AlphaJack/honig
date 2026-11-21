//! `types.rs`: one Rust item per `TypeDef` built as a syntax tree.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::syntax::{doc_opt, has_float, holds, ident, lit, ty};

use crate::emit::rust::names::{pascal, snake};
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::names::Name;
use crate::ir::types::{Checked, Field, Format, Kind, Struct, Ty, TypeDef, TypeId};

mod defaults;
mod enums;

pub(super) use defaults::default_expr;
use defaults::{const_default, field_default, is_arg, is_unit, struct_default};
use enums::{OTHER_DOC, one_of, reads_added, str_enum, union};

/// Returns `types.rs`. A type spelt out twice is declared once and the others are `pub type` aliases of it
/// ([`aliases`]).
pub(super) fn emit(api: &crate::ir::Api, space: &Space<'_>, opts: &crate::emit::Options) -> TokenStream {
    // Whether any body travels as XML.
    let xml = api
        .http
        .operations
        .iter()
        .flat_map(crate::ir::http::Operation::contents)
        .any(|c| c.kind == crate::ir::media::MediaKind::Xml);
    let (extra, binary) = (opts.rust.derives.as_slice(), opts.rust.binary);
    // A type spelt out twice (one inline schema per operation) is declared once; the others alias it.
    let aliases = &aliases(space, extra);
    let mut items = vec![];
    let mut builders = vec![];
    // Whether a builder refers to a generated type: then the module imports `types`.
    let mut builders_name_types = false;
    for (i, def) in space.types.defs.iter().enumerate() {
        let id = TypeId(i);
        if let Some(path) = &def.rust_type {
            external(&mut items, def, path);
            continue;
        }
        let name = pascal_ident(&def.name);
        if let Some(&first) = aliases.get(&i) {
            let (doc, first) = (doc_opt(def.doc.as_deref()), pascal_ident(&space.types.defs[first].name));
            // A unit struct is a value too: `use` brings in both, where `type` brings in only the type.
            if matches!(def.kind, Kind::Const(_)) {
                items.push(quote! { #doc pub use #first as #name; });
            } else {
                items.push(quote! { #doc pub type #name = #first; });
            }
            continue;
        }
        let first = items.len();
        // Extra derives from the settings and `deprecated` go on the declaration of the type: inside the body of an
        // object's macro, else on its first item.
        let derive = extra_derives(def, extra);
        let deprecated = def.deprecated.then(|| quote!(#[deprecated]));
        let attrs = quote!(#derive #deprecated);
        match &def.kind {
            Kind::Struct(s) => {
                structure(&mut items, space, id, s, &attrs);
                builders.push(builder(space, &name, &name, s));
                builders_name_types |= names_types(space, s);
                if xml {
                    items.push(xml_shape(space, def, s));
                }
            }
            Kind::StrEnum(values) => str_enum(&mut items, def, values),
            Kind::IntEnum(values) => int_enum(&mut items, def, values),
            Kind::Const(value) => constant(&mut items, def, value),
            Kind::Checked(c) => {
                checked_type(&mut items, space, id, def, c);
                if let Some((b, names_types)) = checked_builder(space, &name, c) {
                    builders.push(b);
                    builders_name_types |= names_types;
                }
            }
            Kind::Union(u) => union(&mut items, space, aliases, def, u),
            Kind::Alias(t) => {
                let doc = doc_opt(def.doc.as_deref());
                let t = ty(space, t, false);
                items.push(quote! { #doc pub type #name = #t; });
            }
            Kind::Newtype(t) if def.json_string => json_string(&mut items, space, def, t),
            Kind::Newtype(t) => {
                newtype(&mut items, space, id, def, t);
            }
            Kind::Pending => {}
        }
        if !matches!(def.kind, Kind::Struct(_))
            && let Some(item) = items.get_mut(first)
        {
            *item = quote!(#attrs #item);
        }
    }
    // A library exports the constructors. A binary may build a type only by decoding it.
    let header = binary.then(|| quote!(#![allow(dead_code, reason = "a binary may build none of the types by hand")]));
    // A description without schemas has no type to validate.
    let runtime_import = (!items.is_empty()).then(|| {
        quote!(
            use super::runtime;
        )
    });
    let builders = builders_mod(&builders, builders_name_types);
    quote! { #header #runtime_import #(#items)* #builders }
}

/// Inlines types declared as an alias of an identical earlier one (one inline schema per operation): index to the
/// first. A schema of `components/schemas` is never one, nor does an inline one alias it.
/// A twin with derives of its own or a deprecation keeps its own declaration.
pub(crate) fn aliases(space: &Space<'_>, extra: &[(String, Vec<String>)]) -> std::collections::BTreeMap<usize, usize> {
    let mut twins = twins(space);
    twins.retain(|&i, _| extra_derives(&space.types.defs[i], extra).is_none() && !space.types.defs[i].deprecated);
    twins
}

/// Returns a better name for the type that its twins alias: the longest ending of two words or more that most twins'
/// names share. `<Operation>Ok` envelopes that each spell out `usage` alias `OkUsage` and not the name of the first one
/// built. Bigger groups pick first. A name another type has is skipped for the next ending. When one twin is just those
/// words it is the schema they all repeat, so the group keeps its name.
pub(super) fn shared_names(
    space: &Space<'_>,
    aliases: &std::collections::BTreeMap<usize, usize>,
) -> Vec<(usize, Name)> {
    let defs = &space.types.defs;
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for (&twin, &first) in aliases {
        groups.entry(first).or_insert_with(|| vec![first]).push(twin);
    }
    let mut taken = space.types.name_keys();
    let mut out = vec![];
    let mut groups: Vec<(usize, Vec<usize>)> = groups.into_iter().collect();
    groups.sort_by_key(|(first, members)| (std::cmp::Reverse(members.len()), *first));
    for (first, members) in groups {
        let words: Vec<&[String]> = members.iter().map(|&i| defs[i].name.words()).collect();
        // Every ending of two words or more, with how many twins end with it.
        let mut endings: std::collections::BTreeMap<&[String], usize> = std::collections::BTreeMap::new();
        for w in &words {
            for k in 2..=w.len() {
                *endings.entry(&w[w.len() - k..]).or_default() += 1;
            }
        }
        // The longest ending that most twins share. One word (`Key`, `Role`) says too little to stand for them.
        let mut endings: Vec<(&[String], usize)> = endings.into_iter().filter(|(_, n)| n * 2 > members.len()).collect();
        endings.sort_by_key(|(e, n)| std::cmp::Reverse((e.len(), *n)));
        let mine = |name: &Name| {
            name.key() == defs[first].name.key() || members.iter().any(|&i| defs[i].name.key() == name.key())
        };
        let Some(name) =
            endings.iter().map(|(e, _)| Name::new(&e.join("_"))).find(|n| mine(n) || !taken.contains(&n.key()))
        else {
            continue;
        };
        // A twin that is just those words is the schema the others repeat: it is not renamed.
        if mine(&name) {
            continue;
        }
        taken.insert(name.key());
        out.push((first, name));
    }
    out
}

/// Inlines types identical to an earlier inline one in the same role (the last word of their names) as index to the
/// first. Two types are identical when their kinds are.
/// References to twins resolve to the first. So a struct of twin enums is a twin too. Repeated until
/// nothing changes.
fn twins(space: &Space<'_>) -> std::collections::BTreeMap<usize, usize> {
    let mut first: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    loop {
        let canon = |id: TypeId| TypeId(first.get(&id.0).copied().unwrap_or(id.0));
        // References to a twin resolve to its first.
        // The last word of the name is the role (`type`, `usage`, `Ok`): twins with different roles stay apart, so an
        // order's `type` does not merge with a user's `status` although both are `enum: [0, 1]`.
        let key = |def: &TypeDef| {
            let role = def.name.words().last().cloned().unwrap_or_default();
            format!("{:?}|{:?}|{}|{role}", canonical_kind(&def.kind, &canon), def.xml, def.json_string)
        };
        // Keys by their hash first: keys are long and share a long start, so ordering them by text compares that start
        // again on every step down the map. The text still decides which keys are equal.
        let mut seen: std::collections::BTreeMap<u64, Vec<(String, usize)>> = std::collections::BTreeMap::new();
        let mut next = std::collections::BTreeMap::new();
        for (i, def) in space.types.defs.iter().enumerate() {
            // A schema of `components/schemas` is its own type: `Kind` and `Mode` may both be `enum: [0, 1]`.
            if def.component || def.rust_type.is_some() || matches!(def.kind, Kind::Pending | Kind::Alias(_)) {
                continue;
            }
            let key = key(def);
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&key, &mut hasher);
            let bucket = seen.entry(std::hash::Hasher::finish(&hasher)).or_default();
            match bucket.iter().find(|(k, _)| *k == key) {
                Some(&(_, first)) => {
                    next.insert(i, first);
                }
                None => bucket.push((key, i)),
            }
        }
        if next == first {
            return first;
        }
        first = next;
    }
}

/// Returns `kind` with every named type it refers to mapped through `canon`.
fn canonical_kind(kind: &Kind, canon: &dyn Fn(TypeId) -> TypeId) -> Kind {
    let mut kind = kind.clone();
    match &mut kind {
        Kind::Struct(s) => {
            for f in &mut s.fields {
                f.ty = canonical_ty(&f.ty, canon);
            }
            if let Some(a) = &mut s.additional {
                *a = canonical_ty(a, canon);
            }
            for (_, t, _) in &mut s.patterns {
                *t = canonical_ty(t, canon);
            }
        }
        Kind::Union(u) => {
            for v in &mut u.variants {
                v.ty = canonical_ty(&v.ty, canon);
            }
        }
        Kind::Alias(t) | Kind::Newtype(t) => *t = canonical_ty(t, canon),
        Kind::StrEnum(_) | Kind::IntEnum(_) | Kind::Const(_) | Kind::Checked(_) | Kind::Pending => {}
    }
    kind
}

/// Returns `t` with every named type mapped through `canon`.
fn canonical_ty(t: &Ty, canon: &dyn Fn(TypeId) -> TypeId) -> Ty {
    fn canonical(t: &mut Ty, canon: &dyn Fn(TypeId) -> TypeId) {
        if let Ty::Named(id) = t {
            *id = canon(*id);
        }
        t.children_mut().iter_mut().for_each(|x| canonical(x, canon));
    }
    let mut out = t.clone();
    canonical(&mut out, canon);
    out
}

/// The base of a checked newtype as its impls tell it apart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Base {
    Str,
    Int,
    Float,
    Decimal,
    /// Any other base, or keywords checked on the whole value (`if`, `not`, ...): its checks are those of `checks`.
    Other,
}

impl Base {
    /// Returns how the checked type `c` is written.
    const fn of(c: &Checked) -> Self {
        match &c.base {
            Ty::String => Self::Str,
            Ty::F32 | Ty::F64 => Self::Float,
            Ty::Decimal => Self::Decimal,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => Self::Int,
            Ty::Bool
            | Ty::Bytes
            | Ty::Binary
            | Ty::Format(_)
            | Ty::Any
            | Ty::Unit
            | Ty::Named(_)
            | Ty::Vec(_)
            | Ty::Array(..)
            | Ty::Tuple(_)
            | Ty::Map(_)
            | Ty::Option(_)
            | Ty::Indirect(_) => Self::Other,
        }
    }
}

/// Emits `x-rust-type` as `pub type Name = your::Type;`. Its constraints are yours to keep.
fn external(items: &mut Vec<TokenStream>, def: &TypeDef, path: &str) {
    let name = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let ty = super::syntax::path(path);
    items.push(quote! {
        #doc
        pub type #name = #ty;
    });
}

/// Returns `#[derive(...)]` for the extra derives in the settings of `def` (`*` and its
/// name). Those that the generator already derives for its kind are left out.
fn extra_derives(def: &TypeDef, derives: &[(String, Vec<String>)]) -> Option<TokenStream> {
    let builtin: &[&str] = match &def.kind {
        Kind::Struct(_) | Kind::Newtype(_) => {
            &["Debug", "Clone", "PartialEq", "Eq", "Default", "Serialize", "Deserialize"]
        }
        Kind::StrEnum(_) | Kind::IntEnum(_) => {
            &["Debug", "Clone", "Copy", "PartialEq", "Eq", "Hash", "PartialOrd", "Ord", "Serialize", "Deserialize"]
        }
        Kind::Union(_) => &["Debug", "Clone", "PartialEq", "Eq", "Serialize", "Deserialize"],
        Kind::Const(_) => &[
            "Debug",
            "Clone",
            "Copy",
            "PartialEq",
            "Eq",
            "Hash",
            "PartialOrd",
            "Ord",
            "Default",
            "Serialize",
            "Deserialize",
        ],
        Kind::Alias(_) | Kind::Checked(_) | Kind::Pending => return None,
    };
    let mut paths: Vec<&str> = vec![];
    for (key, list) in derives {
        if key == "*" || *key == pascal(&def.name) {
            for p in list {
                let last = p.rsplit("::").next().unwrap_or(p);
                if !builtin.contains(&last) && !paths.contains(&p.as_str()) {
                    paths.push(p);
                }
            }
        }
    }
    (!paths.is_empty()).then(|| {
        let paths = paths.iter().map(|p| super::syntax::path(p));
        quote!(#[derive(#(#paths),*)])
    })
}

/// Returns `impl runtime::XmlShape`: element names and how each property maps to XML.
fn xml_shape(space: &Space<'_>, def: &TypeDef, s: &Struct) -> TokenStream {
    let n = pascal_ident(&def.name);
    let name = lit(def.xml.name.as_deref().unwrap_or(&pascal(&def.name)));
    let namespace = lit(def.xml.namespace.as_deref().unwrap_or(""));
    let prefix = lit(def.xml.prefix.as_deref().unwrap_or(""));
    // Unwrapped arrays whose items share an element name cannot be told
    // apart on the wire: those are wrapped in their property element.
    let item_of = |f: &Field| f.xml.item.clone().or_else(|| f.xml.name.clone()).unwrap_or_else(|| f.wire.clone());
    let unwrapped: Vec<String> =
        s.fields.iter().filter(|f| xml_inner(space, &f.ty).1 && !f.xml.wrapped).map(item_of).collect();
    let ambiguous = |f: &Field| unwrapped.iter().filter(|n| **n == item_of(f)).count() > 1;
    let fields = s.fields.iter().map(|f| {
        let (inner, list) = xml_inner(space, &f.ty);
        let elem = f.xml.name.as_deref().unwrap_or(&f.wire);
        let item = f.xml.item.as_deref().unwrap_or(elem);
        let wrapped = list && (f.xml.wrapped || ambiguous(f));
        let (attribute, text, required) = (f.xml.attribute, f.xml.text, f.required);
        // `nodeType: none` on an array means no wrapper (an unwrapped list):
        // only an object's fields sit inline in the parent.
        let (cdata, inline) = (f.xml.cdata, f.xml.inline && !list);
        // An omitted attribute of a property that may be `null` decodes as `null`; an empty element of a free-form
        // object as `{}`.
        let nullable = f.ty.is_option();
        let object = matches!(inner, Ty::Map(_));
        let child = if let Ty::Named(id) = inner
            && matches!(space.types.get(*id).kind, Kind::Struct(_))
        {
            let c = pascal_ident(&space.types.get(*id).name);
            Some(quote!(child: Some(<#c as runtime::XmlShape>::xml),))
        } else {
            None
        };
        // Only what differs from `XmlField::new(wire)`.
        let text_of = |field: &str, v: &str, default: &str| (v != default).then(|| (ident(field), lit(v)));
        let texts = [
            text_of("name", elem, &f.wire),
            text_of("prefix", f.xml.prefix.as_deref().unwrap_or(""), ""),
            text_of("item", item, &f.wire),
        ];
        let texts = texts.into_iter().flatten().map(|(k, v)| quote!(#k: #v,));
        let flags = [
            ("attribute", attribute),
            ("text", text),
            ("list", list),
            ("wrapped", wrapped),
            ("required", required),
            ("cdata", cdata),
            ("inline", inline),
            ("nullable", nullable),
            ("object", object),
        ];
        let flags = flags.into_iter().filter(|(_, on)| *on).map(|(k, _)| {
            let k = ident(k);
            quote!(#k: true,)
        });
        let set: Vec<TokenStream> = texts.chain(flags).chain(child).collect();
        let wire = lit(&f.wire);
        if set.is_empty() {
            quote!(runtime::XmlField::new(#wire))
        } else {
            quote!(runtime::XmlField { #(#set)* ..runtime::XmlField::new(#wire) })
        }
    });
    quote! {
        impl runtime::XmlShape for #n {
            fn xml() -> &'static runtime::XmlNode {
                static NODE: runtime::XmlNode = runtime::XmlNode {
                    name: #name,
                    namespace: #namespace,
                    prefix: #prefix,
                    fields: &[#(#fields),*],
                };
                &NODE
            }
        }
    }
}

/// Aliases followed to an XML property's element type: a chain may loop.
const MAX_ALIAS_HOPS: usize = 16;

/// Returns the element type of a property and whether it is a list. This looks through
/// `Option`, `Box` and aliases.
fn xml_inner<'a>(space: &Space<'a>, mut t: &'a Ty) -> (&'a Ty, bool) {
    let mut list = false;
    for _ in 0..MAX_ALIAS_HOPS {
        t = if let Ty::Option(x) | Ty::Indirect(x) = t {
            x
        } else if let (Ty::Vec(x) | Ty::Array(x, _), false) = (t, list) {
            list = true;
            x
        } else if let Ty::Named(id) = t
            && let Kind::Alias(x) = &space.types.get(*id).kind
        {
            x
        } else {
            return (t, list);
        };
    }
    (t, list)
}

/// Returns `impl Default`. Spec defaults go through their functions. A required unit struct is its value, `Unit {}`.
/// The rest is empty.
fn default_impl(space: &Space<'_>, name: &proc_macro2::Ident, s: &Struct, extra: &proc_macro2::Ident) -> TokenStream {
    let inits = s.fields.iter().map(|f| {
        let (n, t) = (snake_ident(&f.name), field_ty(space, f));
        if field_default(space, f).is_some() {
            let fun = format_ident!("default_{}", snake(&f.name));
            quote!(#n: Self::#fun(),)
        } else if is_unit(space, f) {
            // Braces: the field's type may be an alias of the unit struct, which is no value by its name.
            quote!(#n: #t {},)
        } else {
            quote!(#n: <#t>::default(),)
        }
    });
    let extra_init = extra_inits(s, extra);
    quote! {
        impl Default for #name {
            fn default() -> Self { Self { #(#inits)* #extra_init } }
        }
    }
}

/// Returns `#[derive(...)]` with `Eq` whenever no float is reachable.
fn derives(space: &Space<'_>, id: TypeId, default: bool) -> TokenStream {
    let eq = (!has_float(space, &Ty::Named(id))).then(|| quote!(Eq,));
    let default = default.then(|| quote!(Default,));
    quote!(#[derive(Debug, Clone, PartialEq, #eq #default serde::Serialize, serde::Deserialize)])
}

/// Returns one field of `runtime::object!`: its kind, its name and its type, with the serde attributes that its kind
/// does not imply (a rename, a spec default, a reader of a library format).
fn field(space: &Space<'_>, f: &Field, owner: &str) -> TokenStream {
    let doc = doc_opt(f.doc.as_deref());
    let doc = if f.deprecated { quote!(#doc #[deprecated]) } else { doc };
    let name = snake_ident(&f.name);
    // serde already uses the field name (a raw `r#type` as `type`).
    let wire = lit(&f.wire);
    let rename = (f.wire != snake(&f.name)).then(|| quote!(#[serde(rename = #wire)]));
    // A library type that accepts more than its format allows gets its text checked first. `wrap` wraps it as the
    // field's own type does: `Option`, `runtime::Nullable` or itself.
    let checked = |t: &Ty, wrap: &str| {
        holds_text(space, t).map(|h| {
            let h = if wrap.is_empty() { h } else { format!("runtime::formats::{wrap}<{h}>") };
            let read = lit(&format!("runtime::formats::read::<_, _, {h}>"));
            quote!(deserialize_with = #read)
        })
    };
    let plain = |attrs: TokenStream| {
        let t = field_ty(space, f);
        quote! { #doc #rename #[serde(#attrs)] pub #name: #t, }
    };
    if field_default(space, f).is_some() {
        let fun = lit(&format!("{owner}::default_{}", snake(&f.name)));
        let read = checked(&f.ty, "").map(|r| quote!(, #r));
        return plain(quote!(default = #fun #read));
    }
    if f.required && space.types.nullable(&f.ty).is_some() {
        // serde decodes a missing `Option` as `None`: its own deserializer makes the property required again.
        let read = checked(&f.ty, "").unwrap_or_else(|| quote!(deserialize_with = "runtime::required_nullable"));
        return plain(read);
    }
    if f.required {
        if let Some(read) = checked(&f.ty, "") {
            return plain(read);
        }
        let t = ty(space, &f.ty, false);
        return quote! { #doc #rename pub #name: #t, };
    }
    if let Some(inner) = space.types.nullable(&f.ty) {
        if let Some(read) = checked(inner, "OrNull") {
            return plain(quote!(default, skip_serializing_if = "runtime::Nullable::is_absent", #read));
        }
        let t = ty(space, inner, false);
        return quote! { #[nullable] #doc #rename pub #name: #t, };
    }
    let inner = f.ty.non_null();
    if f.null_as_absent {
        // serde's own `Option`: `null` and absent both decode as `None`.
        if let Some(read) = checked(&f.ty, "Maybe") {
            return plain(quote!(default, skip_serializing_if = "Option::is_none", #read));
        }
        let t = ty(space, inner, false);
        return quote! { #[maybe] #doc #rename pub #name: #t, };
    }
    // `null` is no value of a property that is not nullable whether it is absent or not.
    if let Some(read) = checked(&f.ty, "Present") {
        return plain(quote!(default, skip_serializing_if = "Option::is_none", #read));
    }
    let t = ty(space, inner, false);
    quote! { #[optional] #doc #rename pub #name: #t, }
}

/// Returns [`holds`] as the text of a `deserialize_with` path. It is paths, generics and numbers: no space is needed.
fn holds_text(space: &Space<'_>, t: &Ty) -> Option<String> {
    holds(space, t).map(|h| h.to_string().replace(' ', ""))
}

/// Returns the empty undeclared-properties map.
fn no_extra(extra: &proc_macro2::Ident) -> TokenStream {
    quote!(#extra: std::collections::BTreeMap::new(),)
}

/// Returns the Rust type of a struct field. It is its own when required or defaulted. Otherwise it is `Option` or
/// `runtime::Nullable` when it also accepts `null`.
fn field_ty(space: &Space<'_>, f: &Field) -> TokenStream {
    field_ty_in(space, f, false)
}

/// Returns [`field_ty`] with named types as `types::Name` when `qualify`.
fn field_ty_in(space: &Space<'_>, f: &Field, qualify: bool) -> TokenStream {
    if f.required || field_default(space, f).is_some() {
        return ty(space, &f.ty, qualify);
    }
    match space.types.nullable(&f.ty) {
        Some(inner) => {
            let inner = ty(space, inner, qualify);
            quote!(runtime::Nullable<#inner>)
        }
        None => {
            let t = ty(space, &f.ty, qualify);
            quote!(Option<#t>)
        }
    }
}

/// Returns the fields that store the `patternProperties` of `s` ([`Struct::patterns`]) and the names that no pattern
/// matches, with the functions that decode each from the undeclared names. Empty without patterns.
fn pattern_fields(
    space: &Space<'_>,
    s: &Struct,
    name: &proc_macro2::Ident,
    extra: &proc_macro2::Ident,
) -> (TokenStream, TokenStream) {
    if s.patterns.is_empty() {
        return (TokenStream::new(), TokenStream::new());
    }
    let patterns: Vec<proc_macro2::Literal> = s.patterns.iter().map(|(p, _, _)| lit(p)).collect();
    let (mut fields, mut readers) = (vec![], vec![]);
    for (i, (p, t, _)) in s.patterns.iter().enumerate() {
        let (field, t) = (super::syntax::pattern_field(s, i), ty(space, t, false));
        let read = format_ident!("read_{field}");
        let path = lit(&format!("{name}::{read}"));
        let doc = lit(&format!(" The undeclared names that `{p}` matches."));
        fields.push(quote! {
            #[doc = #doc]
            #[serde(flatten, deserialize_with = #path)]
            pub #field: std::collections::BTreeMap<String, #t>,
        });
        readers.push(quote! {
            #[doc(hidden)]
            pub fn #read<'de, D: serde::Deserializer<'de>>(d: D) -> Result<std::collections::BTreeMap<String, #t>, D::Error> {
                runtime::pattern_entries(d, &[#(#patterns),*], #i)
            }
        });
    }
    let read = format_ident!("read_{extra}");
    let path = lit(&format!("{name}::{read}"));
    let (t, body) = match &s.additional {
        Some(t) => {
            let t = ty(space, t, false);
            (quote!(std::collections::BTreeMap<String, #t>), quote!(runtime::other_entries(d, &[#(#patterns),*])))
        }
        None => (quote!(runtime::Closed), quote!(runtime::no_other_entries(d, &[#(#patterns),*]))),
    };
    fields.push(quote! {
        /// The undeclared names that no pattern matches.
        #[serde(flatten, deserialize_with = #path)]
        pub #extra: #t,
    });
    readers.push(quote! {
        #[doc(hidden)]
        pub fn #read<'de, D: serde::Deserializer<'de>>(d: D) -> Result<#t, D::Error> {
            #body
        }
    });
    (quote!(#(#fields)*), quote!(impl #name { #(#readers)* }))
}

/// Returns the empty values of the fields of `s` that store undeclared names.
fn extra_inits(s: &Struct, extra: &proc_macro2::Ident) -> Option<TokenStream> {
    if s.patterns.is_empty() {
        return s.additional.is_some().then(|| no_extra(extra));
    }
    let maps = (0..s.patterns.len()).map(|i| super::syntax::pattern_field(s, i));
    let other =
        if s.additional.is_some() { quote!(std::collections::BTreeMap::new()) } else { quote!(runtime::Closed) };
    Some(quote!(#(#maps: std::collections::BTreeMap::new(),)* #extra: #other,))
}

fn structure(items: &mut Vec<TokenStream>, space: &Space<'_>, id: TypeId, s: &Struct, attrs: &TokenStream) {
    let def = space.types.get(id);
    let name = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    // `..Default::default()` keeps struct literals compiling when a later
    // description adds an optional field.
    // Spec defaults: a `default_<field>` per field for serde and a `Default`
    // impl that uses them instead of the own default of each type.
    let defaults: Vec<(&Field, TokenStream)> =
        s.fields.iter().filter_map(|f| field_default(space, f).map(|e| (f, e))).collect();
    let can_default = struct_default(space, s, &mut vec![id]);
    let derive = derives(space, id, can_default && defaults.is_empty());
    let fields = s.fields.iter().map(|f| field(space, f, &pascal(&def.name)));
    let extra = ident(&super::syntax::extra_field(s));
    let (held, readers) = pattern_fields(space, s, &name, &extra);
    let additional = s.additional.as_ref().filter(|_| s.patterns.is_empty()).map(|t| {
        let read = holds_text(space, t).map(|h| {
            let read = lit(&format!("runtime::formats::read::<_, _, runtime::formats::Values<{h}>>"));
            quote!(, deserialize_with = #read)
        });
        let t = ty(space, t, false);
        quote! {
            #[serde(flatten #read)]
            pub #extra: std::collections::BTreeMap<String, #t>,
        }
    });
    // `additionalProperties: false`: an undeclared property is a decode error,
    // so a server answers 400 and a client reports a server off its contract.
    // A lenient client's response type accepts a property that the server added. Its request copy refuses it.
    let closed = (s.closed && s.additional.is_none() && s.patterns.is_empty() && !def.lenient)
        .then(|| quote!(#[serde(deny_unknown_fields)]));
    // `runtime::object!` writes the struct with `remote = "Self"` and the trait impls that decode through `NoSeq`: a
    // derived struct would also decode a JSON array field by field, and `type: object` allows none. An extra map
    // encodes through `UniqueKeys`: an entry named like a declared field would write the name twice.
    let encode = (s.additional.is_some() || !s.patterns.is_empty()).then(|| quote!(#[unique_keys]));
    items.push(quote! {
        runtime::object! {
            #encode
            #doc
            #derive
            #attrs
            #closed
            pub struct #name {
                #(#fields)*
                #additional
                #held
            }
        }
        #readers
    });
    if new_is_safe(space, s) {
        items.push(struct_new(space, &name, s, &extra));
    }
    if !defaults.is_empty() {
        let funs = defaults.iter().map(|(f, e)| {
            let (fun, t) = (format_ident!("default_{}", snake(&f.name)), ty(space, &f.ty, false));
            let constness = const_default(space, f).then(|| quote!(const));
            // Serde's `default = "..."` needs a path; users get defaults through `Default`.
            quote!(#[doc(hidden)] #[must_use] pub #constness fn #fun() -> #t { #e })
        });
        items.push(quote!(impl #name { #(#funs)* }));
        if can_default {
            items.push(default_impl(space, &name, s, &extra));
        }
    }
}

/// Can `new` accept the required fields of `s` by position? Only when it cannot fail and no two of them can swap: no
/// argument is of a checked type, and no two have the same type. Otherwise the builder is the one constructor.
fn new_is_safe(space: &Space<'_>, s: &Struct) -> bool {
    let args: Vec<&Field> = s.fields.iter().filter(|f| is_arg(space, f)).collect();
    let mut types: Vec<String> = args.iter().map(|f| field_ty(space, f).to_string()).collect();
    types.sort();
    types.dedup();
    types.len() == args.len() && !args.iter().any(|f| holds_checked(space, &f.ty))
}

/// Returns `new(required...)`: every required field without a spec default in document order. The rest start empty or
/// at their spec default. So a struct literal `..Name::new(...)` cannot leave out a required field as
/// `..Default::default()` can.
fn struct_new(space: &Space<'_>, name: &proc_macro2::Ident, s: &Struct, extra: &proc_macro2::Ident) -> TokenStream {
    let arg = |f: &Field| is_arg(space, f);
    let args = s.fields.iter().filter(|f| arg(f)).map(|f| {
        let (n, t) = (snake_ident(&f.name), field_ty(space, f));
        quote!(#n: #t)
    });
    let inits = s.fields.iter().map(|f| {
        let n = snake_ident(&f.name);
        if arg(f) {
            quote!(#n,)
        } else if is_unit(space, f) {
            // Braces: the field's type may be an alias of the unit struct, which is no value by its name.
            let t = field_ty(space, f);
            quote!(#n: #t {},)
        } else if field_default(space, f).is_some() {
            let fun = format_ident!("default_{}", snake(&f.name));
            quote!(#n: Self::#fun(),)
        } else if space.types.nullable(&f.ty).is_some() {
            quote!(#n: runtime::Nullable::Absent,)
        } else {
            quote!(#n: None,)
        }
    });
    let extra_init = extra_inits(s, extra);
    let count = s.fields.iter().filter(|f| arg(f)).count();
    let many = many_arguments(count);
    let constness =
        s.fields.iter().all(|f| field_default(space, f).is_none() || const_default(space, f)).then(|| quote!(const));
    quote! {
        impl #name {
            #[must_use]
            pub #constness fn new(#(#args),*) -> Self {
                #many
                Self { #(#inits)* #extra_init }
            }
        }
    }
}

/// Returns the `builders` module. Builders sit in a module of their own: `builders::User` cannot clash with a schema
/// named `UserBuilder`. A builder gets the name of its type: the types are named through `types::`.
fn builders_mod(builders: &[TokenStream], names_types: bool) -> Option<TokenStream> {
    let types_import = names_types.then(|| {
        quote!(
            use super as types;
        )
    });
    (!builders.is_empty()).then(|| {
        quote! {
            /// One builder per struct, started by its `builder`. Each setter accepts anything that converts to the type
            /// of its field. `build` returns the first field that did not convert.
            pub mod builders {
                #types_import
                use super::runtime;
                #(#builders)*
            }
        }
    })
}

/// Does `t` contain a value that only a checked conversion makes: a checked type, the text of a format without its
/// library, a mail address or host name, or one of them in a list, map, option or box? Each such field needs a `?`,
/// which a builder gathers into one. Other fields build with `new` and struct update.
fn holds_checked(space: &Space<'_>, t: &Ty) -> bool {
    match t {
        Ty::Named(id) => match &space.types.get(*id).kind {
            Kind::Checked(_) => true,
            Kind::Alias(x) => holds_checked(space, x),
            Kind::Struct(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Union(_)
            | Kind::Newtype(_)
            | Kind::Const(_)
            | Kind::Pending => false,
        },
        Ty::Format(f) => {
            super::syntax::untyped(space.libs, *f)
                || (!space.libs.named.contains_key(f)
                    && matches!(f, Format::Email | Format::IdnEmail | Format::Hostname | Format::IdnHostname))
        }
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
        | Ty::Any
        | Ty::Unit => false,
        Ty::Vec(_) | Ty::Array(..) | Ty::Tuple(_) | Ty::Map(_) | Ty::Option(_) | Ty::Indirect(_) => {
            t.children().iter().any(|c| holds_checked(space, c))
        }
    }
}

/// Does a field of `s` that its builder accepts refer to a generated type? Then the builder refers to it through
/// `types::`. A required `const` it fills itself.
fn names_types(_space: &Space<'_>, s: &Struct) -> bool {
    s.fields.iter().any(|f| super::syntax::mentions(&f.ty).0)
}

/// Returns the builder of a checked type whose base is a struct, and whether it refers to a generated type. A struct
/// whose schema checks the whole value builds through its fields.
fn checked_builder(space: &Space<'_>, name: &proc_macro2::Ident, c: &Checked) -> Option<(TokenStream, bool)> {
    let Ty::Named(shape) = &c.base else { return None };
    let Kind::Struct(s) = &space.types.get(*shape).kind else { return None };
    let fields = pascal_ident(&space.types.get(*shape).name);
    let names_types = names_types(space, s);
    Some((builder(space, name, &fields, s), names_types))
}

/// Returns the builder of `name` for the `builders` module: `runtime::typestate_builder!` over the fields of `shape`.
/// It accepts each property by name; `build` exists once every required one is set. `shape` is the struct that contains
/// the fields: `name` itself, or the fields of a checked type whose `try_from` then checks the whole value.
fn builder(space: &Space<'_>, name: &proc_macro2::Ident, shape: &proc_macro2::Ident, s: &Struct) -> TokenStream {
    let setter = |f: &Field| {
        let mut setter = snake(&f.name);
        if setter == "build" || setter == "new" || setter == "builder" {
            setter.push('_');
        }
        ident(&setter)
    };
    let required = s.fields.iter().filter(|f| is_arg(space, f)).enumerate().map(|(i, f)| {
        let (n, g, t, wire) = (snake_ident(&f.name), format_ident!("R{i}"), field_ty_in(space, f, true), lit(&f.wire));
        quote!(#n: #g: #t = #wire)
    });
    let optional = s.fields.iter().filter(|f| !is_arg(space, f) && !is_unit(space, f)).map(|f| {
        let (n, set, wire) = (snake_ident(&f.name), setter(f), lit(&f.wire));
        let (t, wrap, start) = if field_default(space, f).is_some() {
            let fun = format_ident!("default_{}", snake(&f.name));
            (ty(space, &f.ty, true), quote!(value), quote!(super::#shape::#fun()))
        } else if f.required {
            (ty(space, &f.ty, true), quote!(value), quote!(Default::default()))
        } else if let Some(inner) = space.types.nullable(&f.ty) {
            (ty(space, inner, true), quote!(nullable), quote!(runtime::Nullable::Absent))
        } else {
            (ty(space, f.ty.non_null(), true), quote!(some), quote!(None))
        };
        quote!(#n (#set): #t = #wire => #wrap, #start)
    });
    let units = s.fields.iter().filter(|f| !is_arg(space, f) && is_unit(space, f)).map(|f| {
        let (n, t) = (snake_ident(&f.name), field_ty_in(space, f, true));
        quote!(#n: #t {})
    });
    let extra = ident(&super::syntax::extra_field(s));
    let extras = extra_inits(s, &extra).into_iter();
    let outer = (name != shape).then(|| quote!(, try_from super::#name));
    quote! {
        runtime::typestate_builder! {
            #name => super::#shape #outer;
            required { #(#required),* }
            optional { #(#optional),* }
            fixed { #(#units,)* #(#extras)* }
        }
    }
}

/// Returns the `allow` of a constructor that accepts `count` required properties, past what clippy allows. It goes
/// inside the body: `clippy::allow_attributes` leaves an inner `#![allow]` alone, and an `#[expect]` would fail a crate
/// that raises `too-many-arguments-threshold`.
fn many_arguments(count: usize) -> Option<TokenStream> {
    (count > super::syntax::MAX_ARGUMENTS)
        .then(|| quote!(#![allow(clippy::too_many_arguments, reason = "one argument per required property")]))
}

/// Emits a newtype carried as JSON text inside a string.
fn json_string(items: &mut Vec<TokenStream>, space: &Space<'_>, def: &TypeDef, t: &Ty) {
    let name = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let eq = (!has_float(space, t)).then(|| quote!(Eq,));
    let t = ty(space, t, false);
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, #eq)]
        pub struct #name(pub #t);
        runtime::json_text_impls!(#name);
    });
}

/// Emits a closed integer enum with one variant per listed value. Any other value fails to convert or decode.
fn int_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[(Name, i64)]) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let names: Vec<_> = values.iter().map(|(v, _)| pascal_ident(v)).collect();
    let nums: Vec<_> = values.iter().map(|(_, x)| proc_macro2::Literal::i64_unsuffixed(*x)).collect();
    let allowed = lit(&one_of(values.iter().map(|(_, v)| v.to_string())));
    // A lenient client's response accepts a value that the server added as `Other`. Its request copy is closed.
    let (unknown, unknown_value, other) = if def.lenient {
        (
            quote!(#[doc = #OTHER_DOC] Other(i64),),
            quote!(Self::Other(v) => v,),
            quote!(other => Ok(Self::Other(other)),),
        )
    } else {
        (TokenStream::new(), TokenStream::new(), quote!(other => Err(runtime::BuildError::value(format!(#allowed))),))
    };
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum #n {
            #(#names,)*
            #unknown
        }
        impl #n {
            #[must_use]
            pub const fn value(self) -> i64 {
                match self {
                    #(Self::#names => #nums,)*
                    #unknown_value
                }
            }
        }
        impl TryFrom<i64> for #n {
            type Error = runtime::BuildError;
            fn try_from(v: i64) -> Result<Self, runtime::BuildError> {
                match v {
                    #(#nums => Ok(Self::#names),)*
                    #other
                }
            }
        }
        runtime::int_enum_impls!(#n);
    });
}

/// Emits `pub struct Name(pub T);` that is transparent on the wire.
fn newtype(items: &mut Vec<TokenStream>, space: &Space<'_>, id: TypeId, def: &TypeDef, t: &Ty) {
    let name = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let derive = derives(space, id, false);
    // A library type has its text checked before the library parses it.
    let read = holds_text(space, t).map(|h| {
        let read = lit(&format!("runtime::formats::read::<_, _, {h}>"));
        quote!(#[serde(deserialize_with = #read)])
    });
    let t = ty(space, t, false);
    items.push(quote! {
        #doc
        #derive
        #[serde(transparent)]
        pub struct #name(#read pub #t);
    });
}

/// Emits a checked newtype: a scalar that only `TryFrom` and decoding build. Both check the keywords of its schema and
/// return the first that the value breaks. The impls shared by every checked type of one base are a macro (see
/// `checked_macros`).
fn checked_type(items: &mut Vec<TokenStream>, space: &Space<'_>, id: TypeId, def: &TypeDef, c: &Checked) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let path = quote!(&runtime::InstancePath::Root);
    let base = ty(space, &c.base, false);
    let kind = Base::of(c);
    // A scalar's own checks leave out `not`: it is checked here. `checks::checks` checks it for any other base.
    let subschemas = (!matches!(kind, Base::Other))
        .then(|| super::checks::subschema_checks(space, &c.constraints, &quote!(&value), &path));
    let (checks, derive, consts) = match kind {
        Base::Str => {
            let pat = c.constraints.pattern.as_deref().map(|p| {
                let p = lit(p);
                quote!(impl #n { pub const PATTERN: &'static str = #p; })
            });
            let checks = super::checks::string_checks(&c.constraints, &quote!(value.as_str()), &path);
            (checks, quote!(Eq, Hash, PartialOrd, Ord,), pat)
        }
        Base::Float => {
            let checks = super::checks::float_checks(&c.constraints, &quote!(&value), &path);
            (checks, quote!(Copy, PartialOrd,), None)
        }
        Base::Decimal => {
            let checks = super::checks::decimal_checks(&c.constraints, &quote!(&value), &path);
            (checks, quote!(Eq, Hash,), None)
        }
        Base::Int => {
            let checks = super::checks::integer_checks(&c.base, &c.constraints, &quote!(&value), &path);
            (checks, quote!(Copy, Eq, Hash, PartialOrd, Ord,), None)
        }
        Base::Other => {
            let checks = super::checks::checks(space, &c.base, &c.constraints, &quote!(&value), &path);
            // A member that the server added meets no keyword of the description: a lenient client accepts it as it is.
            let added = matches!(&c.base, Ty::Named(inner) if {
                let inner = space.types.get(*inner);
                matches!(&inner.kind, Kind::Union(union) if reads_added(inner, union))
            });
            let checks = if added { quote!(if !matches!(value, #base::Other(_)) { #checks }) } else { checks };
            let eq = (!has_float(space, &Ty::Named(id))).then(|| quote!(Eq,));
            // The base decodes the text of its library formats checked, as a field of it would. Otherwise JSON text is
            // parsed once when the checks use it as JSON.
            let de = match holds(space, &c.base) {
                Some(h) => quote!(runtime::checked_deserialize!(#n, #base, read #h);),
                None if reads_json(subschemas.as_ref(), &checks) => {
                    quote!(runtime::checked_deserialize!(#n, #base, once);)
                }
                None => quote!(runtime::checked_deserialize!(#n, #base);),
            };
            (checks, quote!(#eq), Some(de))
        }
    };
    let shared = match kind {
        Base::Str => quote!(runtime::checked_str_impls!(#n);),
        Base::Decimal => quote!(runtime::checked_decimal_impls!(#n);),
        Base::Int | Base::Float => quote!(runtime::checked_num_impls!(#n, #base);),
        Base::Other => quote!(runtime::checked_impls!(#n, #base);),
    };
    // An earlier checked type of the same base and keywords checks for this one.
    let twin = space.types.defs[..id.0].iter().find(|d| {
        d.rust_type.is_none()
            && matches!(&d.kind, Kind::Checked(o) if o.base == c.base && o.constraints == c.constraints)
    });
    let build = match twin {
        Some(first) => {
            let first = pascal_ident(&first.name);
            let json = reads_json(subschemas.as_ref(), &checks).then(|| quote!(, json));
            quote!(runtime::checked_twin!(#n, #first, #base #json);)
        }
        None => checked_try_from(&n, &base, subschemas.as_ref(), &checks),
    };
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, #derive serde::Serialize)]
        #[serde(transparent)]
        pub struct #n(pub(super) #base);
        #consts
        #build
        #shared
    });
}

/// Returns `TryFrom<base>` of the checked type `n`: it runs `subschemas` and `checks` on the value. Checks that look
/// into the value as JSON use one buffer of it, written out once.
fn checked_try_from(
    n: &proc_macro2::Ident,
    base: &TokenStream,
    subschemas: Option<&TokenStream>,
    checks: &TokenStream,
) -> TokenStream {
    let body = quote! {
        let mut violations: Vec<runtime::Violation> = Vec::new();
        {
            let out = &mut violations;
            #subschemas
            #checks
        }
        runtime::first(violations).map(|()| Self(value)).map_err(runtime::BuildError::value)
    };
    if !reads_json(subschemas, checks) {
        return quote! {
            impl TryFrom<#base> for #n {
                type Error = runtime::BuildError;
                fn try_from(value: #base) -> Result<Self, runtime::BuildError> {
                    #body
                }
            }
        };
    }
    // The checks see the value as JSON: from the text that it was decoded from, or from the value written out once.
    quote! {
        impl #n {
            fn checked(value: #base, json: &runtime::Content<'_>) -> Result<Self, runtime::BuildError> {
                #body
            }
        }
        runtime::checked_json!(#n, #base);
    }
}

/// Do `checks` use `json`, the checked value as a buffer that the checked type binds? A block that binds its own
/// (`let json = ...`, a value further in) does not.
fn reads_json(subschemas: Option<&TokenStream>, checks: &TokenStream) -> bool {
    fn reads(ts: &TokenStream) -> bool {
        ts.clone().into_iter().any(|t| match t {
            proc_macro2::TokenTree::Ident(i) => i == "json",
            proc_macro2::TokenTree::Group(g) => {
                let mut inner = g.stream().into_iter();
                let binds = matches!((inner.next(), inner.next()),
                    (Some(proc_macro2::TokenTree::Ident(l)), Some(proc_macro2::TokenTree::Ident(j))) if l == "let" && j == "json");
                !binds && reads(&g.stream())
            }
            proc_macro2::TokenTree::Punct(_) | proc_macro2::TokenTree::Literal(_) => false,
        })
    }
    subschemas.is_some_and(reads) || reads(checks)
}

/// Returns a `const`. It is a unit struct that serializes to the value and decodes only it.
fn constant(items: &mut Vec<TokenStream>, def: &TypeDef, value: &serde_json::Value) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let (ty, v) = match value {
        serde_json::Value::String(x) => {
            let x = lit(x);
            (quote!(&'static str), quote!(#x))
        }
        serde_json::Value::Bool(b) => (quote!(bool), quote!(#b)),
        serde_json::Value::Number(x) if x.is_u64() => {
            let l = proc_macro2::Literal::u64_suffixed(x.as_u64().unwrap_or_default());
            (quote!(u64), quote!(#l))
        }
        serde_json::Value::Number(x) if x.is_i64() => {
            let l = proc_macro2::Literal::i64_suffixed(x.as_i64().unwrap_or_default());
            (quote!(i64), quote!(#l))
        }
        serde_json::Value::Number(x) => {
            let l = proc_macro2::Literal::f64_suffixed(x.as_f64().unwrap_or_default());
            (quote!(f64), quote!(#l))
        }
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => return structured_constant(items, def, value),
        serde_json::Value::Null => return,
    };
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct #n;
        impl #n {
            pub const VALUE: #ty = #v;
        }
        runtime::const_impls!(#n, VALUE);
    });
}

/// Returns a `const` of an object or a list: a unit struct that serializes to the value and decodes only it, as JSON
/// Schema compares values.
fn structured_constant(items: &mut Vec<TokenStream>, def: &TypeDef, value: &serde_json::Value) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let json = super::syntax::json_tokens(value);
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct #n;
        impl #n {
            /// The one value of this type.
            #[must_use]
            pub fn value() -> &'static serde_json::Value {
                static VALUE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
                VALUE.get_or_init(|| #json)
            }
        }
        runtime::const_impls!(#n);
    });
}
