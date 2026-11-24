//! The security requirements that each operation declares as the client and server refer to them.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::emit::rust::names::pascal;
use crate::emit::rust::syntax::{doc_opt, ident, lit, pascal_ident};
use crate::ir::Api;
use crate::ir::http::Operation;
use crate::ir::http::declared as requirements;
use crate::ir::names::Name;
use crate::ir::security::{SchemeKind, SecurityScheme as Scheme};

/// Each distinct list of security requirements that the operations declare. Each is declared once in
/// `operations.rs` as `SECURITY_<n>` in order of first use. An operation that needs none refers to `&[]`.
pub(crate) struct Securities(BTreeMap<String, (usize, TokenStream)>);

impl Securities {
    pub(crate) fn new(api: &Api) -> Self {
        let mut sets = BTreeMap::new();
        for op in api.http.operations.iter().filter(|o| !o.security.is_empty()) {
            let alts = security_alts(api, op);
            let n = sets.len();
            sets.entry(alts.to_string()).or_insert((n, alts));
        }
        Self(sets)
    }

    /// Returns the requirements of the operation as a `&[&[runtime::Requirement]]` expression.
    pub(crate) fn of(&self, api: &Api, op: &Operation) -> TokenStream {
        if op.security.is_empty() {
            return quote!(&[]);
        }
        let n = self.0[&security_alts(api, op).to_string()].0;
        let name = format_ident!("SECURITY_{n}");
        quote!(ops::#name)
    }

    /// Returns the constants. In a binary, `operations.rs` allows the ones that no operation it calls names.
    pub(crate) fn consts(&self) -> TokenStream {
        let mut sets: Vec<&(usize, TokenStream)> = self.0.values().collect();
        sets.sort_by_key(|(n, _)| *n);
        let consts = sets.iter().map(|(n, alts)| {
            let name = format_ident!("SECURITY_{n}");
            quote!(#[doc(hidden)] pub const #name: &[&[runtime::Requirement]] = #alts;)
        });
        if sets.is_empty() {
            return TokenStream::new();
        }
        quote!(use super::runtime; #(#consts)*)
    }
}

/// Returns the security requirements of the operation as a `&[&[runtime::Requirement]]` expression.
fn security_alts(api: &Api, op: &Operation) -> TokenStream {
    let alts = op.security.iter().map(|alt| {
        let schemes = alt
            .iter()
            .filter_map(|req| api.schemes.iter().find(|s| s.name == req.scheme).map(|s| (s, &req.scopes)))
            .map(|(s, scopes)| {
                let n = lit(&s.name);
                let scheme = match &s.kind {
                    SchemeKind::ApiKey { location, param } => {
                        let loc = match location.as_str() {
                            "query" => quote!(runtime::KeyIn::Query),
                            "cookie" => quote!(runtime::KeyIn::Cookie),
                            _ => quote!(runtime::KeyIn::Header),
                        };
                        let param = lit(param);
                        quote!(runtime::Scheme::ApiKey(#n, #loc, #param))
                    }
                    SchemeKind::Basic => quote!(runtime::Scheme::Basic(#n)),
                    SchemeKind::Bearer => quote!(runtime::Scheme::Bearer(#n)),
                    SchemeKind::OtherHttp(h) => {
                        let h = lit(h);
                        quote!(runtime::Scheme::Http(#n, #h))
                    }
                    SchemeKind::MutualTls => quote!(runtime::Scheme::MutualTls(#n)),
                };
                let scopes = scopes.iter().map(|x| lit(x));
                quote!((#scheme, &[#(#scopes),*]))
            });
        quote!(&[#(#schemes),*])
    });
    quote!(&[#(#alts),*])
}

fn scheme_scopes(api: &Api, scheme: &Scheme) -> Vec<(String, Option<String>)> {
    let mut out: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (name, d) in &scheme.scopes {
        out.entry(name.clone()).or_insert_with(|| (!d.trim().is_empty()).then(|| d.clone()));
    }
    for req in requirements(api.http.operations.iter()).filter(|r| r.scheme == scheme.name) {
        for name in &req.scopes {
            out.entry(name.clone()).or_insert(None);
        }
    }
    out.into_iter().collect()
}

/// Returns the scope enum of `scheme` (`PatScope`), or `None` when it has no scopes or roles.
pub(crate) fn scope_enum(api: &Api, scheme: &Scheme) -> Option<proc_macro2::Ident> {
    (!scheme_scopes(api, scheme).is_empty()).then(|| ident(&format!("{}Scope", pascal(&Name::new(&scheme.name)))))
}

/// Returns one scope enum per scheme with scopes or roles: one variant per scope with `as_str` and `from_name`. A
/// validator returns the scopes of a credential in it, so it cannot grant a scope of another scheme or one that the
/// description does not list.
pub(crate) fn scope_enums(api: &Api) -> TokenStream {
    let enums = api.schemes.iter().filter_map(|scheme| {
        let names = scheme_scopes(api, scheme);
        let name = scope_enum(api, scheme)?;
        let mut taken = BTreeSet::new();
        let variants: Vec<proc_macro2::Ident> =
            names.iter().map(|(n, _)| pascal_ident(&crate::ir::names::unique(Name::new(n), &mut taken))).collect();
        let decls = names.iter().zip(&variants).map(|((_, d), v)| {
            let doc = doc_opt(d.as_deref());
            quote!(#doc #v,)
        });
        let to_str = names.iter().zip(&variants).map(|((n, _), v)| {
            let n = lit(n);
            quote!(Self::#v => #n,)
        });
        let by_name = names.iter().zip(&variants).map(|((n, _), v)| {
            let n = lit(n);
            quote!(#n => Some(Self::#v),)
        });
        let doc = lit(&format!(" A scope or role of `{}`.", scheme.name));
        Some(quote! {
            #[doc = #doc]
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
            pub enum #name {
                #(#decls)*
            }

            impl #name {
                #[must_use]
                pub const fn as_str(self) -> &'static str {
                    match self {
                        #(#to_str)*
                    }
                }

                #[must_use]
                pub fn from_name(name: &str) -> Option<Self> {
                    match name {
                        #(#by_name)*
                        _ => None,
                    }
                }
            }
        })
    });
    quote!(#(#enums)*)
}
