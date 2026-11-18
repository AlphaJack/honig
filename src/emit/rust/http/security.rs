//! The security requirements that each operation declares as the client and server name them.

use std::collections::BTreeMap;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::emit::rust::syntax::lit;
use crate::ir::Api;
use crate::ir::http::Operation;
use crate::ir::security::SchemeKind;

/// Each distinct list of security requirements that the operations declare. Each is declared once in
/// `operations.rs` as `SECURITY_<n>` in order of first use. An operation that needs none names `&[]`.
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
