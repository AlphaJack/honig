//! `types.rs`: one Rust item per `TypeDef` built as a syntax tree.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::syntax::{doc_opt, has_float, ident, lit, ty};

use crate::emit::rust::names::{pascal, snake};
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::names::Name;
use crate::ir::types::{EnumValue, Field, JsonKind, Kind, Struct, Ty, TypeDef, TypeId, Union, UnionMode, def_types};

/// Returns `aliases`: from [`aliases`]. They are declared as `pub type` of the type that they repeat.
pub(super) fn emit(
    space: &Space<'_>,
    xml: bool,
    extra: &[(String, Vec<String>)],
    aliases: &std::collections::BTreeMap<usize, usize>,
    deprecated: bool,
    binary: bool,
) -> TokenStream {
    let mut items = vec![];
    let cx = super::validate::Cx { space, aliases, live: &super::validate::live_types(space, aliases) };
    for (i, def) in space.types.defs.iter().enumerate() {
        let id = TypeId(i);
        if let Some(path) = &def.rust_type {
            external(&mut items, def, path);
            continue;
        }
        let name = pascal_ident(&def.name);
        if let Some(&first) = aliases.get(&i) {
            let (doc, first) = (doc_opt(def.doc.as_deref()), pascal_ident(&space.types.defs[first].name));
            items.push(quote! { #doc pub type #name = #first; });
            continue;
        }
        let first = items.len();
        match &def.kind {
            Kind::Struct(s) => {
                structure(&mut items, space, id, s);
                if xml {
                    items.push(xml_shape(space, def, s));
                }
            }
            Kind::StrEnum(values) => str_enum(&mut items, def, values),
            Kind::IntEnum(values) => int_enum(&mut items, def, values),
            Kind::Const(value) => constant(&mut items, def, value),
            Kind::Pattern(c) => pattern_type(&mut items, def, c),
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
        // Extra derives from the settings go on the declaration of the type, the first item.
        if let Some(attr) = extra_derives(def, extra)
            && let Some(item) = items.get_mut(first)
        {
            *item = quote!(#attr #item);
        }
        if def.deprecated
            && let Some(item) = items.get_mut(first)
        {
            *item = quote!(#[deprecated] #item);
        }
        // After the declaration so that the type reads first.
        items.extend(super::validate::type_impl(cx, def));
    }
    let allow = super::syntax::file_allow(deprecated);
    // A library exports the constructors. A binary may build a type only by decoding it.
    let unused = binary.then(|| quote!(#![allow(dead_code, reason = "a binary may build none of the types by hand")]));
    let header = quote!(#allow #unused);
    let enum_macros = enum_macros(space);
    // A description without schemas has no type to validate.
    let runtime_import = (!items.is_empty()).then(|| {
        quote!(
            use super::runtime;
        )
    });
    quote! { #header #runtime_import #enum_macros #(#items)* }
}

/// Inlines types declared as an alias of an identical earlier one (one inline schema per operation): index to the
/// first. A schema of `components/schemas` is never one, nor does an inline one alias it.
/// A twin with derives of its own or a deprecation keeps its own declaration.
pub(super) fn aliases(space: &Space<'_>, extra: &[(String, Vec<String>)]) -> std::collections::BTreeMap<usize, usize> {
    let mut twins = twins(space);
    twins.retain(|&i, _| extra_derives(&space.types.defs[i], extra).is_none() && !space.types.defs[i].deprecated);
    twins
}

/// Returns a better name for the type that its twins alias: the longest ending of two words or more that most twins'
/// names share. The 48 `<Operation>Ok` envelopes that each spell out `usage` alias `OkUsage` and not
/// `ListShardKeysOkUsage`, the first one built. Bigger groups pick first. A name another type has is skipped for the
/// next ending. When one twin is just those words it is the schema they all repeat, so the group keeps its name.
pub(super) fn shared_names(
    space: &Space<'_>,
    aliases: &std::collections::BTreeMap<usize, usize>,
) -> Vec<(usize, Name)> {
    let defs = &space.types.defs;
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = std::collections::BTreeMap::new();
    for (&twin, &first) in aliases {
        groups.entry(first).or_insert_with(|| vec![first]).push(twin);
    }
    let mut taken: std::collections::BTreeSet<String> = defs.iter().map(|d| d.name.key()).collect();
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
/// References to twins read as references to the first. So a struct of twin enums is a twin too. Repeated until
/// nothing changes.
fn twins(space: &Space<'_>) -> std::collections::BTreeMap<usize, usize> {
    let mut first: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    loop {
        let canon = |id: TypeId| TypeId(first.get(&id.0).copied().unwrap_or(id.0));
        // References to a twin read as references to its first.
        // The last word of the name is the role (`type`, `usage`, `Ok`): twins with different roles stay apart, so a
        // send's `type` does not read as a cipher's `reprompt` although both are `enum: [0, 1]`.
        let key = |def: &TypeDef| {
            let role = def.name.words().last().cloned().unwrap_or_default();
            format!("{:?}|{:?}|{}|{role}", canonical_kind(&def.kind, &canon), def.xml, def.json_string)
        };
        let mut seen: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        let mut next = std::collections::BTreeMap::new();
        for (i, def) in space.types.defs.iter().enumerate() {
            // A schema the description names is its own type: `Kdf` and `FileUploadType` may both be `enum: [0, 1]`.
            if def.component || def.rust_type.is_some() || matches!(def.kind, Kind::Pending | Kind::Alias(_)) {
                continue;
            }
            match seen.entry(key(def)) {
                std::collections::btree_map::Entry::Occupied(e) => {
                    next.insert(i, *e.get());
                }
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(i);
                }
            }
        }
        if next == first {
            return first;
        }
        first = next;
    }
}

/// Returns `kind` with every named type it refers to read through `canon`.
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
        }
        Kind::Union(u) => {
            for v in &mut u.variants {
                v.ty = canonical_ty(&v.ty, canon);
            }
        }
        Kind::Alias(t) | Kind::Newtype(t) => *t = canonical_ty(t, canon),
        Kind::StrEnum(_) | Kind::IntEnum(_) | Kind::Const(_) | Kind::Pattern(_) | Kind::Pending => {}
    }
    kind
}

/// Returns `t` with every named type read through `canon`.
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

/// Returns the trait impls that every string or integer enum shares. Each is written once as a macro. So generated code
/// holds no two identical function bodies. Only the kinds that the description has.
fn enum_macros(space: &Space<'_>) -> TokenStream {
    let has = |f: fn(&Kind) -> bool| space.types.defs.iter().any(|d| f(&d.kind));
    let str_enums = has(|k| matches!(k, Kind::StrEnum(_))).then(|| {
        quote! {
            macro_rules! str_enum_impls {
                ($n:ident) => {
                    impl std::fmt::Display for $n {
                        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            f.write_str(self.as_str())
                        }
                    }
                    impl std::str::FromStr for $n {
                        type Err = std::convert::Infallible;
                        fn from_str(s: &str) -> Result<Self, Self::Err> {
                            Ok(Self::from(s))
                        }
                    }
                    impl serde::Serialize for $n {
                        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                            s.serialize_str(self.as_str())
                        }
                    }
                    impl<'de> serde::Deserialize<'de> for $n {
                        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                            let s = <String as serde::Deserialize>::deserialize(d)?;
                            Ok(Self::from(s.as_str()))
                        }
                    }
                };
            }
        }
    });
    let int_enums = has(|k| matches!(k, Kind::IntEnum(_))).then(|| {
        quote! {
            macro_rules! int_enum_impls {
                ($n:ident) => {
                    impl std::fmt::Display for $n {
                        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                            write!(f, "{}", self.value())
                        }
                    }
                    impl serde::Serialize for $n {
                        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                            s.serialize_i64(self.value())
                        }
                    }
                    impl<'de> serde::Deserialize<'de> for $n {
                        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                            <i64 as serde::Deserialize>::deserialize(d).map(Self::from)
                        }
                    }
                };
            }
        }
    });
    quote!(#str_enums #int_enums)
}

/// Emits `x-rust-type` as `pub type Name = your::Type;`. Validation of its constraints
/// is yours (a no-op `Validate`).
fn external(items: &mut Vec<TokenStream>, def: &TypeDef, path: &str) {
    let name = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let segs = path.trim_start_matches("::").split("::").map(ident);
    let lead = path.starts_with("::").then(|| quote!(::));
    items.push(quote! {
        #doc
        pub type #name = #lead #(#segs)::*;
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
            &["Debug", "Clone", "PartialEq", "Eq", "Hash", "PartialOrd", "Ord", "Serialize", "Deserialize"]
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
        Kind::Alias(_) | Kind::Pattern(_) | Kind::Pending => return None,
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
        let paths = paths.iter().map(|p| {
            let segs = p.split("::").map(ident);
            quote!(#(#segs)::*)
        });
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
        let (wire, name, prefix) = (lit(&f.wire), lit(elem), lit(f.xml.prefix.as_deref().unwrap_or("")));
        let item = lit(f.xml.item.as_deref().unwrap_or(elem));
        let wrapped = list && (f.xml.wrapped || ambiguous(f));
        let (attribute, text, required) = (f.xml.attribute, f.xml.text, f.required);
        // `nodeType: none` on an array means no wrapper (an unwrapped list):
        // only an object's fields sit inline in the parent.
        let (cdata, inline) = (f.xml.cdata, f.xml.inline && !list);
        // An omitted attribute of a property that may be `null` reads as `null`; an empty element of a free-form
        // object as `{}`.
        let nullable = f.ty.is_option();
        let object = matches!(inner, Ty::Map(_));
        let child = if let Ty::Named(id) = inner
            && matches!(space.types.get(*id).kind, Kind::Struct(_))
        {
            let c = pascal_ident(&space.types.get(*id).name);
            quote!(Some(<#c as runtime::XmlShape>::xml))
        } else {
            quote!(None)
        };
        quote! {
            runtime::XmlField {
                wire: #wire, name: #name, prefix: #prefix, attribute: #attribute, text: #text,
                list: #list, wrapped: #wrapped, required: #required, item: #item, child: #child,
                cdata: #cdata, inline: #inline, nullable: #nullable, object: #object,
            }
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

/// Returns the literal `v` as an expression of type `t` (`qualify`: `types::` paths)
/// if it fits. That is scalars and string/integer enums through aliases.
pub(super) fn default_expr(space: &Space<'_>, t: &Ty, v: &serde_json::Value, qualify: bool) -> Option<TokenStream> {
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
        (Ty::Named(id), _) => return enum_default(&space.types.get(*id).kind, &ty(space, t, qualify), v),
        _ => return None,
    })
}

/// Returns the default of an enum. A listed value is its variant. So a `const fn` can set it. Any other value goes
/// through `From` into `Other`.
fn enum_default(kind: &Kind, t: &TokenStream, v: &serde_json::Value) -> Option<TokenStream> {
    let variant = match (kind, v) {
        (Kind::StrEnum(values), serde_json::Value::String(x)) => {
            values.iter().find(|e| e.wire == *x).map(|e| pascal(&e.name))
        }
        (Kind::IntEnum(values), serde_json::Value::Number(n)) => {
            let wanted = n.as_i64()?;
            values.iter().find(|(_, value)| *value == wanted).map(|(name, _)| pascal(name))
        }
        _ => return None,
    };
    if let Some(name) = variant {
        let name = ident(&name);
        return Some(quote!(#t::#name));
    }
    Some(match v {
        serde_json::Value::String(x) => {
            let x = lit(x);
            quote!(#t::from(#x))
        }
        serde_json::Value::Number(n) => {
            let l = proc_macro2::Literal::i64_suffixed(n.as_i64()?);
            quote!(#t::from(#l))
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => return None,
    })
}

/// Returns the spec default of a field as an expression when it has one that fits.
fn field_default(space: &Space<'_>, f: &Field) -> Option<TokenStream> {
    default_expr(space, &f.ty, f.default.as_ref()?, false)
}

/// Returns whether `f`'s `default_<field>` can be a `const fn`. A literal is; `to_owned` and an enum's `From` are not.
fn const_default(space: &Space<'_>, f: &Field) -> bool {
    !matches!(space.types.unalias(&f.ty), Ty::String | Ty::Named(_))
}

/// Does every required field of `s` have a `Default`? (`seen` guards cycles.)
fn struct_default(space: &Space<'_>, s: &Struct, seen: &mut Vec<TypeId>) -> bool {
    // A field with a spec default is filled by its `default_<field>` function.
    s.fields.iter().filter(|f| is_arg(space, f)).all(|f| ty_default(space, &f.ty, seen))
}

/// Is `f` required with no spec default? Then `new` takes it as an argument.
fn is_arg(space: &Space<'_>, f: &Field) -> bool {
    f.required && field_default(space, f).is_none()
}

/// Returns `impl Default`. Spec defaults go through their functions. A required unit struct is itself. The rest is
/// empty.
fn default_impl(space: &Space<'_>, name: &proc_macro2::Ident, s: &Struct, extra: &proc_macro2::Ident) -> TokenStream {
    let inits = s.fields.iter().map(|f| {
        let (n, t) = (snake_ident(&f.name), field_ty(space, f));
        let unit =
            matches!(space.types.unalias(&f.ty), Ty::Named(c) if matches!(space.types.get(*c).kind, Kind::Const(_)));
        if field_default(space, f).is_some() {
            let fun = format_ident!("default_{}", snake(&f.name));
            quote!(#n: Self::#fun(),)
        } else if f.required && unit {
            quote!(#n: #t,)
        } else {
            quote!(#n: <#t>::default(),)
        }
    });
    let extra_init = s.additional.is_some().then(|| no_extra(extra));
    quote! {
        impl Default for #name {
            fn default() -> Self { Self { #(#inits)* #extra_init } }
        }
    }
}

/// Does the Rust type of `t` implement `Default` as generated?
fn ty_default(space: &Space<'_>, t: &Ty, seen: &mut Vec<TypeId>) -> bool {
    use crate::ir::types::Format;
    if let Ty::Named(id) = t {
        // Your own type (`x-rust-type`) may not implement `Default`.
        if seen.contains(id) || space.types.get(*id).rust_type.is_some() {
            return false;
        }
        seen.push(*id);
        let ok = match &space.types.get(*id).kind {
            Kind::Struct(st) => struct_default(space, st, seen),
            Kind::Alias(x) => ty_default(space, x, seen),
            // The unit struct's only value.
            Kind::Const(_) => true,
            // A checked newtype has no valid empty value.
            Kind::Pattern(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Union(_)
            | Kind::Newtype(_)
            | Kind::Pending => false,
        };
        seen.pop();
        return ok;
    }
    if let Ty::Format(f) = t {
        // `url::Url` and `std::net`'s `Ipv4Addr` and `Ipv6Addr` have no `Default`. A mail address and a host name
        // have no value that every one of their kind could start from either. The uuid, chrono, jiff and ipnet format
        // types all implement it.
        return !((*f == Format::Uri && space.libs.url)
            || matches!(
                f,
                Format::Email | Format::IdnEmail | Format::Hostname | Format::IdnHostname | Format::Ipv4 | Format::Ipv6
            ));
    }
    // A list, a map and an option start empty. `[T; N]` is `Default` up to 32 items. `fixed_len` guarantees that.
    matches!(t, Ty::Vec(_) | Ty::Map(_) | Ty::Option(_)) || t.children().iter().all(|x| ty_default(space, x, seen))
}

/// Returns `#[derive(...)]` with `Eq` whenever no float is reachable.
fn derives(space: &Space<'_>, id: TypeId, default: bool) -> TokenStream {
    let eq = (!has_float(space, &Ty::Named(id))).then(|| quote!(Eq,));
    let default = default.then(|| quote!(Default,));
    quote!(#[derive(Debug, Clone, PartialEq, #eq #default serde::Serialize, serde::Deserialize)])
}

/// Returns a struct field with doc, serde attributes and declaration.
fn field(space: &Space<'_>, f: &Field, owner: &str) -> TokenStream {
    let doc = doc_opt(f.doc.as_deref());
    let doc = if f.deprecated { quote!(#doc #[deprecated]) } else { doc };
    let (name, t) = (snake_ident(&f.name), field_ty(space, f));
    // serde already uses the field name (a raw `r#type` as `type`).
    let wire = lit(&f.wire);
    let rename = (f.wire != snake(&f.name)).then(|| quote!(rename = #wire));
    let serde = |rest: &[TokenStream]| {
        let all: Vec<&TokenStream> = rename.iter().chain(rest).collect();
        (!all.is_empty()).then(|| quote!(#[serde(#(#all),*)]))
    };
    let attr = if field_default(space, f).is_some() {
        let fun = lit(&format!("{owner}::default_{}", snake(&f.name)));
        serde(&[quote!(default = #fun)])
    } else if f.required && space.types.nullable(&f.ty).is_some() {
        // serde reads a missing `Option` as `None`: its own deserializer makes the property required again.
        serde(&[quote!(deserialize_with = "runtime::required_nullable")])
    } else if f.required {
        serde(&[])
    } else if space.types.nullable(&f.ty).is_some() {
        serde(&[quote!(default), quote!(skip_serializing_if = "runtime::Nullable::is_absent")])
    } else if f.null_as_absent {
        // serde's own `Option`: `null` and absent both read as `None`.
        serde(&[quote!(default), quote!(skip_serializing_if = "Option::is_none")])
    } else {
        // `null` is no value of a property that is not nullable whether it is absent or not.
        let not_null = quote!(deserialize_with = "runtime::not_null");
        serde(&[quote!(default), quote!(skip_serializing_if = "Option::is_none"), not_null])
    };
    quote! { #doc #attr pub #name: #t, }
}

/// Returns the empty undeclared-properties map.
fn no_extra(extra: &proc_macro2::Ident) -> TokenStream {
    quote!(#extra: std::collections::BTreeMap::new(),)
}

/// Returns the Rust type of a struct field. It is its own when required or defaulted. Otherwise it is `Option` or
/// `runtime::Nullable` when it also accepts `null`.
fn field_ty(space: &Space<'_>, f: &Field) -> TokenStream {
    if f.required || field_default(space, f).is_some() {
        return ty(space, &f.ty, false);
    }
    match space.types.nullable(&f.ty) {
        Some(inner) => {
            let inner = ty(space, inner, false);
            quote!(runtime::Nullable<#inner>)
        }
        None => {
            let t = ty(space, &f.ty, false);
            quote!(Option<#t>)
        }
    }
}

fn structure(items: &mut Vec<TokenStream>, space: &Space<'_>, id: TypeId, s: &Struct) {
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
    let extra = ident(&extra_field(s));
    let additional = s.additional.as_ref().map(|t| {
        let t = ty(space, t, false);
        quote! {
            #[serde(flatten)]
            pub #extra: std::collections::BTreeMap<String, #t>,
        }
    });
    // `additionalProperties: false`: an undeclared property is a decode error,
    // so a server answers 400 and a client reports a server off its contract.
    let closed = (s.closed && s.additional.is_none()).then(|| quote!(#[serde(deny_unknown_fields)]));
    // `remote = "Self"` turns the derives into inherent functions. The trait impls call them, decoding through
    // `NoSeq`: a derived struct would also read a JSON array field by field, and `type: object` allows none.
    // An extra map encodes through `UniqueKeys`: an entry named like a declared field would write the name twice.
    let encoder = if s.additional.is_some() { quote!(runtime::UniqueKeys(s)) } else { quote!(s) };
    items.push(quote! {
        #doc
        #derive
        #[serde(remote = "Self")]
        #closed
        pub struct #name {
            #(#fields)*
            #additional
        }
        impl<'de> serde::Deserialize<'de> for #name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::deserialize(runtime::NoSeq(d))
            }
        }
        impl serde::Serialize for #name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                Self::serialize(self, #encoder)
            }
        }
    });
    items.push(struct_new(space, &name, s, &extra));
    if !defaults.is_empty() {
        let funs = defaults.iter().map(|(f, e)| {
            let (fun, t) = (format_ident!("default_{}", snake(&f.name)), ty(space, &f.ty, false));
            let constness = const_default(space, f).then(|| quote!(const));
            // Serde's `default = "..."` needs a path; users read defaults through `Default`.
            quote!(#[doc(hidden)] #[must_use] pub #constness fn #fun() -> #t { #e })
        });
        items.push(quote!(impl #name { #(#funs)* }));
        if can_default {
            items.push(default_impl(space, &name, s, &extra));
        }
    }
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
        } else if field_default(space, f).is_some() {
            let fun = format_ident!("default_{}", snake(&f.name));
            quote!(#n: Self::#fun(),)
        } else if space.types.nullable(&f.ty).is_some() {
            quote!(#n: runtime::Nullable::Absent,)
        } else {
            quote!(#n: None,)
        }
    });
    let extra_init = s.additional.is_some().then(|| no_extra(extra));
    let count = s.fields.iter().filter(|f| arg(f)).count();
    let many = (count > super::syntax::MAX_ARGUMENTS)
        .then(|| quote!(#[allow(clippy::too_many_arguments, reason = "one argument per required property")]));
    let constness =
        s.fields.iter().all(|f| field_default(space, f).is_none() || const_default(space, f)).then(|| quote!(const));
    quote! {
        impl #name {
            #[must_use]
            #many
            pub #constness fn new(#(#args),*) -> Self {
                Self { #(#inits)* #extra_init }
            }
        }
    }
}

fn extra_field(s: &Struct) -> String {
    let mut name = "additional_properties".to_owned();
    while s.fields.iter().any(|f| snake(&f.name) == name) {
        name.push('_');
    }
    name
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
        impl serde::Serialize for #name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let text = serde_json::to_string(&self.0).map_err(serde::ser::Error::custom)?;
                s.serialize_str(&text)
            }
        }
        impl<'de> serde::Deserialize<'de> for #name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = <String as serde::Deserialize>::deserialize(d)?;
                runtime::json_text(&text).map(Self).map_err(serde::de::Error::custom)
            }
        }
    });
}

/// Emits an open integer enum with one variant per listed value and `Other(i64)` for the rest.
fn int_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[(Name, i64)]) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let names: Vec<_> = values.iter().map(|(v, _)| pascal_ident(v)).collect();
    let nums: Vec<_> = values.iter().map(|(_, x)| proc_macro2::Literal::i64_unsuffixed(*x)).collect();
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum #n {
            #(#names,)*
            Other(i64),
        }
        impl #n {
            #[must_use]
            pub const fn value(self) -> i64 {
                match self {
                    #(Self::#names => #nums,)*
                    Self::Other(v) => v,
                }
            }
        }
        impl From<i64> for #n {
            fn from(v: i64) -> Self {
                match v {
                    #(#nums => Self::#names,)*
                    other => Self::Other(other),
                }
            }
        }
        int_enum_impls!(#n);
    });
}

/// Returns a `const`. It is a unit struct that serializes to the value and decodes only it.
/// The pattern newtype and its `TryFrom`, `AsRef<str>`, `Display` and `Deserialize`.
/// No `Deref`. It would put every `str` method on the type next to `as_str`.
fn pattern_impls(
    n: &proc_macro2::Ident,
    doc: &TokenStream,
    pat: &proc_macro2::Literal,
    min: &TokenStream,
    max: &TokenStream,
) -> TokenStream {
    quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
        #[serde(transparent)]
        pub struct #n(String);
        impl #n {
            pub const PATTERN: &'static str = #pat;
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }
        impl TryFrom<String> for #n {
            type Error = String;
            fn try_from(value: String) -> Result<Self, String> {
                static RE: runtime::Pattern = runtime::Pattern::new();
                let mut out = Vec::new();
                runtime::check_len(&mut out, &runtime::InstancePath::Root, value.chars().count(), #min, #max, "characters");
                if let Some(v) = out.first() {
                    return Err(v.message.clone());
                }
                if !runtime::pattern_matches(&RE, Self::PATTERN, &value) {
                    return Err(format!("`{value}` does not match `{}`", Self::PATTERN));
                }
                Ok(Self(value))
            }
        }
        impl TryFrom<&str> for #n {
            type Error = String;
            fn try_from(value: &str) -> Result<Self, String> {
                Self::try_from(value.to_owned())
            }
        }
        impl AsRef<str> for #n {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
        impl std::fmt::Display for #n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl<'de> serde::Deserialize<'de> for #n {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <String as serde::Deserialize>::deserialize(d)?;
                Self::try_from(s).map_err(serde::de::Error::custom)
            }
        }
    }
}

/// Emits `pub struct Name(pub T);` that is transparent on the wire.
fn newtype(items: &mut Vec<TokenStream>, space: &Space<'_>, id: TypeId, def: &TypeDef, t: &Ty) {
    let name = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let derive = derives(space, id, false);
    let t = ty(space, t, false);
    items.push(quote! {
        #doc
        #derive
        #[serde(transparent)]
        pub struct #name(pub #t);
    });
}

/// Emits a string with a `pattern`. It is a newtype that only `TryFrom` and decoding build. They check the pattern and
/// the length limits. So an invalid value cannot exist.
fn pattern_type(items: &mut Vec<TokenStream>, def: &TypeDef, c: &crate::ir::types::Constraints) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let pattern = c.pattern.as_deref().unwrap_or_default();
    let pat = lit(pattern);
    let opt = |v: Option<u64>| v.map_or_else(|| quote!(None), |m| quote!(Some(#m)));
    let (min, max) = (opt(c.min_length), opt(c.max_length));
    items.push(pattern_impls(&n, &doc, &pat, &min, &max));
}

fn constant(items: &mut Vec<TokenStream>, def: &TypeDef, value: &serde_json::Value) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let (ty, v, ser) = match value {
        serde_json::Value::String(x) => {
            let x = lit(x);
            (quote!(&'static str), quote!(#x), quote!(s.serialize_str(Self::VALUE)))
        }
        serde_json::Value::Bool(b) => (quote!(bool), quote!(#b), quote!(s.serialize_bool(Self::VALUE))),
        serde_json::Value::Number(x) if x.is_u64() => {
            let l = proc_macro2::Literal::u64_suffixed(x.as_u64().unwrap_or_default());
            (quote!(u64), quote!(#l), quote!(s.serialize_u64(Self::VALUE)))
        }
        serde_json::Value::Number(x) if x.is_i64() => {
            let l = proc_macro2::Literal::i64_suffixed(x.as_i64().unwrap_or_default());
            (quote!(i64), quote!(#l), quote!(s.serialize_i64(Self::VALUE)))
        }
        serde_json::Value::Number(x) => {
            let l = proc_macro2::Literal::f64_suffixed(x.as_f64().unwrap_or_default());
            (quote!(f64), quote!(#l), quote!(s.serialize_f64(Self::VALUE)))
        }
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => return,
    };
    let json = super::syntax::json_tokens(value);
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct #n;
        impl #n {
            pub const VALUE: #ty = #v;
        }
        impl serde::Serialize for #n {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                #ser
            }
        }
        impl<'de> serde::Deserialize<'de> for #n {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                runtime::expect_const(d, &#json).map(|()| Self)
            }
        }
    });
}

fn str_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[EnumValue]) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let variants = values.iter().map(|v| {
        let d = doc_opt(v.doc.as_deref());

        let name = pascal_ident(&v.name);
        quote!(#d #name,)
    });
    let names: Vec<_> = values.iter().map(|v| pascal_ident(&v.name)).collect();
    let wires: Vec<_> = values.iter().map(|v| lit(&v.wire)).collect();
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum #n {
            #(#variants)*
            Other(String),
        }
        impl #n {
            #[must_use]
            pub const fn as_str(&self) -> &str {
                match self {
                    #(Self::#names => #wires,)*
                    Self::Other(s) => s.as_str(),
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
        str_enum_impls!(#n);
    });
}

fn union(
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
    let deserialize = union_deserialize_impl(space, &n, u);
    items.push(quote! {
        #doc
        #[derive(Debug, Clone, PartialEq, #eq)]
        pub enum #n {
            #(#variants)*
            Other(serde_json::Value),
        }
        impl serde::Serialize for #n {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                match self {
                    #ser
                    Self::Other(v) => serde::Serialize::serialize(v, s),
                }
            }
        }
        #deserialize
    });
}

/// Returns `impl Deserialize`. Tagged unions also implement `runtime::TaggedUnion`. So a
/// discriminator sent first streams straight into the variant.
fn union_deserialize_impl(space: &Space<'_>, n: &proc_macro2::Ident, u: &Union) -> TokenStream {
    let de = union_deserialize(space, u);
    // A buffered number is an `f64`. So a union that holds a `Decimal` decodes from the JSON text instead. An
    // untagged one pays one parse per branch that it tries. A tagged one reads its tag and then parses the one branch.
    let exact = u.variants.iter().any(|v| holds_decimal(space, &v.ty, &mut vec![]));
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
        quote!(#(#tags)|* => <#t as serde::Deserialize>::deserialize(d).map(Self::#name),)
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

/// Does a value of `t` hold a `Decimal` itself or anywhere inside? `seen` holds the named types on the way. A
/// recursive type reaches them again.
fn holds_decimal(space: &Space<'_>, t: &Ty, seen: &mut Vec<TypeId>) -> bool {
    if let Ty::Named(id) = t {
        if seen.contains(id) {
            return false;
        }
        seen.push(*id);
        return def_types(&space.types.get(*id).kind).into_iter().any(|x| holds_decimal(space, x, seen));
    }
    *t == Ty::Decimal || t.children().iter().any(|x| holds_decimal(space, x, seen))
}

/// Does the variant type declare the discriminator itself (a required
/// string property)? Then it serializes directly without a buffer.
fn declares_tag(space: &Space<'_>, t: &Ty, property: &str) -> bool {
    let t = if let Ty::Indirect(inner) = t { inner } else { t };
    let Ty::Named(id) = t else { return false };
    matches!(&space.types.get(*id).kind, Kind::Struct(s) if s.fields.iter().any(|f| f.wire == property && f.required && f.ty == Ty::String))
}

fn union_serialize(space: &Space<'_>, aliases: &std::collections::BTreeMap<usize, usize>, u: &Union) -> TokenStream {
    let arms = u.variants.iter().map(|v| {
        let (name, direct) = (pascal_ident(&v.name), quote!(serde::Serialize::serialize(v, s)));
        let same = Some(space.resolved(aliases, &v.ty));
        match (&u.mode, v.tags.first()) {
            // The struct carries the tag: its value is the caller's to keep right.
            (UnionMode::Tagged { property, .. }, Some(_)) if declares_tag(space, &v.ty, property) => {
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
    let arms = super::syntax::shared_arms(arms.collect());
    quote!(#(#arms)*)
}

fn conv(name: &str) -> TokenStream {
    let name = ident(name);
    quote!(v.decode_into(Self::#name))
}

fn union_deserialize(space: &Space<'_>, u: &Union) -> TokenStream {
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
                let (i, c) = (proc_macro2::Literal::usize_unsuffixed(i), conv(&pascal(&v.name)));
                quote!(#i => #c,)
            });
            let fallback = match default {
                Some(i) => conv(&pascal(&u.variants[*i].name)),
                None => quote!(Ok(Self::Other(v.into_value()))),
            };
            let none = proc_macro2::Literal::usize_unsuffixed(tagged.len());
            quote! {
                let which = match v.tag(#property).as_deref() {
                    #(#picks)*
                    _ => #none,
                };
                match which {
                    #(#arms)*
                    _ => #fallback,
                }
            }
        }
        UnionMode::ByKind => by_kind(space, u),
        UnionMode::Untagged => untagged(space, u),
    }
}

/// Tries each branch in order.
fn untagged(space: &Space<'_>, u: &Union) -> TokenStream {
    // What rules a branch out before it decodes: the JSON kind of the value and the fields of a struct. Each is
    // named `fit_<i>` and computed once.
    let gates: Vec<Option<TokenStream>> = u
        .variants
        .iter()
        .map(|v| {
            // serde lets a struct read an array by position. So a branch of a known kind only sees values of
            // that kind.
            let kind = match space.types.kind_of(&v.ty) {
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
    let lets = gates.iter().enumerate().filter_map(|(i, g)| {
        let (name, g) = (fit(i), g.as_ref()?);
        Some(quote!(let #name = #g;))
    });
    let tries = u.variants.iter().enumerate().map(|(i, v)| {
        let (name, t) = (pascal_ident(&v.name), ty(space, &v.ty, false));
        // A union always decodes because its `Other` takes anything. As a branch it matches only through one
        // of its own variants. So the branches after it still get their turn. A boxed one too.
        let (inner, boxed) = if let Ty::Indirect(i) = &v.ty { (i.as_ref(), true) } else { (&v.ty, false) };
        let union = matches!(inner, Ty::Named(id) if matches!(space.types.get(*id).kind, Kind::Union(_)));
        let gate = gates[i].as_ref().map(|_| {
            let f = fit(i);
            quote!(#f &&)
        });
        let decoded = if union {
            let it = ty(space, inner, false);
            let x = if boxed { quote!(x.as_ref()) } else { quote!(x) };
            quote!(#gate let Ok(x) = v.decode_ref::<#t>() && !matches!(#x, #it::Other(_)))
        } else {
            quote!(#gate let Ok(x) = v.decode_ref::<#t>())
        };
        // A branch that decodes but fails its checks (an open enum, a `format`) is not a match: the union's
        // own checks of that variant decide, and the branches after it get their turn. A closed union keeps
        // the first that decoded, so that its violations name what is wrong. So when no later branch can fit,
        // a closed union's choice does not depend on the checks: it skips them. Checked at every level, a
        // recursive union checked each nested value once per level above it.
        let later = &gates[i + 1..];
        let checked = if u.open || later.iter().any(Option::is_none) {
            quote!(runtime::is_valid(&picked))
        } else {
            // Nothing held from an earlier branch and no later branch that can fit: this one is the answer.
            let fits = (i + 1..gates.len()).map(fit);
            if later.is_empty() {
                quote!(first.is_none() || runtime::is_valid(&picked))
            } else {
                quote!((first.is_none() && !(#(#fits)||*)) || runtime::is_valid(&picked))
            }
        };
        let keep = (!u.open).then(|| quote!(first.get_or_insert(picked);));
        let body = quote! {
            let picked = Self::#name(x);
            if #checked {
                return Ok(picked);
            }
            #keep
        };
        quote! {
            if #decoded {
                #body
            }
        }
    });
    // When none passes, an open union has a branch that takes any value: `Other` holds it.
    if u.open {
        quote! { #(#lets)* #(#tries)* Ok(Self::Other(v.into_value())) }
    } else {
        quote! {
            #(#lets)*
            let mut first = None;
            #(#tries)*
            Ok(first.unwrap_or_else(|| Self::Other(v.into_value())))
        }
    }
}

/// `runtime::fits(&v, ...)` for a struct branch: the fields that must be present and the kinds that each may have.
/// It never refuses a value that the branch decodes: a kind that a type reads from more than one JSON kind (a
/// `Decimal`, an untyped value, an open union) is not checked, an integer may come as a number, and an optional or
/// nullable field may be absent or `null`. `None` when there is nothing to check.
fn fits(space: &Space<'_>, t: &Ty) -> Option<TokenStream> {
    use rtkind::K;
    let Ty::Named(id) = t else { return None };
    let Kind::Struct(s) = &space.types.get(*id).kind else { return None };
    let checks: Vec<TokenStream> = s
        .fields
        .iter()
        .filter_map(|f| {
            let optional = space.types.nullable(&f.ty).is_some();
            let present = f.required && field_default(space, f).is_none() && !optional;
            let mut kinds: Vec<K> = match space.types.kind_of(&f.ty) {
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
fn reads_several_kinds(space: &Space<'_>, t: &Ty, depth: usize) -> bool {
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
            | Kind::Pattern(_)
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
fn by_kind(space: &Space<'_>, u: &Union) -> TokenStream {
    use rtkind::K;
    let mut kinds: Vec<(JsonKind, String, &Ty)> =
        u.variants.iter().map(|v| (space.types.kind_of(&v.ty), pascal(&v.name), &v.ty)).collect();
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
            // An open union has a branch that takes any value. A value of this kind that does not fit the
            // variant, or fails its checks, is that branch's: `Other` holds it.
            let (vn, t) = (ident(&name), ty(space, vty, false));
            quote!({
                if let Ok(x) = v.decode_ref::<#t>() {
                    let picked = Self::#vn(x);
                    if runtime::is_valid(&picked) {
                        return Ok(picked);
                    }
                }
                Ok(Self::Other(v.into_value()))
            })
        } else {
            conv(&name)
        };
        arms.push(quote!(#(#pats)|* => #c,));
    }
    let rest: Vec<TokenStream> = K::ALL.iter().filter(|k| !covered.contains(k)).map(|k| k.path()).collect();
    let other = (!rest.is_empty()).then(|| quote!(#(#rest)|* => Ok(Self::Other(v.into_value())),));
    quote! {
        match v.kind() {
            #(#arms)*
            #other
        }
    }
}

/// The JSON kinds of `runtime::Kind` as the emitter names them.
mod rtkind {
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
