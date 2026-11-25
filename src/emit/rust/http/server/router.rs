//! The router: the `Op` enum, the route trie, base paths and the `Routes` that honig serves.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};

use crate::emit::rust::http::paths::specificity;
use crate::emit::rust::http::paths::{Seg, template_segs};
use crate::emit::rust::http::shared::{DISPATCH_CHUNK, is_path, num, operation_variant};
use crate::emit::rust::syntax::lit;
use crate::emit::rust::syntax::snake_ident;
use crate::ir::Api;
use crate::ir::http::Operation;

/// Returns the routes that a framework that routes first (axum, matchit) registers. These are the routes of the path
/// operations. Each template comes alone and under each base path without duplicates. matchit accepts a variable only
/// as a whole segment. So a segment that contains one (`{name}.{ext}`) is a variable named after its position. So is a
/// literal that axum would refuse (`:x`, `*x`). `Routes` then matches the real template.
pub(crate) fn axum_paths(api: &Api) -> BTreeSet<String> {
    let bases = served_bases(api);
    let mut out = BTreeSet::new();
    for op in api.http.operations.iter().filter(|o| is_path(o)) {
        let path: String = template_segs(op)
            .iter()
            .enumerate()
            .map(|(i, seg)| match seg {
                Seg::Lit(l) if !l.starts_with([':', '*']) => format!("/{}", l.replace('{', "{{").replace('}', "}}")),
                Seg::Lit(_) | Seg::Var | Seg::Mixed(_) => format!("/{{s{i}}}"),
            })
            .collect();
        for base in &bases {
            out.insert(format!("{}{path}", base.trim_end_matches('/')));
        }
        out.insert(path);
    }
    out
}

