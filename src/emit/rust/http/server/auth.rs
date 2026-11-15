//! The scope and credential enums a server authorizes with.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::quote;

use crate::emit::rust::names::pascal;
use crate::emit::rust::syntax::pascal_ident;
use crate::emit::rust::syntax::{doc_opt, ident, lit};
use crate::ir::Api;
use crate::ir::names::Name;
use crate::ir::security::SchemeKind;

/// Returns every scope or role that the description names in name order. That is `OAuth2` scopes declared on a
/// scheme (with their description) and the roles that security requirements list
/// (3.1+: any scheme type).
pub(crate) fn scope_names(api: &Api) -> Vec<(String, Option<String>)> {
    let mut out: BTreeMap<String, Option<String>> = BTreeMap::new();
    for s in &api.schemes {
        for (name, d) in &s.scopes {
            out.entry(name.clone()).or_insert_with(|| (!d.trim().is_empty()).then(|| d.clone()));
        }
    }
    for req in api.http.operations.iter().flat_map(|o| o.security.iter().flatten()) {
        for name in &req.scopes {
            out.entry(name.clone()).or_insert(None);
        }
    }
    out.into_iter().collect()
}

/// Returns `Scope`: one variant per scope or role with `scope_alternatives` and
/// `scopes_held`. So an authentication layer can match exhaustively. A scope added to the
/// description is a compile error there. A misspelt one cannot be written.
pub(crate) fn scope_enum(api: &Api) -> Option<TokenStream> {
    let names = scope_names(api);
    if names.is_empty() {
        return None;
    }
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
    Some(quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Scope {
            #(#decls)*
        }

        impl Scope {
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

        #[must_use]
        pub fn scope_alternatives(request: &impl runtime::Secured) -> Vec<Vec<Scope>> {
            request.requirements().iter().map(|alt| alt.iter().filter_map(|s| Scope::from_name(s)).collect()).collect()
        }

        pub fn scopes_held(request: &impl runtime::Secured, held: impl Fn(Scope) -> bool) -> bool {
            runtime::Secured::scopes_held(request, |s| Scope::from_name(s).is_some_and(&held))
        }
    })
}

/// Returns the unique variant name of each security scheme in `Credential`.
pub(crate) fn credential_variants(api: &Api) -> Vec<String> {
    let mut taken = BTreeSet::new();
    api.schemes.iter().map(|s| pascal(&crate::ir::names::unique(Name::new(&s.name), &mut taken))).collect()
}

/// Returns `Credential`: one variant per security scheme. So an authentication layer can match
/// exhaustively. A new scheme in the description is a compile error there.
pub(crate) fn credential_enum(api: &Api) -> Option<TokenStream> {
    if api.schemes.is_empty() {
        return None;
    }
    let names = credential_variants(api);
    let variants = api.schemes.iter().zip(&names).map(|(s, v)| {
        let v = ident(v);
        let doc = doc_opt(s.doc.as_deref());
        match s.kind {
            SchemeKind::Basic => quote! {
                #doc
                #v {
                    user: &'a str,
                    password: &'a str,
                },
            },
            SchemeKind::MutualTls => quote!(#doc #v,),
            SchemeKind::ApiKey { .. } | SchemeKind::Bearer | SchemeKind::OtherHttp(_) => quote!(#doc #v(&'a str),),
        }
    });
    let arms = api.schemes.iter().zip(&names).map(|(s, v)| {
        let v = ident(v);
        let name = lit(&s.name);
        match s.kind {
            SchemeKind::Basic => quote! {
                #name => {
                    let (user, password) = value.split_once(':').unwrap_or((value.as_str(), ""));
                    Some(Credential::#v { user, password })
                }
            },
            SchemeKind::MutualTls => quote!(#name => Some(Credential::#v),),
            SchemeKind::ApiKey { .. } | SchemeKind::Bearer | SchemeKind::OtherHttp(_) => {
                quote!(#name => Some(Credential::#v(value.as_str())),)
            }
        }
    });
    Some(quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Credential<'a> {
            #(#variants)*
        }

        #[must_use]
        pub fn credentials(request: &impl runtime::Secured) -> Vec<Credential<'_>> {
            request
                .credentials()
                .iter()
                .filter_map(|(scheme, value)| match scheme.name() {
                    #(#arms)*
                    _ => None,
                })
                .collect()
        }
    })
}
