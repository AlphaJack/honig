//! `server.rs`: the `Api` trait, the router and the server adapters.

mod auth;
mod encode;
mod errors;
mod params;
mod router;

pub(crate) use auth::*;
pub(crate) use encode::*;
pub(crate) use errors::*;
pub(crate) use params::*;
pub(crate) use router::*;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::security::Securities;
use super::shared::{boxing, hook_name, is_path, is_success, op_path, op_static, operation_variant};
use crate::emit::rust::names::snake;
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{doc_opt, snake_ident};
use crate::emit::rust::syntax::{ident, lit};
use crate::ir::Api;
use crate::ir::http::{Location, Operation, Status};
use crate::ir::media::MediaKind;

/// What an operation's `security` asks of who is calling.
#[derive(Clone, Copy)]
enum Needs {
    /// No `security`: the handler sees no principal.
    Nobody,
    /// An empty alternative (`{}`) allows anonymous requests: the handler sees an `Option`.
    Optional,
    /// Credentials are required. The handler sees the principal. A request without one gets 401.
    SignedIn,
}

fn needs(op: &Operation) -> Needs {
    if op.security.is_empty() {
        Needs::Nobody
    } else if op.security.iter().any(Vec::is_empty) {
        Needs::Optional
    } else {
        Needs::SignedIn
    }
}

