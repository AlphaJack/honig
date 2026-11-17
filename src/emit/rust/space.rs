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
        Self { types, libs, floats: float_defs(types) }
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
