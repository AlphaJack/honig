//! `types.rs`: one Rust item per `TypeDef` built as a syntax tree.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::syntax::{doc_opt, has_float, holds, ident, lit, ty};

use crate::emit::rust::names::{pascal, snake};
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::names::Name;
use crate::ir::types::{
    Checked, EnumValue, Field, Format, JsonKind, Kind, Struct, Ty, TypeDef, TypeId, Union, UnionMode,
};

/// Returns `aliases`: from [`aliases`]. They are declared as `pub type` of the type that they repeat.
pub(super) fn emit(
    space: &Space<'_>,
    xml: bool,
    extra: &[(String, Vec<String>)],
    aliases: &std::collections::BTreeMap<usize, usize>,
    binary: bool,
) -> TokenStream {
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
        match &def.kind {
            Kind::Struct(s) => {
                structure(&mut items, space, id, s);
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
        // Keys by their hash first: keys are long and share a long start, so ordering them by text compares that start
        // again on every step down the map. The text still decides which keys are equal.
        let mut seen: std::collections::BTreeMap<u64, Vec<(String, usize)>> = std::collections::BTreeMap::new();
        let mut next = std::collections::BTreeMap::new();
        for (i, def) in space.types.defs.iter().enumerate() {
            // A schema the description names is its own type: `Kdf` and `FileUploadType` may both be `enum: [0, 1]`.
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

/// Emits `x-rust-type` as `pub type Name = your::Type;`. Validation of its constraints
/// is yours (a no-op `Validate`).
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
fn literal_type(space: &Space<'_>, t: &Ty) -> bool {
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
fn literal_expr(space: &Space<'_>, t: &Ty, v: &serde_json::Value, qualify: bool) -> Option<TokenStream> {
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
fn enum_default(kind: &Kind, t: &TokenStream, v: &serde_json::Value) -> Option<TokenStream> {
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

/// Is `f` required with no spec default and more than one value? Then `new` takes it as an argument.
fn is_arg(space: &Space<'_>, f: &Field) -> bool {
    f.required && field_default(space, f).is_none() && !is_unit(space, f)
}

/// Is `f` a required `const`: a unit struct whose one value `new` and `Default` fill in?
fn is_unit(space: &Space<'_>, f: &Field) -> bool {
    f.required
        && matches!(space.types.unalias(&f.ty), Ty::Named(c) if matches!(space.types.get(*c).kind, Kind::Const(_)))
}

/// Returns `impl Default`. Spec defaults go through their functions. A required unit struct is its value, `Unit {}`. The
/// rest is empty.
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
            Kind::Checked(_)
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
        // types all implement it. So does `runtime::FullTime`.
        // Nor has the text of a format without its library: an empty one is not of the format.
        // Nor has a type that the table names: honig cannot know.
        return !(super::syntax::untyped(space.libs, *f)
            || space.libs.named.contains_key(f)
            || (*f == Format::Uri && space.libs.url)
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
/// Returns one field of `runtime::object!`: its kind, its name and its type, with the serde attributes that its kind
/// does not imply (a rename, a spec default, a reader of a library format).
fn field(space: &Space<'_>, f: &Field, owner: &str) -> TokenStream {
    let doc = doc_opt(f.doc.as_deref());
    let doc = if f.deprecated { quote!(#doc #[deprecated]) } else { doc };
    let name = snake_ident(&f.name);
    // serde already uses the field name (a raw `r#type` as `type`).
    let wire = lit(&f.wire);
    let rename = (f.wire != snake(&f.name)).then(|| quote!(#[serde(rename = #wire)]));
    // A library type that reads more than its format allows gets its text checked first. `wrap` holds it as the
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
        // serde reads a missing `Option` as `None`: its own deserializer makes the property required again.
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
        // serde's own `Option`: `null` and absent both read as `None`.
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

/// Returns the fields that hold the `patternProperties` of `s` ([`Struct::patterns`]) and the names that no pattern
/// matches, with the functions that read each from the undeclared names. Empty without patterns.
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

/// Returns the empty values of the fields of `s` that hold undeclared names.
fn extra_inits(s: &Struct, extra: &proc_macro2::Ident) -> Option<TokenStream> {
    if s.patterns.is_empty() {
        return s.additional.is_some().then(|| no_extra(extra));
    }
    let maps = (0..s.patterns.len()).map(|i| super::syntax::pattern_field(s, i));
    let other =
        if s.additional.is_some() { quote!(std::collections::BTreeMap::new()) } else { quote!(runtime::Closed) };
    Some(quote!(#(#maps: std::collections::BTreeMap::new(),)* #extra: #other,))
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
    // A lenient client's response type reads a property that the server added. Its request copy refuses it.
    let closed = (s.closed && s.additional.is_none() && s.patterns.is_empty() && !def.lenient)
        .then(|| quote!(#[serde(deny_unknown_fields)]));
    // `runtime::object!` writes the struct with `remote = "Self"` and the trait impls that decode through `NoSeq`: a
    // derived struct would also read a JSON array field by field, and `type: object` allows none. An extra map encodes
    // through `UniqueKeys`: an entry named like a declared field would write the name twice.
    let encode = (s.additional.is_some() || !s.patterns.is_empty()).then(|| quote!(#[unique_keys]));
    items.push(quote! {
        runtime::object! {
            #encode
            #doc
            #derive
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
            // Serde's `default = "..."` needs a path; users read defaults through `Default`.
            quote!(#[doc(hidden)] #[must_use] pub #constness fn #fun() -> #t { #e })
        });
        items.push(quote!(impl #name { #(#funs)* }));
        if can_default {
            items.push(default_impl(space, &name, s, &extra));
        }
    }
}

/// Can `new` take the required fields of `s` by position? Only when it cannot fail and no two of them can swap: no
/// argument holds a checked value, and no two have the same type. Otherwise the builder is the one constructor.
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
/// named `UserBuilder`. A builder takes the name of its type: the types are named through `types::`.
fn builders_mod(builders: &[TokenStream], names_types: bool) -> Option<TokenStream> {
    let types_import = names_types.then(|| {
        quote!(
            use super as types;
        )
    });
    (!builders.is_empty()).then(|| {
        quote! {
            /// One builder per struct, started by its `builder`. Each setter takes anything that converts to the type of
            /// its field. `build` returns the first field that did not convert.
            pub mod builders {
                #types_import
                use super::runtime;
                #(#builders)*
            }
        }
    })
}

/// Does `t` hold a value that only a checked conversion makes: a checked type, the text of a format without its
/// library, a mail address or host name, or one of them in a list, map, option or box? Each such field takes a `?`,
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

/// Does a field of `s` that its builder takes refer to a generated type? Then the builder refers to it through
/// `types::`. A required `const` it fills itself.
fn names_types(_space: &Space<'_>, s: &Struct) -> bool {
    s.fields.iter().any(|f| super::syntax::mentions(&f.ty).0)
}

/// Returns the builder of a checked type whose base is a struct, and whether it refers to a generated type. A struct whose
/// schema checks the whole value builds through its fields.
fn checked_builder(space: &Space<'_>, name: &proc_macro2::Ident, c: &Checked) -> Option<(TokenStream, bool)> {
    let Ty::Named(shape) = &c.base else { return None };
    let Kind::Struct(s) = &space.types.get(*shape).kind else { return None };
    let fields = pascal_ident(&space.types.get(*shape).name);
    let names_types = names_types(space, s);
    Some((builder(space, name, &fields, s), names_types))
}

/// Returns the builder of `name` for the `builders` module: `runtime::typestate_builder!` over the fields of `shape`.
/// It takes each property by name; `build` exists once every required one is set. `shape` is the struct that holds
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

/// Returns the `allow` of a constructor that takes `count` required properties, past what clippy allows. It goes inside
/// the body: `clippy::allow_attributes` leaves an inner `#![allow]` alone, and an `#[expect]` would fail a crate that
/// raises `too-many-arguments-threshold`.
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

/// Emits a closed integer enum with one variant per listed value. Any other value fails to convert or decode.
fn int_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[(Name, i64)]) {
    let n = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let names: Vec<_> = values.iter().map(|(v, _)| pascal_ident(v)).collect();
    let nums: Vec<_> = values.iter().map(|(_, x)| proc_macro2::Literal::i64_unsuffixed(*x)).collect();
    let allowed = lit(&one_of(values.iter().map(|(_, v)| v.to_string())));
    // A lenient client's response reads a value that the server added as `Other`. Its request copy is closed.
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

/// Returns the message of a value that a closed enum does not list: `{other}` takes the value. Braces in the values are
/// escaped.
fn one_of(values: impl Iterator<Item = String>) -> String {
    let allowed = values.map(|v| format!("`{v}`")).collect::<Vec<_>>().join(", ");
    format!("`{{other}}` is not one of {}", allowed.replace('{', "{{").replace('}', "}}"))
}

/// Emits `pub struct Name(pub T);` that is transparent on the wire.
fn newtype(items: &mut Vec<TokenStream>, space: &Space<'_>, id: TypeId, def: &TypeDef, t: &Ty) {
    let name = pascal_ident(&def.name);
    let doc = doc_opt(def.doc.as_deref());
    let derive = derives(space, id, false);
    // A library type has its text checked before the library reads it.
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
            // A member that the server added meets no keyword of the description: a lenient client takes it as it is.
            let added = matches!(&c.base, Ty::Named(inner) if {
                let inner = space.types.get(*inner);
                matches!(&inner.kind, Kind::Union(union) if reads_added(inner, union))
            });
            let checks = if added { quote!(if !matches!(value, #base::Other(_)) { #checks }) } else { checks };
            let eq = (!has_float(space, &Ty::Named(id))).then(|| quote!(Eq,));
            // The base reads the text of its library formats checked, as a field of it would.
            let read = match holds(space, &c.base) {
                Some(h) => quote!(runtime::formats::read::<_, #base, #h>(d)?),
                None => quote!(<#base as serde::Deserialize>::deserialize(d)?),
            };
            // JSON text is read once ([`checked_deserialize`]); a base read through a library format keeps `read`.
            let once = holds(space, &c.base).is_none() && reads_json(subschemas.as_ref(), &checks);
            let de = checked_deserialize(&n, &base, &read, once);
            (checks, quote!(#eq), Some(de))
        }
    };
    let shared = match kind {
        Base::Str => quote!(runtime::checked_str_impls!(#n);),
        Base::Decimal => quote!(runtime::checked_decimal_impls!(#n);),
        Base::Int | Base::Float => quote!(runtime::checked_num_impls!(#n, #base);),
        Base::Other => quote!(runtime::checked_impls!(#n, #base);),
    };
    let build = checked_try_from(&n, &base, subschemas.as_ref(), &checks);
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
/// into the value as JSON read one buffer of it, written out once.
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
    // The checks read the value as JSON: from the text that it was decoded from, or from the value written out once.
    quote! {
        impl TryFrom<#base> for #n {
            type Error = runtime::BuildError;
            fn try_from(value: #base) -> Result<Self, runtime::BuildError> {
                let json = runtime::to_value(&value).map_err(|e| runtime::BuildError::value(e.to_string()))?;
                Self::checked(value, &runtime::Content::of(&json))
            }
        }
        impl #n {
            fn checked(value: #base, json: &runtime::Content<'_>) -> Result<Self, runtime::BuildError> {
                #body
            }
        }
    }
}

/// Returns `Deserialize` of the checked type `n` of `base`, read by `read`. With `once`, JSON text is read once: the
/// value decodes from it and the checks read it. Any other source of the value decodes as `read` does, and the checks
/// read the value written out once.
fn checked_deserialize(n: &proc_macro2::Ident, base: &TokenStream, read: &TokenStream, once: bool) -> TokenStream {
    if !once {
        return quote! {
            impl<'de> serde::Deserialize<'de> for #n {
                fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    let value = #read;
                    Self::try_from(value).map_err(serde::de::Error::custom)
                }
            }
        };
    }
    quote! {
        impl<'de> serde::Deserialize<'de> for #n {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let (value, text) = runtime::read_once::<_, #base>(d)?;
                match text {
                    Some(text) => {
                        let json = runtime::Content::parse(text).map_err(serde::de::Error::custom)?;
                        Self::checked(value, &json)
                    }
                    None => Self::try_from(value),
                }
                .map_err(serde::de::Error::custom)
            }
        }
    }
}

/// Do `checks` read `json`, the checked value as a buffer that the checked type binds? A block that binds its own
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
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => return structured_constant(items, def, value),
        serde_json::Value::Null => return,
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
        impl serde::Serialize for #n {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                serde::Serialize::serialize(Self::value(), s)
            }
        }
        impl<'de> serde::Deserialize<'de> for #n {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                runtime::expect_const(d, Self::value()).map(|()| Self)
            }
        }
    });
}

fn str_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[EnumValue]) {
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
const OTHER_DOC: &str = " A value that the description does not list: the server added it. It is sent back as it came.";

/// Emits the string enum of a lenient client's response: a value that it does not list is `Other`, so a value that
/// the server added reads instead of failing the response. Its request copy is closed.
fn open_str_enum(items: &mut Vec<TokenStream>, def: &TypeDef, values: &[EnumValue]) {
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
const fn has_other(u: &Union) -> bool {
    u.open || u.rest
}

/// Does `Other` hold a member that the server added? Only the response type of a lenient client's tagged union does:
/// its tag matches no variant. A union with a default variant reads an unknown tag as that variant already.
const fn reads_added(def: &TypeDef, u: &Union) -> bool {
    def.lenient && matches!(u.mode, UnionMode::Tagged { default: None, .. })
}

/// Returns the value that no variant of the union takes: `Other` holds it when the union has one. Else decoding fails.
fn no_variant(u: &Union) -> TokenStream {
    if has_other(u) {
        quote!(Ok(Self::Other(v.into_value())))
    } else {
        quote!(Err(serde::de::Error::custom("matches no variant")))
    }
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
fn union_deserialize_impl(space: &Space<'_>, n: &proc_macro2::Ident, def: &TypeDef, u: &Union) -> TokenStream {
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

/// Returns the buffered value `v` decoded into variant `name` of type `t`. A library type has its text checked first.
fn conv(space: &Space<'_>, name: &str, t: &Ty) -> TokenStream {
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
fn decode_ref(space: &Space<'_>, t: &Ty) -> TokenStream {
    let rust = ty(space, t, false);
    match holds(space, t) {
        Some(h) => {
            quote!(v.decode_ref::<runtime::formats::Validated<#rust, #h>>().map(runtime::formats::Validated::into_inner))
        }
        None => quote!(v.decode_ref::<#rust>()),
    }
}

fn union_deserialize(space: &Space<'_>, def: &TypeDef, u: &Union) -> TokenStream {
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
fn untagged(space: &Space<'_>, u: &Union) -> TokenStream {
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
fn counting(counts: bool) -> (TokenStream, TokenStream) {
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
fn by_kind(space: &Space<'_>, u: &Union) -> TokenStream {
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