/// Returns one `Api` trait method. With `allow_unimplemented` it has a default that answers 501.
pub(crate) fn api_method(op: &Operation, allow_unimplemented: bool) -> TokenStream {
    let (doc, name) = (doc_opt(op.doc.as_deref()), snake_ident(&op.name));
    let m = op_path(op);
    let (params_ty, response_ty) = (quote!(#m::Params), quote!(#m::Result));
    let principal = match needs(op) {
        Needs::Nobody => quote!(()),
        Needs::Optional => quote!(Option<Self::Principal>),
        Needs::SignedIn => quote!(Self::Principal),
    };
    // `allow_unimplemented`: operations added to the description
    // answer 501 until implemented instead of failing the build.
    let body = if allow_unimplemented {
        quote!({
            let _ = (params, ctx);
            std::future::ready(Err(runtime::ApiError::new(
                runtime::http::StatusCode::NOT_IMPLEMENTED,
                "not implemented",
            )))
        })
    } else {
        quote!(;)
    };
    let context = ident(if is_path(op) { "Context" } else { "WebhookContext" });
    quote! {
        #doc
        fn #name(&self, params: #params_ty, ctx: &#context<'_, #principal>)
            -> impl Future<Output = #response_ty> + Send #body
    }
}

/// Returns the generated enum of `op`: `Operation` for a path operation, `WebhookOperation` for a webhook or
/// callback.
fn operation_type(op: &Operation) -> proc_macro2::Ident {
    ident(if is_path(op) { "Operation" } else { "WebhookOperation" })
}

/// Returns `name`: one variant per operation of `ops`, each to its `runtime::Op`. Then the runtime types typed by it
/// under the names in `aliases` (`Context` to `runtime::Context`), the one way your code refers to them.
fn operation_enum(name: &str, ops: &[&Operation], aliases: &[(&str, &str)]) -> TokenStream {
    let name = ident(name);
    let variants = ops.iter().map(|op| operation_variant(op));
    let arms = ops.iter().map(|op| {
        let (v, info) = (operation_variant(op), op_static(op));
        quote!(Self::#v => &#info,)
    });
    let aliases = aliases.iter().map(|(alias, base)| {
        let (alias, base) = (ident(alias), ident(base));
        match base.to_string().as_str() {
            "Context" => quote!(pub type #alias<'a, C = ()> = runtime::Context<'a, C, #name>;),
            _ => quote!(pub type #alias = runtime::#base<#name>;),
        }
    });
    quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #name { #(#variants),* }

        impl runtime::Operation for #name {
            fn op(self) -> &'static runtime::Op {
                match self { #(#arms)* }
            }
        }

        #(#aliases)*
    }
}

/// Returns `error_response` of `Api` or `Webhooks`: how the API answers a request that the server refuses itself.
/// Where every error body of the description is one that the server fills by name (RFC 9457 `problem+json`, a text
/// message), it has a default. Otherwise your impl writes it.
fn error_response_method(operation: &TokenStream, filled: bool) -> TokenStream {
    let body = if filled { quote!({ rejection.response() }) } else { quote!(;) };
    let doc = if filled {
        " Answers a request that the server refuses itself (no route, bad input, missing credentials, a handler `Err`). \
         Default: RFC 9457 `problem+json`, or the error body of the description that it fills by name."
    } else {
        " Answers a request that the server refuses itself (no route, bad input, missing credentials, a handler `Err`) \
         in the error body of the description. No standard defines its fields, so this maps them: \
         `rejection.json(&body)` sends `body` with the status of the reason, and `rejection.message()` is the detail."
    };
    quote! {
        #[doc = #doc]
        fn error_response(rejection: runtime::Rejection<#operation>) -> runtime::Response #body
    }
}

/// Returns the `Principal` associated type of `Api` and `Webhooks`.
fn caller_type() -> TokenStream {
    quote! {
        type Principal: Clone + Send + Sync + 'static;
    }
}

pub(crate) fn server(api: &Api, space: &Space<'_>, allow_unimplemented: bool) -> TokenStream {
    let methods = api.http.operations.iter().filter(|o| is_path(o)).map(|op| api_method(op, allow_unimplemented));
    let filled = all_filled(api, space);
    let router = router(api);
    let credential = credential_enum(api);
    let scope = api.schemes.iter().filter_map(|s| super::security::scope_enum(api, s)).collect::<Vec<_>>();
    let scope = (!scope.is_empty()).then(|| quote!(pub use super::operations::{#(#scope),*};));
    let error_schemas = error_schema_fn(api, space);
    let sec = Securities::new(api);
    let ops = api.http.operations.iter().map(|op| server_op(space, api, &sec, op));
    let principal = caller_type();
    let webhooks = webhooks(api, allow_unimplemented, filled);
    let path_ops: Vec<&Operation> = api.http.operations.iter().filter(|o| is_path(o)).collect();
    let verify = verify_methods(api, &path_ops, allow_unimplemented);
    let operations = (!path_ops.is_empty()).then(|| {
        operation_enum(
            "Operation",
            &path_ops,
            &[("Context", "Context"), ("Rejection", "Rejection"), ("RequestEvent", "RequestEvent")],
        )
    });
    let error_response = (!path_ops.is_empty()).then(|| error_response_method(&quote!(Operation), filled));
    quote! {
        use super::{operations as ops, runtime};
        use std::future::Future;

        pub trait Api: Send + Sync + 'static {
            #principal
            #error_response
            #verify
            #(#methods)*
        }

        #operations
        #credential
        #scope
        #error_schemas

        #webhooks
        #router
        #(#ops)*
    }
}

/// Returns the `Webhooks` trait and the server of each webhook or callback when the description declares any.
fn webhooks(api: &Api, allow_unimplemented: bool, filled: bool) -> Option<TokenStream> {
    let principal = caller_type();
    let hooks: Vec<&Operation> = api.http.operations.iter().filter(|o| !is_path(o)).collect();
    (!hooks.is_empty()).then(|| {
    let methods = hooks.iter().map(|op| api_method(op, allow_unimplemented));
    let error_response = error_response_method(&quote!(WebhookOperation), filled);
    let verify = verify_methods(api, &hooks, allow_unimplemented);
    let dispatch = verify_fn(api, &hooks, &quote!(&*self.api), &quote!(A::Principal));
    let found = hooks.iter().map(|op| {
        let (key, method, v) = (lit(&hook_name(op)), lit(&op.method), operation_variant(op));
        quote!((#key, #method) => Ok((WebhookOperation::#v, runtime::Spans::default())),)
    });
    let operations = operation_enum("WebhookOperation", &hooks, &[("WebhookContext", "Context")]);
    let pairs = hooks.iter().map(|op| {
        let (key, method) = (lit(&hook_name(op)), lit(&op.method));
        quote!((#key, #method))
    });
    let arms = hooks.iter().map(|op| {
        let (key, method, f) = (lit(&hook_name(op)), lit(&op.method), snake_ident(&op.name));
        quote!((#key, #method) => #f(api, routed, call).await,)
    });
    quote! {
        /// Answers the webhooks of the API and the callbacks (`callbacks`) of its operations: the requests that it sends.
        pub trait Webhooks: Send + Sync + 'static {
            #principal
            #error_response
            #verify
            #(#methods)*
        }

        pub struct WebhookRoutes<A> {
            api: std::sync::Arc<A>,
            name: &'static str,
        }

        /// Returns the server of the webhook or callback `name`, answered by `api`.
        #[must_use]
        pub fn webhook<A: Webhooks>(api: std::sync::Arc<A>, name: &'static str) -> runtime::Server<WebhookRoutes<A>> {
            runtime::Server::new(WebhookRoutes { api, name })
        }

        #operations

        impl<A: Webhooks> runtime::Operations for WebhookRoutes<A> {
            type Principal = A::Principal;
            type Operation = WebhookOperation;

            fn error_response(&self) -> fn(runtime::Rejection<WebhookOperation>) -> runtime::Response {
                |rejection| A::error_response(rejection)
            }

            fn paths(&self) -> &'static [&'static str] {
                &[]
            }

            fn route(&self, method: &str, _: &str) -> Result<(WebhookOperation, runtime::Spans), runtime::Reason> {
                match (self.name, method) {
                    #(#found)*
                    (name, _) => {
                        let allowed: Vec<&'static str> = WEBHOOK_METHODS.iter().filter(|(n, _)| *n == name).map(|(_, m)| *m).collect();
                        Err(if allowed.is_empty() { runtime::Reason::NotFound } else { runtime::Reason::MethodNotAllowed(allowed) })
                    }
                }
            }

            fn errors(&self) -> runtime::ProblemSchemas {
                error_schema
            }

            #dispatch

            fn call<'r>(&'r self, req: runtime::Request, call: runtime::Call<'r, Option<A::Principal>, WebhookOperation>) -> runtime::BoxFuture<'r, runtime::Response> {
                Box::pin(async move {
                    let api = &*self.api;
                    let mut req = req;
                    let uri = req.uri().clone();
                    let raw_query = uri.query().unwrap_or_default();
                    let query = runtime::parse_query(raw_query);
                    let routed = runtime::Routed { request: &mut req, caps: &[], raw_query, query: &query };
                    match (self.name, routed.request.method().as_str()) {
                        #(#arms)*
                        // `route` found the operation first.
                        _ => runtime::new_response(404),
                    }
                })
            }
        }

        static WEBHOOK_METHODS: &[(&str, &str)] = &[#(#pairs),*];
    }
    })
}

/// Returns the handler of one operation: its `runtime::Op` and what `runtime::serve` calls to decode, run and encode
/// it. Does the server answer `Range` for `op`? Yes for a `GET` whose success response is binary. Not if it declares a
/// `206` (or `2XX`) or a `Range` parameter. Then its handler answers ranges itself.
fn serves_ranges(op: &Operation) -> bool {
    let own_206 = op.responses.iter().any(|r| {
        matches!(r.status, Status::Code(c) if c == runtime::http::StatusCode::PARTIAL_CONTENT)
            || r.status == Status::Range(2)
    });
    let own_range = op.params.iter().any(|p| p.location == Location::Header && p.wire.eq_ignore_ascii_case("range"));
    let binary =
        op.responses.iter().filter(|r| is_success(r)).flat_map(|r| &r.contents).any(|c| c.kind == MediaKind::Bytes);
    op.method == "GET" && binary && !own_206 && !own_range
}

pub(crate) fn server_op(space: &Space<'_>, api: &Api, sec: &Securities, op: &Operation) -> TokenStream {
    let m = op_path(op);
    let name = snake_ident(&op.name);
    let bx = boxing(space, op);
    let inputs = param_inputs(op);
    let empty = op.params.is_empty() && op.body.is_none() && !crate::emit::rust::http::shared::chooses_media(op);
    let security = sec.of(api, op);
    let op_name = lit(&snake(&op.name));
    let tr = ident(if is_path(op) { "Api" } else { "Webhooks" });
    let route_text = if op.path.is_empty() { hook_name(op) } else { op.path.clone() };
    let (method, route, span) = (lit(&op.method), lit(&route_text), lit(&format!("{} {route_text}", op.method)));
    let take = inputs.take;
    let (params_fn, encode_fn) =
        (format_ident!("{}_params", snake(&op.name)), format_ident!("{}_encode", snake(&op.name)));
    // Decoding borrows the request unless it moves the body out; encoding borrows a response that carries nothing.
    let decode = match (empty, inputs.fallible, inputs.body) {
        (true, ..) => quote!(|_| Ok(#m::Params {})),
        (false, true, true) => quote!(#params_fn),
        (false, true, false) => quote!(|i| #params_fn(&i)),
        (false, false, true) => quote!(|i| Ok(#params_fn(i))),
        (false, false, false) => quote!(|i| Ok(#params_fn(&i))),
    };
    // The encoder receives the media type `Accept` chose only where a response is one type in several.
    let encode = match (encode_fallible(op), carries_data(op), encodes_by_media(space, op)) {
        (true, true, true) => quote!(#encode_fn),
        (true, true, false) => quote!(|r, _| #encode_fn(r)),
        (true, false, _) => quote!(|r, _| #encode_fn(&r)),
        (false, true, true) => quote!(|r, m| Ok(#encode_fn(r, m))),
        (false, true, false) => quote!(|r, _| Ok(#encode_fn(r))),
        (false, false, _) => quote!(|r, _| Ok(#encode_fn(&r))),
    };
    let params_def = (!empty).then(|| server_params(space, op, &m, &inputs, &bx));
    let encode_def = server_encode(space, op, &m, &bx);
    let info = op_static(op);
    let produces = crate::emit::rust::http::shared::produces(op).into_iter().map(|(m, _)| lit(m));
    let ranges = serves_ranges(op);
    let serve = match needs(op) {
        Needs::Nobody => quote!(runtime::serve(routed, call.public(), &steps).await),
        Needs::Optional => quote!(runtime::serve(routed, call, &steps).await),
        Needs::SignedIn => quote!(runtime::serve_signed_in(routed, call, &steps).await),
    };
    let id = operation_type(op);
    // Only what differs from `Op::new`.
    let produces: Vec<proc_macro2::Literal> = produces.collect();
    let set: Vec<TokenStream> = (!op.security.is_empty())
        .then(|| quote!(security: #security,))
        .into_iter()
        .chain(take.then(|| quote!(takes_body: true,)))
        .chain((!produces.is_empty()).then(|| quote!(produces: &[#(#produces),*],)))
        .chain(ranges.then(|| quote!(ranges: true,)))
        .collect();
    let new = quote!(runtime::Op::new(#op_name, #method, #route, #span, error_schema));
    let op_value = if set.is_empty() { new } else { quote!(runtime::Op { #(#set)* ..#new }) };
    quote! {
        static #info: runtime::Op = #op_value;
        async fn #name<A: #tr>(api: &A, routed: runtime::Routed<'_>, call: runtime::Call<'_, Option<A::Principal>, #id>) -> runtime::Response {
            let steps = runtime::Steps { api, decode: #decode, call: |a, p, c| Box::pin(a.#name(p, c)), encode: #encode };
            #serve
        }
        #params_def
        #encode_def
    }
}
