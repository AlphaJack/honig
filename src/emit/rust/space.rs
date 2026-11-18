//! The type space as the Rust emitter sees it. That is the types of the IR with what only Rust output depends on.

use std::collections::BTreeMap;

use crate::ir::TypeSpace;
use crate::ir::types::{Kind, Ty, TypeId, all_refs, def_types};
use crate::settings::FormatLibs;

/// The types of the IR, the libraries that the target crate provides for `Ty::Format` and which types hold a float.
pub(crate) struct Space<'a> {
    /// Every named type.
    pub types: &'a TypeSpace,
    /// Libraries the target crate provides for `Ty::Format`.
    pub libs: FormatLibs,
    /// Which definitions hold a float somewhere by `TypeId`.
    floats: Vec<bool>,
    /// The rough size of each definition by `TypeId` ([`Space::approx_size`]).
    sizes: Vec<usize>,
}

impl<'a> Space<'a> {
    /// Is `t` an `x-rust-type`, a type of yours, maybe behind a `Box`?
    pub(crate) fn is_external(&self, t: &Ty) -> bool {
        match t {
            Ty::Indirect(x) => self.is_external(x),
            Ty::Named(id) => self.types.get(*id).rust_type.is_some(),
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
            | Ty::Format(_)
            | Ty::Any
            | Ty::Unit
            | Ty::Vec(_)
            | Ty::Array(..)
            | Ty::Tuple(_)
            | Ty::Map(_)
            | Ty::Option(_) => false,
        }
    }

    /// Creates the view of the emitter on `types` for a crate with `libs`.
    pub(crate) fn new(types: &'a TypeSpace, libs: FormatLibs) -> Self {
        Self { types, libs, floats: float_defs(types), sizes: def_sizes(types) }
    }

    /// Returns the rough in-memory size in bytes of `ty` on a 64-bit target. It decides which enum variants to box
    /// (`clippy::large_enum_variant`).
    pub(crate) fn approx_size(&self, ty: &Ty) -> usize {
        size_in(ty, &mut |id| self.sizes.get(id.0).copied().unwrap_or(WORD))
    }

    /// Does `ty` hold a float anywhere (and so cannot derive `Eq`)? Your own type (`x-rust-type`) counts as one:
    /// it may not implement `Eq`.
    pub(crate) fn has_float(&self, ty: &Ty) -> bool {
        let mut refs = vec![];
        all_refs(ty, &mut refs);
        float_here(ty) || refs.iter().any(|id| self.floats.get(id.0).copied().unwrap_or_default())
    }

    /// Returns the type that `t` names once aliases are followed. That covers a twin declared as an alias of its first
    /// (`aliases`) and a `Kind::Alias`.
    pub(crate) fn resolved(&self, aliases: &BTreeMap<usize, usize>, t: &Ty) -> Ty {
        let Ty::Named(id) = t else { return t.clone() };
        match (aliases.get(&id.0), &self.types.get(*id).kind) {
            (Some(&first), _) => self.resolved(aliases, &Ty::Named(TypeId(first))),
            (None, Kind::Alias(x)) => self.resolved(aliases, x),
            (None, _) => t.clone(),
        }
    }
}

/// Returns [`Space::has_float`] for every definition. First those that hold one themselves and then whatever refers to
/// them. Each definition is visited once. Asking each definition alone walks all that it reaches. That is O(n^2) on
/// long `$ref` chains.
fn float_defs(space: &TypeSpace) -> Vec<bool> {
    let mut floats = vec![false; space.defs.len()];
    let mut referrers: Vec<Vec<usize>> = vec![vec![]; space.defs.len()];
    let mut pending = vec![];
    for (i, d) in space.defs.iter().enumerate() {
        let own = def_types(&d.kind);
        if d.rust_type.is_some() || own.iter().any(|t| float_here(t)) {
            floats[i] = true;
            pending.push(i);
        }
        let mut refs = vec![];
        for t in own {
            all_refs(t, &mut refs);
        }
        for r in refs {
            if let Some(list) = referrers.get_mut(r.0) {
                list.push(i);
            }
        }
    }
    while let Some(i) = pending.pop() {
        for &r in referrers.get(i).into_iter().flatten() {
            if !floats[r] {
                floats[r] = true;
                pending.push(r);
            }
        }
    }
    floats
}