/// Returns `dispatch`: the routed operation to its handler. Past `DISPATCH_CHUNK` operations each chunk of them gets a
/// function of its own. One level of chunks and not a chain: nested futures would grow with the operation count and hit
/// the layout depth limit of the compiler.
pub(crate) fn dispatch_fns(path_ops: &[&Operation]) -> TokenStream {
    let sig = quote!(routed: ::honig::__private::Routed<'_>, call: ::honig::__private::Call<'_, Option<::honig::Passed<<Routes<A> as ::honig::__private::Operations>::Value>>, Operation>);
    // The future of each operation is boxed. So `dispatch` stays a small match. Otherwise it would be one state
    // machine that contains every operation (and your handler) inline. That is large in code and on the stack.
    let arm = |op: &&Operation| {
        let (v, name) = (operation_variant(op), snake_ident(&op.name));
        quote!(Operation::#v => Box::pin(#name(api, routed, call)).await,)
    };
    if path_ops.len() <= DISPATCH_CHUNK {
        let arms = path_ops.iter().map(arm);
        return quote! {
            async fn dispatch<A: Api>(api: &A, op: Operation, #sig) -> ::honig::Response {
                match op { #(#arms)* }
            }
        };
    }
    // Each chunk matches the position of the operation in it: the last position takes the rest of the integers.
    let chunks: Vec<&[&Operation]> = path_ops.chunks(DISPATCH_CHUNK).collect();
    let fs: Vec<_> = (0..chunks.len()).map(|ci| format_ident!("dispatch_{ci}")).collect();
    let groups = chunks.iter().zip(&fs).flat_map(|(ops, f)| {
        ops.iter().enumerate().map(move |(i, op)| {
            let (v, i) = (operation_variant(op), num(i));
            quote!(Operation::#v => #f(api, #i, routed, call).await,)
        })
    });
    let parts = chunks.iter().zip(&fs).map(|(ops, f)| {
        let arms = ops.iter().enumerate().map(|(i, op)| {
            let name = snake_ident(&op.name);
            let at = if i + 1 == ops.len() { quote!(_) } else { num(i).into_token_stream() };
            quote!(#at => Box::pin(#name(api, routed, call)).await,)
        });
        quote! {
            async fn #f<A: Api>(api: &A, at: usize, #sig) -> ::honig::Response {
                match at { #(#arms)* }
            }
        }
    });
    quote! {
        async fn dispatch<A: Api>(api: &A, op: Operation, #sig) -> ::honig::Response {
            match op { #(#groups)* }
        }
        #(#parts)*
    }
}

/// A node of the route trie under construction.
#[derive(Default)]
pub(crate) struct TrieNode {
    pub(crate) lits: BTreeMap<String, usize>,
    /// Mixed segments as (pieces, child) in specificity order.
    pub(crate) mixed: Vec<(Vec<Result<String, ()>>, usize)>,
    pub(crate) var: Option<usize>,
    /// Index into the method groups.
    pub(crate) leaf: Option<usize>,
}

/// Returns the route trie (root first) and the (method, operation) pairs of each template.
/// Templates go in in specificity order. So where two reach the same
/// node (`/a/{id}`, `/a/{name}`) their methods join one group with the first first.
/// Group numbers follow specificity. `::honig::__private::route` picks the lowest leaf
/// among mixed siblings that both match.
pub(crate) fn route_trie(routes: &[&Operation]) -> (Vec<TrieNode>, Vec<Vec<TokenStream>>) {
    let mut nodes = vec![TrieNode::default()];
    let mut groups: Vec<Vec<TokenStream>> = vec![];
    for op in routes {
        let mut cur = 0;
        for seg in template_segs(op) {
            let next = nodes.len();
            cur = match seg {
                Seg::Lit(l) => *nodes[cur].lits.entry(l).or_insert(next),
                Seg::Var => *nodes[cur].var.get_or_insert(next),
                Seg::Mixed(pieces) => match nodes[cur].mixed.iter().find(|(p, _)| *p == pieces) {
                    Some((_, c)) => *c,
                    None => {
                        nodes[cur].mixed.push((pieces, next));
                        next
                    }
                },
            };
            if cur == next {
                nodes.push(TrieNode::default());
            }
        }
        let g = *nodes[cur].leaf.get_or_insert_with(|| {
            groups.push(vec![]);
            groups.len() - 1
        });
        let (method, v) = (lit(&op.method), operation_variant(op));
        groups[g].push(quote!((#method, Operation::#v)));
    }
    (nodes, groups)
}

/// Returns `::honig::__private::Node`s of the trie. Literal children are sorted by length and then bytes
/// as `::honig::__private::route` searches them.
pub(crate) fn trie_nodes(nodes: &[TrieNode]) -> Vec<TokenStream> {
    let index = |n: Option<usize>| {
        n.map_or_else(
            || quote!(::honig::__private::Node::NONE),
            |n| {
                let n = num(n);
                quote!(#n)
            },
        )
    };
    nodes
        .iter()
        .map(|n| {
            let mut lits: Vec<(&String, &usize)> = n.lits.iter().collect();
            lits.sort_by(|a, b| (a.0.len(), a.0).cmp(&(b.0.len(), b.0)));
            let lits = lits.into_iter().map(|(l, c)| {
                let (l, c) = (lit(l), num(*c));
                quote!((#l, #c))
            });
            let mixed = n.mixed.iter().map(|(pieces, c)| {
                let parts = pieces.iter().map(|p| match p {
                    Ok(l) => {
                        let l = lit(l);
                        quote!(::honig::__private::Part::Lit(#l))
                    }
                    Err(()) => quote!(::honig::__private::Part::Var),
                });
                let c = num(*c);
                quote!((&[#(#parts),*], #c))
            });
            let (var, leaf) = (index(n.var), index(n.leaf));
            quote!(::honig::__private::node(&[#(#lits),*], &[#(#mixed),*], #var, #leaf))
        })
        .collect()
}

/// Server URLs a description's variables may expand to before the rest are left out: `enum`s multiply.
const MAX_SERVER_URLS: usize = 64;

/// Returns the base paths of the API's servers and of every operation's own `servers` (on the operation or its path
/// item). A merged description keeps each file's servers on its path items. A request under any of these bases is
/// routed by the rest of its path.
fn served_bases(api: &Api) -> Vec<String> {
    let servers: Vec<crate::ir::Server> =
        api.servers.iter().chain(api.http.operations.iter().flat_map(|o| o.servers.iter())).cloned().collect();
    base_paths(&servers)
}

/// Returns the path part of each server URL (3.2 section 4.4: "the full URL is the server
/// URL followed by the path"). Every variable is expanded to its `enum`
/// values or else its `default`. The longest come first and `/` alone is left out. Relative
/// URLs that do not start with `/` depend on where the document is served.
/// They are left out.
pub(crate) fn base_paths(servers: &[crate::ir::Server]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for s in servers {
        // Variables first: in `{scheme}://{host}{basePath}` only the expanded URL shows where the path starts.
        let mut expanded = vec![s.url.clone()];
        for (name, default, allowed) in &s.variables {
            let values: Vec<&String> = if allowed.is_empty() { vec![default] } else { allowed.iter().collect() };
            let var = format!("{{{name}}}");
            let var = var.as_str();
            expanded = expanded
                .iter()
                .flat_map(|p| values.iter().map(move |v| if p.contains(var) { p.replace(var, v) } else { p.clone() }))
                .take(MAX_SERVER_URLS)
                .collect();
        }
        for url in expanded {
            let path = match url.split_once("://") {
                Some((_, rest)) => rest.find('/').and_then(|i| rest.get(i..)).unwrap_or(""),
                None if url.starts_with('/') => url.as_str(),
                None => continue,
            };
            let path = path.split(['?', '#']).next().unwrap_or("").trim_end_matches('/');
            if !path.is_empty() && !path.contains('{') {
                out.insert(path.to_owned());
            }
        }
    }
    let mut out: Vec<String> = out.into_iter().collect();
    out.sort_by_key(|p| std::cmp::Reverse(p.len()));
    out
}

/// Returns the body of `Operations::route`: the operation of the method on the leaf of the path, and where the variables
/// of its template are. `strip_base` binds `found` without the base path when it has one.
fn route_body(strip_base: &TokenStream) -> TokenStream {
    quote! {
        let (path, _) = ::honig::__private::split_target(target);
        let path = ::honig::__private::normalize_path(path);
        let path: &str = &path;
        #strip_base
        let Some((methods, caps)) = found.and_then(|(leaf, caps)| Some((*METHODS.get(leaf)?, caps))) else {
            return Err(::honig::Reason::NotFound);
        };
        ::honig::__private::find_method(methods, method)
            .map(|(_, op)| (*op, ::honig::__private::Spans::of(path, &caps)))
            .ok_or_else(|| ::honig::Reason::MethodNotAllowed(::honig::__private::allowed_methods(methods)))
    }
}

/// Returns the body of `Operations::call`: the server routed the request, so its operation and the variables of the
/// path that it routed come along.
fn call_body() -> TokenStream {
    quote! {
        Box::pin(async move {
            let mut req = req;
            let uri = call.uri().clone();
            let (path, raw_query) = ::honig::__private::split_target(::honig::__private::path_and_query(&uri));
            let path = ::honig::__private::normalize_path(path);
            let caps = call.captures(&path);
            let query = ::honig::__private::parse_query(raw_query);
            let routed = ::honig::__private::Routed { request: &mut req, caps: &caps, raw_query, query: &query };
            dispatch(&*self.0, call.operation(), routed, call).await
        })
    }
}

pub(crate) fn router(api: &Api) -> TokenStream {
    let bases = served_bases(api);
    let strip_base = (!bases.is_empty()).then(|| {
        quote! {
            // Routed once. The stripped path if it matches or else the path as it is.
            let found = BASE_PATHS
                .iter()
                .find_map(|b| path.strip_prefix(b).filter(|rest| rest.is_empty() || rest.starts_with('/')))
                .map(|rest| if rest.is_empty() { "/" } else { rest })
                .and_then(|rest| ::honig::__private::route(ROUTES, rest))
                .or_else(|| ::honig::__private::route(ROUTES, path));
        }
    });
    let strip_base = strip_base.unwrap_or_else(|| quote!(let found = ::honig::__private::route(ROUTES, path);));
    let base_paths = (!bases.is_empty()).then(|| {
        let bases = bases.iter().map(|b| lit(b));
        quote! {
            static BASE_PATHS: &[&str] = &[#(#bases),*];
        }
    });
    let paths = axum_paths(api).into_iter().map(|p| lit(&p));
    let path_ops: Vec<&Operation> = api.http.operations.iter().filter(|o| is_path(o)).collect();
    // Webhooks only: nothing to route by path.
    if path_ops.is_empty() {
        return TokenStream::new();
    }
    let mut routes = path_ops.clone();
    routes.sort_by_key(|op| specificity(op));
    let (nodes, groups) = route_trie(&routes);
    let nodes = trie_nodes(&nodes);
    let groups = groups.iter().map(|g| quote!(&[#(#g),*]));
    let (route, call) = (route_body(&strip_base), call_body());
    let dispatch = dispatch_fns(&path_ops);
    let verify = super::verify_fn(api, &path_ops, &quote!(&*self.0), &quote!(SchemeValue));
    let value = super::value_type(api, &path_ops, "SchemeValue");
    quote! {
        static METHODS: &[&[(&str, Operation)]] = &[#(#groups),*];

        static ROUTES: &[::honig::__private::Node] = &[#(#nodes),*];

        #base_paths

        static PATHS: &[&str] = &[#(#paths),*];

        pub struct Routes<A>(std::sync::Arc<A>);

        /// The server of this API: `Server::new(your_api)`, or `Server::new(arc)` to keep a handle on it. Add layers,
        /// then `into_axum` or `into_service`.
        pub type Server<A> = ::honig::Server<Routes<A>>;

        impl<A: Api> From<std::sync::Arc<A>> for Routes<A> {
            fn from(api: std::sync::Arc<A>) -> Self {
                Self(api)
            }
        }

        impl<A: Api> From<A> for Routes<A> {
            fn from(api: A) -> Self {
                Self(std::sync::Arc::new(api))
            }
        }

        impl<A: Api> ::honig::__private::Operations for Routes<A> {
            type Value = #value;
            type Operation = Operation;

            fn error_response(&self) -> fn(::honig::Rejection<Operation>) -> ::honig::Response {
                |rejection| A::error_response(rejection)
            }

            fn paths(&self) -> &'static [&'static str] {
                PATHS
            }

            fn route(&self, method: &str, target: &str) -> Result<(Operation, ::honig::__private::Spans), ::honig::Reason> {
                #route
            }

            fn errors(&self) -> ::honig::__private::ProblemSchemas {
                error_schema
            }

            #verify

            fn call<'r>(&'r self, req: ::honig::Request, call: ::honig::__private::Call<'r, Option<::honig::Passed<#value>>, Operation>) -> ::honig::__private::BoxFuture<'r, ::honig::Response> {
                #call
            }
        }

        #dispatch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(url: &str, vars: &[(&str, &str, &[&str])]) -> crate::ir::Server {
        crate::ir::Server {
            url: url.to_owned(),
            name: None,
            variables: vars
                .iter()
                .map(|(n, d, a)| ((*n).to_owned(), (*d).to_owned(), a.iter().map(|x| (*x).to_owned()).collect()))
                .collect(),
            doc: None,
        }
    }

    #[test]
    fn a_base_path_in_a_variable_after_the_host_counts() {
        // No `/` after the host: only expanding shows the path.
        let s = server(
            "{protocol}://{hostname}{basePath}",
            &[
                ("protocol", "https", &["http", "https"]),
                ("hostname", "localhost:8008", &[]),
                ("basePath", "/api/client/v3", &[]),
            ],
        );
        assert_eq!(base_paths(&[s]), ["/api/client/v3"]);
        assert_eq!(base_paths(&[server("https://api.example.com/v1/", &[])]), ["/v1"]);
    }
}
