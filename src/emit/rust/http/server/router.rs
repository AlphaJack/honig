//! The router: the `Op` enum, the route trie, base paths and the `Routes` that the runtime serves.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

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
    let sig = quote!(routed: runtime::Routed<'_>, call: runtime::Call<'_, Option<A::Principal>, Operation>);
    // The future of each operation is boxed. So `dispatch` stays a small match. Otherwise it would be one state
    // machine that contains every operation (and your handler) inline. That is large in code and on the stack.
    let arm = |op: &&Operation| {
        let (v, name) = (operation_variant(op), snake_ident(&op.name));
        quote!(Operation::#v => Box::pin(#name(api, routed, call)).await,)
    };
    if path_ops.len() <= DISPATCH_CHUNK {
        let arms = path_ops.iter().map(arm);
        return quote! {
            async fn dispatch<A: Api>(api: &A, op: Operation, #sig) -> runtime::Response {
                match op { #(#arms)* }
            }
        };
    }
    let chunks: Vec<&[&Operation]> = path_ops.chunks(DISPATCH_CHUNK).collect();
    let fs: Vec<_> = (0..chunks.len()).map(|ci| format_ident!("dispatch_{ci}")).collect();
    let groups = chunks.iter().zip(&fs).map(|(ops, f)| {
        let vs = ops.iter().map(|op| operation_variant(op));
        quote!(#(Operation::#vs)|* => #f(api, op, routed, call).await,)
    });
    let parts = chunks.iter().zip(&fs).map(|(ops, f)| {
        let arms = ops.iter().map(arm);
        quote! {
            async fn #f<A: Api>(api: &A, op: Operation, #sig) -> runtime::Response {
                match op {
                    #(#arms)*
                    _ => unreachable!("`dispatch` passes only the operations of this chunk"),
                }
            }
        }
    });
    quote! {
        async fn dispatch<A: Api>(api: &A, op: Operation, #sig) -> runtime::Response {
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
/// Group numbers follow specificity. `runtime::route` picks the lowest leaf
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

/// Returns `runtime::Node`s of the trie. Literal children are sorted by length and then bytes
/// as `runtime::route` searches them.
pub(crate) fn trie_nodes(nodes: &[TrieNode]) -> Vec<TokenStream> {
    let index = |n: Option<usize>| {
        n.map_or_else(
            || quote!(runtime::Node::NONE),
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
                        quote!(runtime::Part::Lit(#l))
                    }
                    Err(()) => quote!(runtime::Part::Var),
                });
                let c = num(*c);
                quote!((&[#(#parts),*], #c))
            });
            let (var, leaf) = (index(n.var), index(n.leaf));
            quote!(runtime::node(&[#(#lits),*], &[#(#mixed),*], #var, #leaf))
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

pub(crate) fn router(api: &Api) -> TokenStream {
    let bases = served_bases(api);
    let strip_base = (!bases.is_empty()).then(|| {
        quote! {
            // Routed once. The stripped path if it matches or else the path as it is.
            let found = BASE_PATHS
                .iter()
                .find_map(|b| path.strip_prefix(b).filter(|rest| rest.is_empty() || rest.starts_with('/')))
                .map(|rest| if rest.is_empty() { "/" } else { rest })
                .and_then(|rest| runtime::route(ROUTES, rest))
                .or_else(|| runtime::route(ROUTES, path));
        }
    });
    let strip_base = strip_base.unwrap_or_else(|| quote!(let found = runtime::route(ROUTES, path);));
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
    // The leaf of the path with its (method, operation) pairs and the variables of the template.
    let route = quote! {
        let (path, _) = runtime::split_target(target);
        let path = runtime::normalize_path(path);
        let path: &str = &path;
        #strip_base
        let Some((methods, caps)) = found.and_then(|(leaf, caps)| Some((*METHODS.get(leaf)?, caps))) else {
            return Err(runtime::Reason::NotFound);
        };
        runtime::find_method(methods, method)
            .map(|(_, op)| (*op, runtime::Spans::of(path, &caps)))
            .ok_or_else(|| runtime::Reason::MethodNotAllowed(runtime::allowed_methods(methods)))
    };
    // The server routed the request: its operation and the variables of the path that it routed come along.
    let call = quote! {
        Box::pin(async move {
            let mut req = req;
            let uri = call.uri().clone();
            let (path, raw_query) = runtime::split_target(runtime::path_and_query(&uri));
            let path = runtime::normalize_path(path);
            let caps = call.captures(&path);
            let query = runtime::parse_query(raw_query);
            let routed = runtime::Routed { request: &mut req, caps: &caps, raw_query, query: &query };
            dispatch(&*self.0, call.operation(), routed, call).await
        })
    };
    let dispatch = dispatch_fns(&path_ops);
    let verify = super::verify_fn(api, &path_ops, &quote!(&*self.0), &quote!(A::Principal));
    quote! {
        static METHODS: &[&[(&str, Operation)]] = &[#(#groups),*];

        static ROUTES: &[runtime::Node] = &[#(#nodes),*];

        #base_paths

        static PATHS: &[&str] = &[#(#paths),*];

        pub struct Routes<A>(std::sync::Arc<A>);

        /// The server of this API: `Server::new(your_api)`, or `Server::new(arc)` to keep a handle on it. Add layers,
        /// then `into_axum` or `into_service`.
        pub type Server<A> = runtime::Server<Routes<A>>;

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

        impl<A: Api> runtime::Operations for Routes<A> {
            type Principal = A::Principal;
            type Operation = Operation;

            fn error_response(&self) -> fn(runtime::Rejection<Operation>) -> runtime::Response {
                |rejection| A::error_response(rejection)
            }

            fn paths(&self) -> &'static [&'static str] {
                PATHS
            }

            fn route(&self, method: &str, target: &str) -> Result<(Operation, runtime::Spans), runtime::Reason> {
                #route
            }

            fn errors(&self) -> runtime::ProblemSchemas {
                error_schema
            }

            #verify

            fn call<'r>(&'r self, req: runtime::Request, call: runtime::Call<'r, Option<A::Principal>, Operation>) -> runtime::BoxFuture<'r, runtime::Response> {
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