/// Returns whether `ty` holds a float itself. Named types that it refers to do not count.
fn float_here(ty: &Ty) -> bool {
    matches!(ty, Ty::F32 | Ty::F64) || ty.children().iter().any(float_here)
}

/// The size of a pointer, and of a variant that is boxed.
pub(crate) const WORD: usize = 8;
const COLLECTION: usize = 24;
const JSON_VALUE: usize = 32;

/// Where finding the size of a definition is.
#[derive(Clone, Copy)]
enum Size {
    Unknown,
    /// Met again while it is found, the definition is on a cycle that a `Box` breaks.
    Finding,
    Found(usize),
}

/// Returns [`Space::approx_size`] of every definition of `types`, each found once. A union of unions asks for the size
/// of each type again on every path to it: found afresh each time, that grows with the paths and not with the types.
fn def_sizes(types: &TypeSpace) -> Vec<usize> {
    fn named(types: &TypeSpace, found: &mut [Size], i: usize) -> usize {
        match found[i] {
            Size::Found(size) => return size,
            Size::Finding => return WORD,
            Size::Unknown => {}
        }
        found[i] = Size::Finding;
        let size = def_size(&types.defs[i].kind, &mut |id| named(types, found, id.0));
        found[i] = Size::Found(size);
        size
    }
    let mut found = vec![Size::Unknown; types.defs.len()];
    (0..types.defs.len()).map(|i| named(types, &mut found, i)).collect()
}

/// Returns the size of a definition of kind `kind`, with the sizes of the definitions it names from `named`.
fn def_size(kind: &Kind, named: &mut dyn FnMut(TypeId) -> usize) -> usize {
    match kind {
        Kind::Struct(s) => {
            s.fields
                .iter()
                .map(|f| if f.required { size_in(&f.ty, named) } else { option_size(&f.ty, named) })
                .sum::<usize>()
                + if s.additional.is_some() { COLLECTION } else { 0 }
        }
        Kind::StrEnum(_) | Kind::IntEnum(_) => COLLECTION,
        // A checked type holds its base.
        Kind::Checked(c) => size_in(&c.base, named),
        Kind::Const(_) => 0,
        Kind::Union(u) => u.variants.iter().map(|v| size_in(&v.ty, named)).max().unwrap_or(0).max(JSON_VALUE) + WORD,
        Kind::Alias(t) | Kind::Newtype(t) => size_in(t, named),
        Kind::Pending => WORD,
    }
}

fn size_in(ty: &Ty, named: &mut dyn FnMut(TypeId) -> usize) -> usize {
    match ty {
        Ty::Unit => 0,
        Ty::Bool | Ty::I8 | Ty::U8 => size_of::<u8>(),
        Ty::I16 | Ty::U16 => size_of::<u16>(),
        Ty::I32 | Ty::U32 | Ty::F32 => size_of::<u32>(),
        Ty::I64 | Ty::U64 | Ty::F64 => size_of::<u64>(),
        Ty::Indirect(_) => WORD,
        Ty::String | Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Vec(_) | Ty::Map(_) | Ty::Decimal => COLLECTION,
        Ty::Any => JSON_VALUE,
        Ty::Option(t) => option_size(t, named),
        Ty::Tuple(ts) => ts.iter().map(|t| size_in(t, named)).sum(),
        Ty::Array(t, n) => n * size_in(t, named),
        Ty::Named(id) => named(*id),
    }
}

fn option_size(t: &Ty, named: &mut dyn FnMut(TypeId) -> usize) -> usize {
    let inner = size_in(t, named);
    if matches!(
        t,
        Ty::String | Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Vec(_) | Ty::Map(_) | Ty::Indirect(_) | Ty::Named(_)
    ) {
        inner
    } else {
        inner + WORD
    }
}
