//! The scope and credential enums and the credential checks of a server.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::quote;

use crate::emit::rust::http::security::scope_enum;
use crate::emit::rust::names::pascal;
use crate::emit::rust::syntax::{doc_opt, ident, lit};
use crate::ir::Api;
use crate::ir::http::Operation;
use crate::ir::http::declared as requirements;
use crate::ir::names::Name;
use crate::ir::security::{SchemeKind, SecurityScheme as Scheme};

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
        let own = ident(&variant);
        let found = match scope_enum(api, scheme) {
            None => quote!(Self::#own),
            Some(scope) => quote!(runtime::Granted<Self::#own, #scope>),
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

/// Returns what the server of `ops` carries from the check of a credential to the operation: `values<A>`
/// (`SchemeValue<A>`), or `()` when `ops` use no scheme.
pub(crate) fn value_type(api: &Api, ops: &[&Operation], values: &str) -> TokenStream {
    let values = ident(values);
    if used_schemes(api, ops).is_empty() { quote!(()) } else { quote!(#values<A>) }
}

/// Returns one associated type per scheme that `ops` use: what its `verify_` method finds.
pub(crate) fn scheme_types(api: &Api, ops: &[&Operation]) -> TokenStream {
    let types = used_schemes(api, ops).into_iter().map(|(scheme, variant)| {
        let (name, doc) =
            (ident(&variant), lit(&format!(" What `verify_` finds for a credential of `{}`.", scheme.name)));
        quote!(#[doc = #doc] type #name: Clone + Send + Sync + 'static;)
    });
    quote!(#(#types)*)
}

/// Returns `name<A: tr>`: one variant per scheme that `ops` use with what its `verify_` method found, and a method
/// per scheme that returns its value. The server carries it from the check of a credential to the operation, which
/// gets its own `Security` of it. `None` when `ops` use no scheme.
pub(crate) fn scheme_value_enum(api: &Api, ops: &[&Operation], name: &str, tr: &str) -> Option<TokenStream> {
    let used = used_schemes(api, ops);
    if used.is_empty() {
        return None;
    }
    let (name, tr) = (ident(name), ident(tr));
    let several = used.len() > 1;
    let variants: Vec<_> = used.iter().map(|(_, v)| ident(v)).collect();
    let getters = used.iter().map(|(_, v)| {
        let (variant, getter) = (ident(v), crate::emit::rust::syntax::snake_ident(&Name::new(v)));
        let others: Vec<_> = variants.iter().filter(|o| **o != variant).collect();
        let other = several.then(|| quote!(#(Self::#others(_))|* => None,));
        quote! {
            #[must_use]
            pub fn #getter(self) -> Option<A::#variant> {
                match self { Self::#variant(v) => Some(v), #other }
            }
        }
    });
    Some(quote! {
        /// What the `verify_` method of a scheme found. The server carries it to the operation's `Security`. A layer
        /// finds the alternative that passed as `runtime::Passed` of it in the request's extensions.
        pub enum #name<A: #tr> {
            #(#variants(A::#variants),)*
        }

        impl<A: #tr> Clone for #name<A> {
            fn clone(&self) -> Self {
                match self { #(Self::#variants(v) => Self::#variants(v.clone()),)* }
            }
        }

        impl<A: #tr> #name<A> {
            #(#getters)*
        }
    })
}

/// Returns the variant of the `Security` of `op` for each alternative of its `security`: the names of its schemes
/// joined by `And`, `Anonymous` for an empty one. Two alternatives of the same schemes share one.
fn security_variants(api: &Api, op: &Operation) -> Vec<(String, Vec<String>)> {
    let names: Vec<(&str, String)> =
        api.schemes.iter().map(|s| s.name.as_str()).zip(credential_variants(api)).collect();
    op.security
        .iter()
        .map(|alt| {
            let schemes: Vec<String> =
                alt.iter().filter_map(|r| names.iter().find(|(n, _)| *n == r.scheme).map(|(_, v)| v.clone())).collect();
            let variant = if schemes.is_empty() { "Anonymous".to_owned() } else { schemes.join("And") };
            (variant, schemes)
        })
        .collect()
}

/// Returns the trait that `op` is a method of: `Api` for a path operation, `Webhooks` for a webhook or callback.
fn trait_of(op: &Operation) -> proc_macro2::Ident {
    ident(if crate::emit::rust::http::shared::is_path(op) { "Api" } else { "Webhooks" })
}

/// Returns the `Security` of `op`: one variant per alternative of its `security` with what the `verify_` method of
/// each of its schemes found. `None` for an operation without `security`.
pub(crate) fn security_enum(api: &Api, op: &Operation) -> Option<TokenStream> {
    if op.security.is_empty() {
        return None;
    }
    let tr = trait_of(op);
    let mut seen: Vec<String> = Vec::new();
    let variants = security_variants(api, op).into_iter().filter_map(|(variant, schemes)| {
        if seen.contains(&variant) {
            return None;
        }
        seen.push(variant.clone());
        let v = ident(&variant);
        if schemes.is_empty() {
            return Some(quote!(/// No credentials: the `security` of the operation allows an anonymous request.
                #v,));
        }
        let fields = schemes.iter().map(|s| ident(s));
        let doc =
            lit(&format!(" The alternative of `{}`, with what each `verify_` method found.", schemes.join("` and `")));
        Some(quote!(#[doc = #doc] #v(#(A::#fields),*),))
    });
    Some(quote! {
        /// Who is calling: the alternative of the operation's `security` that the request passed.
        pub enum Security<A: super::super::server::#tr> {
            #(#variants)*
        }
    })
}

/// Returns the type of the `Security` of `op` as a method of its trait refers to it: `()` for an operation without
/// `security`.
pub(crate) fn security_type(op: &Operation, m: &TokenStream) -> TokenStream {
    if op.security.is_empty() { quote!(()) } else { quote!(#m::Security<Self>) }
}

/// Returns the conversion of what the server verified for `op` to its `Security`: a closure from
/// `Option<runtime::Passed<value>>` to `Option<Security>` that `runtime::Call::map` applies. `None` is 401.
pub(crate) fn security_of(api: &Api, op: &Operation, m: &TokenStream) -> TokenStream {
    let variants = security_variants(api, op);
    let anonymous = variants.iter().any(|(_, s)| s.is_empty());
    // Alternatives of the same schemes (with other scopes) make the same variant: one arm.
    let mut arms: Vec<(Vec<usize>, &String, &Vec<String>)> = Vec::new();
    for (i, (variant, schemes)) in variants.iter().enumerate().filter(|(_, (_, s))| !s.is_empty()) {
        match arms.iter_mut().find(|(_, v, _)| *v == variant) {
            Some((indices, ..)) => indices.push(i),
            None => arms.push((vec![i], variant, schemes)),
        }
    }
    let arms = arms.into_iter().map(|(indices, variant, schemes)| {
        let v = ident(variant);
        let getters = schemes.iter().map(|s| {
            let g = crate::emit::rust::syntax::snake_ident(&Name::new(s));
            quote!(values.next()?.#g()?)
        });
        quote!(#(#indices)|* => Some(#m::Security::#v(#(#getters),*)),)
    });
    let take = if anonymous {
        quote!(let Some(passed) = passed else { return Some(#m::Security::Anonymous) };)
    } else {
        quote!(let passed = passed?;)
    };
    quote! {
        |passed: Option<runtime::Passed<_>>| {
            #take
            let mut values = passed.values.into_iter();
            match passed.alternative {
                #(#arms)*
                _ => None,
            }
        }
    }
}

/// Returns `runtime::Operations::verify` of the server of `ops`: the `verify_` method of the scheme of the credential,
/// on the `Api` that `api` reaches (`&*self.0`), its value a variant of `values` (`SchemeValue`).
pub(crate) fn verify_fn(api: &Api, ops: &[&Operation], reach: &TokenStream, values: &TokenStream) -> TokenStream {
    let value_type = if used_schemes(api, ops).is_empty() { quote!(()) } else { quote!(#values<A>) };
    let signature = |names: TokenStream| {
        quote! {
            fn verify<'r>(&'r self, #names) -> runtime::BoxFuture<'r, Result<Option<runtime::Verified<#value_type>>, runtime::ApiError>>
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
        let v = ident(&variant);
        let verified = match scope_enum(api, scheme) {
            None => quote!(#call.map(|v| runtime::Verified::new(#values::#v(v)))),
            Some(scope) => quote!(#call.map(|g| runtime::Verified {
                value: #values::#v(g.value),
                scopes: g.scopes.into_iter().map(#scope::as_str).collect(),
            })),
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
