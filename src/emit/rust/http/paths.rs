//! Path templates: their pieces, segments and specificity and the path parameters that they name.

use proc_macro2::TokenStream;
use quote::quote;

use crate::emit::rust::syntax::snake_ident;
use crate::ir::http::{Location, Operation, Param};
use crate::ir::media::MediaKind;

/// Sorts key per path segment. It is 0 when fully literal, 1 when it mixes text and
/// variables and 2 when it is one variable. Lower sorts first. So
/// `/products/search` beats `/products/{id}` and `/media/{id}.{ext}` beats `/media/{id}`.
/// Segments as the router sees them. An undeclared `{name}` is literal.
pub(crate) fn specificity(op: &Operation) -> Vec<u8> {
    template_segs(op)
        .iter()
        .map(|seg| match seg {
            Seg::Lit(_) => 0,
            Seg::Mixed(_) => 1,
            Seg::Var => 2,
        })
        .collect()
}

/// Splits a template into pieces: literal text and `{var}` names.
pub(crate) fn template(path: &str) -> Vec<Result<String, String>> {
    let mut out = vec![];
    let mut rest = path;
    while let Some((before, after)) = rest.split_once('{') {
        if !before.is_empty() {
            out.push(Ok(before.to_owned()));
        }
        if let Some((var, tail)) = after.split_once('}') {
            out.push(Err(var.to_owned()));
            rest = tail;
        } else {
            out.push(Ok(format!("{{{after}")));
            rest = "";
        }
    }
    if !rest.is_empty() {
        out.push(Ok(rest.to_owned()));
    }
    out
}

pub(crate) fn path_param<'a>(op: &'a Operation, name: &str) -> Option<&'a Param> {
    op.params.iter().find(|p| p.location == Location::Path && p.wire == name)
}

pub(crate) fn value_expr(p: &Param) -> TokenStream {
    let n = snake_ident(&p.name);
    if p.location == Location::QueryString {
        quote!(runtime::to_value(&p.#n)?)
    } else if p.content == Some(MediaKind::Json) {
        quote!(runtime::json_param(&p.#n)?)
    } else {
        // Encoders take `Serialize` directly: scalars skip `serde_json::Value`.
        quote!(p.#n)
    }
}

/// One segment of a path template as the router sees it. A `{name}` that
/// is not a declared path parameter is literal text.
pub(crate) enum Seg {
    Lit(String),
    Var,
    /// Text and variables in one segment (`{name}.{ext}`): `Err` is a variable.
    Mixed(Vec<Result<String, ()>>),
}

pub(crate) fn template_segs(op: &Operation) -> Vec<Seg> {
    let path = op.path.strip_prefix('/').unwrap_or(&op.path);
    path.split('/')
        .map(|seg| {
            let pieces: Vec<Result<String, ()>> = template(seg)
                .into_iter()
                .map(|p| match p {
                    Ok(l) => Ok(l),
                    Err(name) if path_param(op, &name).is_some() => Err(()),
                    Err(name) => Ok(format!("{{{name}}}")),
                })
                .collect();
            match pieces.as_slice() {
                [Err(())] => Seg::Var,
                ps if ps.iter().all(Result::is_ok) => Seg::Lit(pieces.into_iter().flatten().collect()),
                _ => Seg::Mixed(pieces),
            }
        })
        .collect()
}
