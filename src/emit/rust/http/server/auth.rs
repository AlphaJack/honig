//! The scope and credential enums and the credential checks of a server.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::quote;

use crate::emit::rust::names::pascal;
use crate::emit::rust::syntax::pascal_ident;
use crate::emit::rust::syntax::{doc_opt, ident, lit};
use crate::ir::Api;
use crate::ir::http::Operation;
use crate::ir::names::Name;
use crate::ir::security::{Requirement, SchemeKind, SecurityScheme as Scheme};

/// Returns every requirement that `ops` declare.
fn requirements<'a>(ops: impl Iterator<Item = &'a Operation>) -> impl Iterator<Item = &'a Requirement> {
    ops.flat_map(|o| o.security.iter().flatten())
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

/// Returns the name of the scope enum of the scheme whose `Credential` variant is `variant`: `PatScope`.
fn scope_enum_name(variant: &str) -> proc_macro2::Ident {
    ident(&format!("{variant}Scope"))
}

/// Returns one scope enum per scheme with scopes or roles: one variant per scope with `as_str` and `from_name`. A
/// validator returns the scopes of a credential in it, so it cannot grant a scope of another scheme or one that the
/// description does not list.
pub(crate) fn scope_enums(api: &Api) -> TokenStream {
    let enums = api.schemes.iter().zip(credential_variants(api)).filter_map(|(scheme, variant)| {
        let names = scheme_scopes(api, scheme);
        if names.is_empty() {
            return None;
        }
        let name = scope_enum_name(&variant);
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

/// Returns the schemes that the requirements of `ops` list, each once in the order of the description, with their
/// `Credential` variant.
fn used_schemes<'a>(api: &'a Api, ops: &[&Operation]) -> Vec<(&'a Scheme, String)> {
    let used: BTreeSet<&str> = requirements(ops.iter().copied()).map(|r| r.scheme.as_str()).collect();
    api.schemes.iter().zip(credential_variants(api)).filter(|(s, _)| used.contains(s.name.as_str())).collect()
}

/// Returns the name of the method that checks a credential of the scheme whose `Credential` variant is `variant`:
/// `verify_pat`.
fn verify_name(variant: &str) -> proc_macro2::Ident {
    ident(&format!("verify_{}", crate::emit::rust::names::snake(&Name::new(variant))))
}

/// Returns one method per scheme that `ops` use: it checks the credential and returns who is calling, or `None` for a
/// credential that is not valid. A scheme with scopes or roles returns the scopes that the credential grants too.
/// With `allow_unimplemented` each has a default that refuses every credential.
pub(crate) fn verify_methods(api: &Api, ops: &[&Operation], allow_unimplemented: bool) -> TokenStream {
    let methods = used_schemes(api, ops).into_iter().map(|(scheme, variant)| {
        let name = verify_name(&variant);
        let found = if scheme_scopes(api, scheme).is_empty() {
            quote!(Self::Principal)
        } else {
            let scope = scope_enum_name(&variant);
            quote!(runtime::Granted<Self::Principal, #scope>)
        };
        let (arg, unused) = match scheme.kind {
            SchemeKind::Basic => (quote!(login: runtime::Basic<'_>,), quote!(login)),
            SchemeKind::MutualTls => (quote!(), quote!(())),
            SchemeKind::ApiKey { .. } | SchemeKind::Bearer | SchemeKind::OtherHttp(_) => {
                (quote!(credential: &str,), quote!(credential))
            }
        };
        let doc = lit(&format!(
            " Checks a credential of `{}`: who is calling, or `None` when it is not valid (401).",
            scheme.name
        ));
        let body = if allow_unimplemented {
            quote!({
                let _ = (#unused, request);
                std::future::ready(Ok(None))
            })
        } else {
            quote!(;)
        };
        quote! {
            #[doc = #doc]
            fn #name(&self, #arg request: &runtime::Request)
                -> impl Future<Output = Result<Option<#found>, runtime::ApiError>> + Send #body
        }
    });
    quote!(#(#methods)*)
}

/// Returns `runtime::Operations::verify` of the server of `ops`: the `verify_` method of the scheme of the credential,
/// on the `Api` that `api` reaches (`&*self.0`).
pub(crate) fn verify_fn(api: &Api, ops: &[&Operation], reach: &TokenStream, principal: &TokenStream) -> TokenStream {
    let signature = |names: TokenStream| {
        quote! {
            fn verify<'r>(&'r self, #names) -> runtime::BoxFuture<'r, Result<Option<runtime::Verified<#principal>>, runtime::ApiError>>
        }
    };
    let used = used_schemes(api, ops);
    if used.is_empty() {
        let signature = signature(quote!(_: &'static str, _: &'r str, _: &'r runtime::Request));
        return quote!(#signature { Box::pin(std::future::ready(Ok(None))) });
    }
    let credential =
        if used.iter().all(|(s, _)| matches!(s.kind, SchemeKind::MutualTls)) { quote!(_) } else { quote!(credential) };
    let signature = signature(quote!(scheme: &'static str, #credential: &'r str, request: &'r runtime::Request));
    let arms = used.into_iter().map(|(scheme, variant)| {
        let (key, name) = (lit(&scheme.name), verify_name(&variant));
        let call = match scheme.kind {
            SchemeKind::Basic => quote!(api.#name(runtime::Basic::of(credential), request).await?),
            SchemeKind::MutualTls => quote!(api.#name(request).await?),
            SchemeKind::ApiKey { .. } | SchemeKind::Bearer | SchemeKind::OtherHttp(_) => {
                quote!(api.#name(credential, request).await?)
            }
        };
        let verified = if scheme_scopes(api, scheme).is_empty() {
            quote!(#call.map(runtime::Verified::new))
        } else {
            let scope = scope_enum_name(&variant);
            quote!(#call.map(|g| runtime::Verified {
                principal: g.principal,
                scopes: g.scopes.into_iter().map(#scope::as_str).collect(),
            }))
        };
        quote!(#key => Ok(#verified),)
    });
    quote! {
        #signature {
            let api = #reach;
            Box::pin(async move {
                match scheme {
                    #(#arms)*
                    _ => Ok(None),
                }
            })
        }
    }
}

/// Returns the variant name of each security scheme in `Credential`. Names are unique: a collision fails generation.
pub(crate) fn credential_variants(api: &Api) -> Vec<String> {
    api.schemes.iter().map(|s| pascal(&Name::new(&s.name))).collect()
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
