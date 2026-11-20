//! Breaking reference cycles: a type that contains itself by value is boxed where the cycle closes.

use super::{Access, Kind, Ty, TypeId, TypeSpace};

/// Collects the direct (by-value) references from a type expression. `Vec` and `Map`
/// already allocate. So they do not count.
pub(crate) fn direct_refs(ty: &Ty, out: &mut Vec<TypeId>) {
    if let Ty::Named(id) = ty {
        out.push(*id);
    }
    if !ty.allocates() {
        ty.children().iter().for_each(|t| direct_refs(t, out));
    }
}

/// Keeps the fields of `kind` that travel in `keep`'s direction (with the
/// both-ways ones); those of that direction are required as the schema says.
pub(crate) fn direction(kind: Kind, keep: Access) -> Kind {
    match kind {
        Kind::Struct(mut s) => {
            s.fields.retain(|f| f.access == Access::ReadWrite || f.access == keep);
            for f in s.fields.iter_mut().filter(|f| f.access == keep) {
                f.required = f.spec_required || f.default.is_some();
            }
            Kind::Struct(s)
        }
        other @ (Kind::StrEnum(_)
        | Kind::IntEnum(_)
        | Kind::Union(_)
        | Kind::Alias(_)
        | Kind::Newtype(_)
        | Kind::Const(_)
        | Kind::Checked(_)
        | Kind::Pending) => other,
    }
}

/// Visits every type expression of a definition as mutable.
pub(crate) fn map_kind(kind: &mut Kind, f: &dyn Fn(&mut Ty)) {
    match kind {
        Kind::Struct(s) => {
            s.fields.iter_mut().for_each(|x| f(&mut x.ty));
            if let Some(a) = &mut s.additional {
                f(a);
            }
            s.patterns.iter_mut().for_each(|(_, t, _)| f(t));
        }
        Kind::Union(u) => u.variants.iter_mut().for_each(|v| f(&mut v.ty)),
        Kind::Alias(t) | Kind::Newtype(t) => f(t),
        Kind::Checked(c) => f(&mut c.base),
        Kind::StrEnum(_) | Kind::IntEnum(_) | Kind::Const(_) | Kind::Pending => {}
    }
}

/// Points named types at their request type where they have one.
pub(crate) fn map_named(ty: &mut Ty, request_of: &[Option<TypeId>]) {
    if let Ty::Named(id) = ty
        && let Some(Some(r)) = request_of.get(id.0)
    {
        *id = *r;
    }
    ty.children_mut().iter_mut().for_each(|t| map_named(t, request_of));
}

pub(crate) fn all_refs(ty: &Ty, out: &mut Vec<TypeId>) {
    if let Ty::Named(id) = ty {
        out.push(*id);
    }
    ty.children().iter().for_each(|t| all_refs(t, out));
}

pub(crate) fn def_types(kind: &Kind) -> Vec<&Ty> {
    match kind {
        Kind::Struct(s) => s
            .fields
            .iter()
            .map(|f| &f.ty)
            .chain(s.additional.iter())
            .chain(s.patterns.iter().map(|(_, t, _)| t))
            .collect(),
        Kind::Union(u) => u.variants.iter().map(|v| &v.ty).collect(),
        Kind::Alias(t) | Kind::Newtype(t) => vec![t],
        Kind::Checked(c) => vec![&c.base],
        Kind::StrEnum(_) | Kind::IntEnum(_) | Kind::Const(_) | Kind::Pending => vec![],
    }
}

/// Runs Tarjan SCC over `edges` and returns the component index per node.
pub(crate) fn scc(n: usize, edges: &[Vec<usize>]) -> Vec<usize> {
    struct S<'a> {
        edges: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        comp: Vec<usize>,
        next: usize,
        ncomp: usize,
    }
    fn visit(s: &mut S<'_>, v: usize) {
        // Iterative to survive deep graphs.
        let mut work = vec![(v, 0usize)];
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on[v] = true;
        while let Some(&mut (u, ref mut i)) = work.last_mut() {
            if *i < s.edges[u].len() {
                let w = s.edges[u][*i];
                *i += 1;
                if s.index[w].is_none() {
                    s.index[w] = Some(s.next);
                    s.low[w] = s.next;
                    s.next += 1;
                    s.stack.push(w);
                    s.on[w] = true;
                    work.push((w, 0));
                } else if s.on[w] {
                    s.low[u] = s.low[u].min(s.index[w].unwrap_or(s.low[u]));
                }
            } else {
                work.pop();
                if let Some(&(p, _)) = work.last() {
                    s.low[p] = s.low[p].min(s.low[u]);
                }
                if Some(s.low[u]) == s.index[u] {
                    while let Some(w) = s.stack.pop() {
                        s.on[w] = false;
                        s.comp[w] = s.ncomp;
                        if w == u {
                            break;
                        }
                    }
                    s.ncomp += 1;
                }
            }
        }
    }
    let mut s = S {
        edges,
        index: vec![None; n],
        low: vec![0; n],
        on: vec![false; n],
        stack: vec![],
        comp: vec![0; n],
        next: 0,
        ncomp: 0,
    };
    for v in 0..n {
        if s.index[v].is_none() {
            visit(&mut s, v);
        }
    }
    s.comp
}

pub(crate) fn break_cycles(space: &mut TypeSpace) {
    let n = space.defs.len();
    // Aliases that are part of any cycle cannot be `type` aliases.
    let full: Vec<Vec<usize>> = space
        .defs
        .iter()
        .map(|d| {
            let mut out = vec![];
            for t in def_types(&d.kind) {
                all_refs(t, &mut out);
            }
            out.into_iter().map(|t| t.0).collect()
        })
        .collect();
    let comp = scc(n, &full);
    let mut size = vec![0usize; n];
    for c in &comp {
        size[*c] += 1;
    }
    for i in 0..n {
        let in_cycle = size[comp[i]] > 1 || full[i].contains(&i);
        if in_cycle && let Kind::Alias(t) = &space.defs[i].kind {
            space.defs[i].kind = Kind::Newtype(t.clone());
        }
    }
    // Box every by-value edge inside a strongly connected component.
    let direct: Vec<Vec<usize>> = space
        .defs
        .iter()
        .map(|d| {
            let mut out = vec![];
            for t in def_types(&d.kind) {
                direct_refs(t, &mut out);
            }
            out.into_iter().map(|t| t.0).collect()
        })
        .collect();
    let comp = scc(n, &direct);
    for i in 0..n {
        let c = comp[i];
        let box_if = |ty: &mut Ty| box_edges(ty, &|t: TypeId| comp[t.0] == c);
        match &mut space.defs[i].kind {
            Kind::Struct(s) => s.fields.iter_mut().for_each(|f| box_if(&mut f.ty)),
            Kind::Union(u) => u.variants.iter_mut().for_each(|v| box_if(&mut v.ty)),
            Kind::Newtype(t) => box_if(t),
            Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Alias(_)
            | Kind::Const(_)
            | Kind::Checked(_)
            | Kind::Pending => {}
        }
    }
}

pub(crate) fn box_edges(ty: &mut Ty, same: &dyn Fn(TypeId) -> bool) {
    if let Ty::Named(id) = *ty
        && same(id)
    {
        *ty = Ty::Indirect(Box::new(Ty::Named(id)));
    } else if !ty.allocates() {
        ty.children_mut().iter_mut().for_each(|t| box_edges(t, same));
    }
}
