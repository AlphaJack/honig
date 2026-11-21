//! `operations.rs`: the parameters, responses, links and callbacks of each operation.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use super::shared::{Boxing, VariantShape};
use super::shared::{
    blob, body_ty, boxed, boxing, content_ty, eq_derive, param_init, server_url, spec_default, status_code,
    variant_shape,
};
use crate::emit::rust::names::{off_prelude, pascal, snake};
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{doc_opt, ident, json_tokens, lit, mentions, ty};
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::Api;
use crate::ir::http::{Content, Link, LinkTarget, Location, OpKind, Operation, Param, Resp, Status};
use crate::ir::names::Name;
use crate::ir::types::{Kind, Ty};

/// Returns `operations.rs`. `conveniences`: `METHOD`, `PATH`, `Params::new` and the `tags` modules for code that calls
/// the API. A binary that only serves it (no client) never refers to them. So they are left out and not left unused.
/// In a binary an item that nothing names is dead code, and a handler may build only some of the responses of its
/// operation, or none when it is left to its `501` default.
pub(crate) fn operations(api: &Api, space: &Space<'_>, opts: &crate::emit::Options) -> TokenStream {
    let (binary, serves) = (opts.rust.binary, opts.server);
    let conveniences = !binary || opts.client;
    let mods = api.http.operations.iter().map(|op| operation_mod(api, space, op, conveniences));
    let dead_code = match (binary, serves, conveniences) {
        (false, ..) => None,
        (true, true, _) => Some("a handler may build only some of the responses of its operation"),
        (true, false, true) => {
            Some("a binary calls part of the client: its `Params::new`, `METHOD` and `PATH` may go unused")
        }
        (true, false, false) => Some("without a client or a server a binary refers to none of the operations"),
    };
    let defaults = dead_code.map(|reason| quote!(#![allow(dead_code, reason = #reason)]));
    let header = quote!(#defaults);
    let tags = conveniences.then(|| tags_mod(api)).flatten();
    let responses = responses_mod(space, &api.http.operations);
    let security = super::security::Securities::new(api).consts();
    quote! { #header #security #(#mods)* #responses #tags }
}

/// Returns `use super::super::types;` for a module of `operations.rs` that refers to a schema type.
fn types_import(needed: bool) -> Option<TokenStream> {
    needed.then(|| {
        quote!(
            use super::super::types;
        )
    })
}

/// Returns `pub mod responses`: the body of each `components/responses` entry with several media types. Each is
/// declared once. The `<Variant>Body` of an operation refers to it.
fn responses_mod(space: &Space<'_>, ops: &[Operation]) -> Option<TokenStream> {
    let mut seen = BTreeSet::new();
    let mut items = vec![];
    let (mut uses_types, mut uses_runtime) = (false, false);
    let mut shapes: Vec<(String, String)> = vec![];
    for op in ops {
        let bx = boxing(space, op);
        for (i, r) in op.responses.iter().enumerate() {
            let Some(shared) = r.shared.as_ref().filter(|_| r.contents.len() > 1).map(|s| off_prelude(s, "Response"))
            else {
                continue;
            };
            if !seen.insert(shared.clone()) {
                continue;
            }
            // A body like an earlier one (same media types and same types) is that type.
            let key = format!("{:?}", r.contents.iter().map(|c| (&c.media, &c.ty)).collect::<Vec<_>>());
            if let Some((_, first)) = shapes.iter().find(|(k, _)| *k == key) {
                let (name, first) = (format_ident!("{shared}"), format_ident!("{first}"));
                items.push(quote!(pub type #name = #first;));
                continue;
            }
            shapes.push((key, shared.clone()));
            uses_types |= r.contents.iter().any(|c| mentions(&c.ty).0);
            // A sequential or binary body is a runtime type.
            uses_runtime |= r.contents.iter().any(|c| mentions(&c.ty).1 || c.streams() || blob(c));
            items.push(content_enum(space, &shared, &r.contents, &bx.resp_enums[i]));
        }
    }
    let types_import = types_import(uses_types);
    let runtime_import = runtime_import(uses_runtime);
    (!items.is_empty()).then(|| {
        quote! {
            pub mod responses {
                #types_import
                #runtime_import
                #(#items)*
            }
        }
    })
}

/// Tags nested under parents deeper than this are left out: a `parent` chain may loop.
const MAX_TAG_DEPTH: usize = 16;

/// Returns `pub mod tags`: one module per tag re-exporting its operations' modules,
/// nested under its 3.2 `parent`. Operations stay reachable at the top level.
pub(crate) fn tags_mod(api: &Api) -> Option<TokenStream> {
    // A tag shows when it or a descendant has operations.
    fn shown(api: &Api, name: &str, used: &dyn Fn(&str) -> bool, depth: usize) -> bool {
        depth < MAX_TAG_DEPTH
            && (used(name)
                || api.tags.iter().any(|c| c.parent.as_deref() == Some(name) && shown(api, &c.name, used, depth + 1)))
    }
    fn module(
        api: &Api,
        tag: &crate::ir::Tag,
        used: &dyn Fn(&str) -> bool,
        depth: usize,
        taken: &mut BTreeSet<String>,
    ) -> TokenStream {
        let name = snake_ident(&crate::ir::names::unique(Name::new(&tag.name), taken));
        let doc = doc_opt(tag.doc.as_deref());
        let up = (0..depth + 2).map(|_| quote!(super));
        let ops: Vec<_> =
            api.http.operations.iter().filter(|o| o.tags.contains(&tag.name)).map(|o| snake_ident(&o.name)).collect();
        let reexport = (!ops.is_empty()).then(|| quote!(pub use #(#up)::*::{#(#ops),*};));
        let mut inner = BTreeSet::new();
        let children = api
            .tags
            .iter()
            .filter(|c| {
                c.parent.as_deref() == Some(tag.name.as_str())
                    && depth < MAX_TAG_DEPTH
                    && shown(api, &c.name, used, depth + 1)
            })
            .map(|c| module(api, c, used, depth + 1, &mut inner));
        quote! { #doc pub mod #name { #reexport #(#children)* } }
    }
    let used = |t: &str| api.http.operations.iter().any(|o| o.tags.iter().any(|x| x == t));
    let mut taken = BTreeSet::new();
    let roots: Vec<_> = api
        .tags
        .iter()
        .filter(|t| {
            t.parent.as_ref().is_none_or(|p| !api.tags.iter().any(|x| &x.name == p)) && shown(api, &t.name, &used, 0)
        })
        .map(|t| module(api, t, &used, 0, &mut taken))
        .collect();
    (!roots.is_empty()).then(|| {
        quote! {
            pub mod tags { #(#roots)* }
        }
    })
}

/// Returns `use super::super::runtime;` for a module of an operation that refers to the runtime.
fn runtime_import(uses: bool) -> Option<TokenStream> {
    uses.then(|| {
        quote!(
            use super::super::runtime;
        )
    })
}

/// Returns `pub mod <operation>` with constants, `Params`, `Response` and body enums.
pub(crate) fn operation_mod(api: &Api, space: &Space<'_>, op: &Operation, conveniences: bool) -> TokenStream {
    let bx = boxing(space, op);
    let params = params_struct(space, op, conveniences);
    let mut items = vec![params];
    if let Some(body) = &op.body
        && body.contents.len() > 1
    {
        items.push(content_enum(space, "Body", &body.contents, &bx.body_enum));
    }
    response_enum(&mut items, space, op, &bx);
    // `Params`, the bodies and the responses refer to the runtime when they need one of its types or helpers. The links
    // and callbacks below import it for themselves.
    let uses_runtime = quote!(#(#items)*).to_string().contains("runtime ::");
    items.extend(links_mod(api, space, op));
    items.extend(callbacks_mod(api, op));
    // Imports follow from the types used and not from a scan of the text.
    let all_tys =
        op.params.iter().map(|p| &p.ty).chain(op.body.iter().flat_map(|b| b.contents.iter().map(|c| &c.ty))).chain(
            // A shared response's body is typed in `responses`; its headers stay here.
            op.responses.iter().flat_map(|r| {
                let own = r.shared.is_none() || r.contents.len() < 2;
                r.contents.iter().filter(move |_| own).map(|c| &c.ty).chain(r.headers.iter().map(|h| &h.ty))
            }),
        );
    let (uses_types, _) = all_tys.map(mentions).fold((false, false), |a, b| (a.0 || b.0, a.1 || b.1));
    let types_import = types_import(uses_types);
    let runtime_import = runtime_import(uses_runtime);
    let doc = doc_opt(op.doc.as_deref());
    let name = snake_ident(&op.name);
    quote! {
        #doc
        pub mod #name {
            #types_import
            #runtime_import
            #(#items)*
        }
    }
}

/// Returns the operation a link points at.
pub(crate) fn link_target<'a>(api: &'a Api, target: &LinkTarget) -> Option<&'a Operation> {
    api.http.operations.iter().find(|o| match target {
        LinkTarget::Id(id) => o.operation_id.as_deref() == Some(id.as_str()),
        LinkTarget::Pointer(p) => &o.pointer == p,
    })
}

/// Returns a parameter named by a link key: `productId` or `path.productId`.
pub(crate) fn link_param<'a>(op: &'a Operation, key: &str) -> Option<&'a Param> {
    let (loc, wire) = match key.split_once('.') {
        Some((l @ ("path" | "query" | "header" | "cookie"), w)) => (Some(l), w),
        _ => (None, key),
    };
    op.params.iter().find(|p| {
        p.wire == wire
            && loc.is_none_or(|l| {
                matches!(
                    (l, p.location),
                    ("path", Location::Path)
                        | ("query", Location::Query)
                        | ("header", Location::Header)
                        | ("cookie", Location::Cookie)
                )
            })
    })
}

/// Which input a link value reads.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LinkInput {
    Constant,
    Request,
    Body,
    /// `runtime::Exchange`: `$url`, `$method`, `$statusCode`, headers and bodies.
    Exchange,
}

/// Returns a link parameter or `requestBody` value as an `Option<serde_json::Value>`
/// expression and the inputs that it reads. Strings may embed runtime
/// expressions in `{}` (`/orders/{$response.body#/id}`). A string that starts
/// with `$` is one expression.
pub(crate) fn link_source(op: &Operation, expr: &serde_json::Value) -> (TokenStream, BTreeSet<LinkInput>) {
    let Some(text) = expr.as_str() else {
        let json = json_tokens(expr);
        return (quote!(Some(#json)), BTreeSet::from([LinkInput::Constant]));
    };
    if text.starts_with('$') {
        let (src, input) = expression_source(op, text, true);
        return (src, BTreeSet::from([input]));
    }
    template_source(op, text, true)
}

/// Returns `text` as a runtime expression template. `{$...}` parts are evaluated and the
/// rest is literal. `body`: whether a response body `body` is in scope.
pub(crate) fn template_source(op: &Operation, text: &str, body: bool) -> (TokenStream, BTreeSet<LinkInput>) {
    let (literals, exprs) = split_template(text);
    if exprs.is_empty() {
        let s = lit(text);
        return (quote!(Some(serde_json::Value::String(#s.to_owned()))), BTreeSet::from([LinkInput::Constant]));
    }
    let mut inputs = BTreeSet::new();
    let values: Vec<TokenStream> = exprs
        .iter()
        .map(|e| {
            let (src, input) = expression_source(op, e, body);
            inputs.insert(input);
            src
        })
        .collect();
    let literals = literals.iter().map(|l| lit(l));
    (quote!(runtime::template(&[#(#literals),*], vec![#(#values),*])), inputs)
}

/// Splits a runtime expression template into literals and the expressions
/// between them (`literals.len() == exprs.len() + 1`). Only `{$...}` is an
/// expression: other braces stay literal.
pub(crate) fn split_template(text: &str) -> (Vec<String>, Vec<String>) {
    let (mut literals, mut exprs) = (vec![], vec![]);
    let mut rest = text;
    let mut lit_acc = String::new();
    while let Some((before, after)) = rest.split_once("{$") {
        let Some((expr, tail)) = after.split_once('}') else { break };
        lit_acc.push_str(before);
        literals.push(std::mem::take(&mut lit_acc));
        exprs.push(format!("${expr}"));
        rest = tail;
    }
    lit_acc.push_str(rest);
    literals.push(lit_acc);
    (literals, exprs)
}

/// Returns one runtime expression. `$request.{path,query,header,cookie}.x` comes from the
/// `Params` of the operation. `$response.body` comes from `body` (when in scope).
/// Everything else comes from the `runtime::Exchange`.
pub(crate) fn expression_source(op: &Operation, text: &str, body: bool) -> (TokenStream, LinkInput) {
    if body && let Some(rest) = text.strip_prefix("$response.body") {
        let ptr = lit(rest.strip_prefix('#').unwrap_or(""));
        return (quote!(body.pointer(#ptr).cloned()), LinkInput::Body);
    }
    if let Some(key) = text.strip_prefix("$request.")
        && !key.starts_with("body")
        && let Some(p) = link_param(op, key)
    {
        let f = snake_ident(&p.name);
        return (
            quote!(serde::Serialize::serialize(&request.#f, serde_json::value::Serializer).ok()),
            LinkInput::Request,
        );
    }
    let e = lit(text);
    (quote!(exchange.eval(#e)), LinkInput::Exchange)
}

/// Fields of one link struct filled from runtime expressions.
#[derive(Default)]
pub(crate) struct LinkFields<'a> {
    pub(crate) fields: Vec<TokenStream>,
    pub(crate) inits: Vec<TokenStream>,
    pub(crate) inputs: BTreeSet<LinkInput>,
    pub(crate) tys: Vec<&'a Ty>,
    pub(crate) uses_types: bool,
    pub(crate) uses_runtime: bool,
}

impl<'a> LinkFields<'a> {
    /// Adds `pub <field>: Option<<inner>>` read from `expr`.
    fn add(&mut self, space: &Space<'_>, op: &Operation, field: &str, inner: &'a Ty, expr: &serde_json::Value) {
        self.tys.push(inner);
        let (named, binary) = mentions(inner);
        self.uses_types |= named;
        self.uses_runtime |= binary;
        let (f, t) = (ident(field), ty(space, inner, true));
        self.fields.push(quote!(pub #f: Option<#t>,));
        // A string that holds a number or bool (`'0'`) also fills a typed parameter.
        let (src, inputs) = link_source(op, expr);
        self.inputs.extend(inputs);
        self.uses_runtime = true;
        self.inits.push(quote!(#f: (#src).and_then(runtime::link_value),));
    }
}

/// Returns one link struct: fields for the target's parameters and body.
pub(crate) fn link_struct<'a>(
    space: &Space<'_>,
    op: &Operation,
    l: &'a Link,
    target: &'a Operation,
    name: &proc_macro2::Ident,
    f: &mut LinkFields<'a>,
) -> TokenStream {
    for (key, expr) in &l.params {
        let Some(p) = link_param(target, key) else { continue };
        let inner = if let Ty::Option(t) = &p.ty { t.as_ref() } else { &p.ty };
        f.add(space, op, &snake(&p.name), inner, expr);
    }
    // `requestBody`: typed after the linked operation's single-media body.
    if let (Some(expr), Some(b)) = (&l.request_body, target.body.as_ref().filter(|b| b.contents.len() == 1)) {
        f.add(space, op, "body", &b.contents[0].ty, expr);
    }
    let doc = doc_opt(l.doc.as_deref());
    let request = ident(if f.inputs.contains(&LinkInput::Request) { "request" } else { "_request" });
    let body = ident(if f.inputs.contains(&LinkInput::Body) { "body" } else { "_body" });
    let eq = eq_derive(space, &f.tys);
    let (fields, inits) = (&f.fields, &f.inits);
    let server = l.server.as_deref().map(|url| {
        let url = lit(url);
        quote! {
            pub const SERVER: &'static str = #url;
        }
    });
    // Expressions that only the whole exchange answers (`$url`, `$statusCode`,
    // headers, `$request.body`) need `from_exchange`. `new` leaves them `None`.
    let ctors = if f.inputs.contains(&LinkInput::Exchange) {
        quote! {
            #[must_use]
            pub fn new(request: &super::Params, body: &serde_json::Value) -> Self {
                Self::from_exchange(request, body, &runtime::Exchange::default())
            }
            #[must_use]
            pub fn from_exchange(#request: &super::Params, #body: &serde_json::Value, exchange: &runtime::Exchange<'_>) -> Self {
                Self { #(#inits)* }
            }
        }
    } else {
        quote! {
            #[must_use]
            pub fn new(#request: &super::Params, #body: &serde_json::Value) -> Self {
                Self { #(#inits)* }
            }
        }
    };
    quote! {
        #doc
        #[derive(Debug, Clone, Default, PartialEq, #eq)]
        pub struct #name {
            #(#fields)*
        }
        impl #name {
            #server
            #ctors
        }
    }
}

/// Returns `pub mod links`: one struct per response link typed after the parameters of the linked operation.
pub(crate) fn links_mod(api: &Api, space: &Space<'_>, op: &Operation) -> Option<TokenStream> {
    let mut taken = BTreeSet::new();
    let mut structs = vec![];
    let (mut uses_types, mut uses_runtime) = (false, false);
    for r in &op.responses {
        for l in &r.links {
            let Some(target) = link_target(api, &l.target) else { continue };
            let name = format_ident!("{}", pascal(&crate::ir::names::unique(Name::new(&l.name), &mut taken)));
            let mut f = LinkFields::default();
            structs.push(link_struct(space, op, l, target, &name, &mut f));
            uses_types |= f.uses_types;
            uses_runtime |= f.uses_runtime;
        }
    }
    let types_import = uses_types.then(|| {
        quote!(
            use super::super::super::types;
        )
    });
    let runtime_import = uses_runtime.then(|| {
        quote!(
            use super::super::super::runtime;
        )
    });
    (!structs.is_empty()).then(|| {
        quote! {
            pub mod links {
                #types_import
                #runtime_import
                #(#structs)*
            }
        }
    })
}

/// Returns `pub mod callbacks`: per callback of `op` a function that evaluates its URL
/// expression (`{$request.body#/callbackUrl}`) against the request that `op`
/// received. Pass the result to the `WebhookClient` method that sends it.
pub(crate) fn callbacks_mod(api: &Api, op: &Operation) -> Option<TokenStream> {
    let mut seen = BTreeSet::new();
    let mut taken = BTreeSet::new();
    let mut fns = vec![];
    for cb in &api.http.operations {
        let OpKind::Callback { parent, name, expression } = &cb.kind else { continue };
        if *parent != op.name || !seen.insert((name.clone(), expression.clone())) {
            continue;
        }
        let f = format_ident!("{}", snake(&crate::ir::names::unique(Name::new(&format!("{name}_url")), &mut taken)));
        let (src, inputs) = template_source(op, expression, false);
        let request = ident(if inputs.contains(&LinkInput::Request) { "request" } else { "_request" });
        let exchange = ident(if inputs.contains(&LinkInput::Exchange) { "exchange" } else { "_exchange" });
        fns.push(quote! {
            #[must_use]
            pub fn #f(#request: &super::Params, #exchange: &runtime::Exchange<'_>) -> Option<String> {
                #src.map(runtime::value_text)
            }
        });
    }
    (!fns.is_empty()).then(|| {
        quote! {
            pub mod callbacks {
                use super::super::super::runtime;
                #(#fns)*
            }
        }
    })
}

/// Returns `Params::new(required...)`: the one constructor that every operation gets. Path
/// parameters come first in URL-template order. Reordering them changes the
/// URL. So arguments cannot silently swap. Then comes every other parameter that
/// has neither `Option` nor a usable spec `default`. Then a required body.
/// Everything else starts at its default. With no arguments `Default` calls it.
pub(crate) fn params_new(space: &Space<'_>, op: &Operation) -> TokenStream {
    let is_arg = |p: &Param| is_required_param(space, p);
    let args = new_args(space, op);
    let body = op.body.as_ref().filter(|b| b.required).map(|b| body_ty(space, &b.contents, "Body"));
    let count = args.len() + usize::from(body.is_some());
    let arg_list = args.iter().map(|p| {
        let (n, t) = (snake_ident(&p.name), ty(space, &p.ty, true));
        quote!(#n: #t)
    });
    let body_arg = body.as_ref().map(|t| quote!(body: #t));
    let inits = op.params.iter().map(|p| {
        let n = snake_ident(&p.name);
        if is_arg(p) {
            return quote!(#n,);
        }
        param_init(space, p, &n)
    });
    let body_init = op.body.as_ref().map(|b| if b.required { quote!(body,) } else { quote!(body: None,) });
    let part_types_init = crate::emit::rust::http::shared::part_types_init(op);
    let media_init = crate::emit::rust::http::shared::media_init(op);
    let all_args = arg_list.chain(body_arg);
    // Inside the body: `clippy::allow_attributes` leaves an inner `#![allow]` alone, and an `#[expect]` would fail a
    // crate that raises `too-many-arguments-threshold`.
    let many = (count > crate::emit::rust::syntax::MAX_ARGUMENTS)
        .then(|| quote!(#![allow(clippy::too_many_arguments, reason = "one argument per required parameter")]));
    // Every other field starts as `None`: nothing to evaluate at run time.
    let constness = op.params.iter().filter(|p| !is_arg(p)).all(|p| const_default(space, p)).then(|| quote!(const));
    let default = (count == 0).then(|| {
        quote! {
            impl Default for Params {
                fn default() -> Self { Self::new() }
            }
        }
    });
    quote! {
        impl Params {
            #[must_use]
            pub #constness fn new(#(#all_args),*) -> Self {
                #many
                Self { #(#inits)* #body_init #part_types_init #media_init }
            }
        }
        #default
    }
}

/// Returns the arguments of `Params::new` in order: path parameters by their place in the URL, then every other
/// parameter that has neither `Option` nor a usable spec `default`.
fn new_args<'a>(space: &Space<'_>, op: &'a Operation) -> Vec<&'a Param> {
    let mut args: Vec<&Param> = op.params.iter().filter(|p| is_required_param(space, p)).collect();
    // A stable sort. Path parameters go by URL position and the rest in declaration order after them.
    args.sort_by_key(|p| {
        if p.location == Location::Path { op.path.find(&format!("{{{}}}", p.wire)).unwrap_or(0) } else { usize::MAX }
    });
    args
}

/// Is `p` an argument of `Params::new` and a required value of its builder?
fn is_required_param(space: &Space<'_>, p: &Param) -> bool {
    p.location == Location::Path || (!p.ty.is_option() && spec_default(space, p).is_none())
}

/// Can `Params::new` take the required values by position? Only when no two of them have the same type: then a swap is
/// a type error. A required value of a checked type is made with its own `try_from` first.
fn params_new_is_safe(space: &Space<'_>, op: &Operation) -> bool {
    let mut types: Vec<String> = new_args(space, op).iter().map(|p| ty(space, &p.ty, true).to_string()).collect();
    if let Some(b) = op.body.as_ref().filter(|b| b.required) {
        types.push(body_ty(space, &b.contents, "Body").to_string());
    }
    let count = types.len();
    types.sort();
    types.dedup();
    types.len() == count
}

/// Returns `builders::Params`: each parameter by name through `runtime::typestate_builder!`, with `Params::builder()`.
/// An operation without parameters or body has none.
pub(crate) fn params_builder(space: &Space<'_>, op: &Operation) -> Option<TokenStream> {
    if op.params.is_empty() && op.body.is_none() {
        return None;
    }
    let setter = |name: &Name| {
        let mut setter = snake(name);
        if setter == "build" || setter == "new" || setter == "builder" {
            setter.push('_');
        }
        ident(&setter)
    };
    let in_builders = |t: TokenStream| if t.to_string() == "Body" { quote!(super::Body) } else { t };
    let mut required: Vec<TokenStream> = Vec::new();
    for (i, p) in new_args(space, op).into_iter().enumerate() {
        let (n, g, t, w) = (snake_ident(&p.name), format_ident!("R{i}"), ty(space, &p.ty, true), lit(&p.wire));
        required.push(quote!(#n: #g: #t = #w));
    }
    let body = op.body.as_ref().map(|b| (b.required, in_builders(body_ty(space, &b.contents, "Body"))));
    if let Some((true, t)) = &body {
        let g = format_ident!("R{}", required.len());
        required.push(quote!(body: #g: #t = "body"));
    }
    let mut optional: Vec<TokenStream> = Vec::new();
    for p in op.params.iter().filter(|p| !is_required_param(space, p)) {
        let (n, set, w) = (snake_ident(&p.name), setter(&p.name), lit(&p.wire));
        let (t, wrap, start) = if p.ty.is_option() {
            (ty(space, p.ty.non_null(), true), quote!(some), quote!(None))
        } else {
            (ty(space, &p.ty, true), quote!(value), crate::emit::rust::http::shared::param_start(space, p))
        };
        optional.push(quote!(#n (#set): #t = #w => #wrap, #start));
    }
    if let Some((false, t)) = &body {
        optional.push(quote!(body (body): #t = "body" => some, None));
    }
    let mut fixed: Vec<TokenStream> = Vec::new();
    if crate::emit::rust::http::shared::chooses_part_types(op) {
        fixed.push(quote!(part_types: runtime::PartTypes::new()));
    }
    if crate::emit::rust::http::shared::chooses_media(op) {
        fixed.push(quote!(media: super::Media::ALL[0]));
    }
    let text = quote!(#(#required)* #(#optional)* #(#fixed)*).to_string();
    let types_import = crate::emit::rust::http::shared::types_import(text.contains("types ::"));
    Some(quote! {
        pub mod builders {
            #types_import
            use super::runtime;
            runtime::typestate_builder! {
                Params => super::Params;
                required { #(#required),* }
                optional { #(#optional),* }
                fixed { #(#fixed),* }
            }
        }
    })
}

/// Returns whether `Params::new` can set the optional parameter `p` in a `const fn`? Yes for `None` or a spec default
/// that is a number, a boolean or a listed enum value. A string default is built at run time (`to_owned`).
fn const_default(space: &Space<'_>, p: &Param) -> bool {
    let Some(d) = &p.default else { return p.ty.is_option() };
    let Ty::Named(id) = p.ty.non_null() else {
        return matches!(
            p.ty.non_null(),
            Ty::Bool | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::F32 | Ty::F64
        );
    };
    match &space.types.get(*id).kind {
        Kind::StrEnum(values) => values.iter().any(|v| d.as_str() == Some(v.wire.as_str())),
        Kind::IntEnum(values) => values.iter().any(|(_, v)| d.as_i64() == Some(*v)),
        // A number default is the tuple of its literal. A string one is built at run time.
        Kind::Checked(c) => c.base != Ty::String && c.base != Ty::Decimal,
        Kind::Struct(_) | Kind::Alias(_) | Kind::Union(_) | Kind::Newtype(_) | Kind::Const(_) | Kind::Pending => false,
    }
}

/// Returns `METHOD`, `PATH` and `Params`.
pub(crate) fn params_struct(space: &Space<'_>, op: &Operation, conveniences: bool) -> TokenStream {
    let (method, path) = (lit(&op.method), lit(&op.path));
    let servers = (!op.servers.is_empty()).then(|| {
        let urls = op.servers.iter().map(server_url);
        quote! {
            pub const SERVERS: &[runtime::ServerUrl] = &[#(#urls),*];
        }
    });
    let server_enum = (op.servers.len() > 1).then(|| {
        let names: Vec<_> =
            op.servers.iter().map(|s| pascal_ident(&Name::new(s.name.as_deref().unwrap_or_default()))).collect();
        let docs = op.servers.iter().map(|s| doc_opt(s.doc.as_deref()));
        let urls = op.servers.iter().map(server_url);
        quote! {
            /// The server that a call goes to, by its name in the description.
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub enum Server { #(#docs #names,)* }
            impl Server {
                #[must_use]
                pub const fn url(self) -> runtime::ServerUrl {
                    match self { #(Self::#names => #urls,)* }
                }
            }
        }
    });
    let mut tys: Vec<&Ty> = op.params.iter().map(|p| &p.ty).collect();
    if let Some(body) = &op.body {
        tys.extend(body.contents.iter().map(|c| &c.ty));
    }
    let eq = eq_derive(space, &tys);
    let fields = op.params.iter().map(|p| {
        let doc = doc_opt(p.doc.as_deref());
        let dep = p.deprecated.then(|| quote!(#[deprecated]));
        let (name, t) = (snake_ident(&p.name), ty(space, &p.ty, true));
        quote!(#doc #dep pub #name: #t,)
    });
    let body_field = op.body.as_ref().map(|body| {
        let doc = doc_opt(with_media_doc(body.doc.clone(), &body.contents).as_deref());
        let t = body_ty(space, &body.contents, "Body");
        let t = if body.required { t } else { quote!(Option<#t>) };
        quote!(#doc pub body: #t,)
    });
    let part_types_field = crate::emit::rust::http::shared::part_types_field(op);
    let (media_field, media_enum) =
        (crate::emit::rust::http::shared::media_field(op), crate::emit::rust::http::shared::media_enum(op));
    let new = conveniences.then(|| params_new(space, op)).filter(|_| params_new_is_safe(space, op));
    let builder = conveniences.then(|| params_builder(space, op)).flatten();
    let consts = conveniences.then(|| quote!(pub const METHOD: &str = #method; pub const PATH: &str = #path;));
    quote! {
        #consts
        #servers
        #server_enum
        #[derive(Debug, Clone, PartialEq, #eq)]
        pub struct Params {
            #(#fields)*
            #body_field
            #part_types_field
            #media_field
        }
        #media_enum
        #new
        #builder
    }
}

/// Returns `doc` and then the description of the only media type of a body. With several
/// each content enum variant carries its own.
pub(crate) fn with_media_doc(doc: Option<String>, contents: &[Content]) -> Option<String> {
    let [c] = contents else { return doc };
    let Some(media) = c.doc.as_deref().map(str::trim).filter(|d| !d.is_empty()) else { return doc };
    Some(match doc {
        Some(d) => format!("{d}\n\n{media}"),
        None => media.to_owned(),
    })
}

/// Emits the `Response` enum of one operation and its per-status content enums.
pub(crate) fn response_enum(items: &mut Vec<TokenStream>, space: &Space<'_>, op: &Operation, bx: &Boxing) {
    let mut rtys: Vec<&Ty> = vec![];
    for r in &op.responses {
        rtys.extend(r.contents.iter().map(|c| &c.ty));
        rtys.extend(r.headers.iter().map(|h| &h.ty));
    }
    let eq = eq_derive(space, &rtys);
    let variants = op.responses.iter().enumerate().map(|(i, r)| {
        let doc = doc_opt(with_media_doc(r.doc.clone(), &r.contents).as_deref());
        let v = pascal_ident(&r.variant);
        let body = || {
            boxed(
                bx.resp[i],
                crate::emit::rust::http::shared::resp_body_ty(space, r, &format!("{}Body", pascal(&r.variant))),
            )
        };
        match variant_shape(r) {
            VariantShape::Unit => quote!(#doc #v,),
            VariantShape::Tuple => {
                let t = body();
                quote!(#doc #v(#t),)
            }
            VariantShape::Struct => {
                let status = status_code(r).is_none().then(|| quote!(status: runtime::http::StatusCode,));
                let body_field = (!r.contents.is_empty()).then(|| {
                    let t = body();
                    quote!(body: #t,)
                });
                let headers = r.headers.iter().map(|h| {
                    let (n, t) = (snake_ident(&h.name), ty(space, &h.ty, true));
                    let d = doc_opt(h.doc.as_deref());
                    let dep = h.deprecated.then(|| quote!(#[deprecated]));
                    quote!(#d #dep #n: #t,)
                });
                quote!(#doc #v { #status #body_field #(#headers)* },)
            }
        }
    });
    items.push(quote! {
        #[derive(Debug, Clone, PartialEq, #eq)]
        pub enum Response {
            #(#variants)*
        }
    });
    items.push(response_error(op));
    items.extend(into_ok(space, op, bx));
    for (i, r) in op.responses.iter().enumerate() {
        if r.contents.len() < 2 || crate::emit::rust::http::shared::collapses(space, r) {
            continue;
        }
        let name = format_ident!("{}Body", pascal(&r.variant));
        if let Some(shared) = &r.shared {
            // The body of the response is declared once in `responses`.
            let shared = format_ident!("{}", off_prelude(shared, "Response"));
            items.push(quote!(pub type #name = super::responses::#shared;));
            continue;
        }
        items.push(content_enum(space, &name.to_string(), &r.contents, &bx.resp_enums[i]));
    }
}

/// Is `r` a success: a `2xx` code or the `2XX` range? `default` is not: it stands for every status not listed.
fn is_success(r: &Resp) -> bool {
    matches!(r.status, Status::Code(c) if (SUCCESS_FROM..SUCCESS_TO).contains(&c)) || r.status == Status::Range(2)
}

/// The first status code that is a success.
const SUCCESS_FROM: u16 = 200;
/// The first status code past the successes.
const SUCCESS_TO: u16 = 300;

/// Returns the pattern that matches the variant of `r` whatever it holds.
fn any_of(r: &Resp) -> TokenStream {
    let variant = pascal_ident(&r.variant);
    match variant_shape(r) {
        VariantShape::Unit => quote!(Self::#variant),
        VariantShape::Tuple => quote!(Self::#variant(..)),
        VariantShape::Struct => quote!(Self::#variant { .. }),
    }
}

/// Returns one pattern that matches any of `patterns`: in parentheses when there are several, as `@` needs.
fn either(patterns: &[TokenStream]) -> TokenStream {
    if let [one] = patterns { one.clone() } else { quote!((#(#patterns)|*)) }
}

/// Returns `Display` and `Error` for `Response`: its status and variant, so that `?` takes it into any error type.
fn response_error(op: &Operation) -> TokenStream {
    let arms = op.responses.iter().map(|r| {
        let variant = pascal_ident(&r.variant);
        match status_code(r) {
            Some(code) => {
                let (pattern, text) = (any_of(r), lit(&format!("status {code} ({})", pascal(&r.variant))));
                quote!(#pattern => f.write_str(#text),)
            }
            None => {
                let text = lit(&format!("status {{}} ({})", pascal(&r.variant)));
                quote!(Self::#variant { status, .. } => write!(f, #text, status.as_u16()),)
            }
        }
    });
    quote! {
        impl std::fmt::Display for Response {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self { #(#arms)* }
            }
        }
        impl std::error::Error for Response {}
    }
}

/// A response of an operation and its index among the responses.
type Numbered<'a> = (usize, &'a Resp);

/// Returns `into_ok`: the response as `Ok` when it is the success, else itself, boxed, as `Err`. One success gives what
/// it carries: nothing, its body, or its body and headers in order. Several successes, or the `2XX` range, give the
/// response itself. The `Err` is boxed: only a failure pays for it, and a large response would make every `Result` as
/// large.
fn into_ok(space: &Space<'_>, op: &Operation, bx: &Boxing) -> Option<TokenStream> {
    let (successes, failures): (Vec<Numbered<'_>>, Vec<Numbered<'_>>) =
        op.responses.iter().enumerate().partition(|(_, r)| is_success(r));
    let failed = (!failures.is_empty()).then(|| {
        let patterns = either(&failures.iter().map(|(_, r)| any_of(r)).collect::<Vec<_>>());
        quote!(other @ #patterns => Err(Box::new(other)),)
    });
    let doc = " Returns the success as `Ok` and any other response as `Err`. `?` takes the `Err` into any error type.";
    let (success, arm, alias) = match successes[..] {
        [] => return None,
        [(index, resp)] => one_success(space, resp, bx.resp[index]),
        _ => {
            let patterns = either(&successes.iter().map(|(_, r)| any_of(r)).collect::<Vec<_>>());
            (quote!(Self), quote!(success @ #patterns => Ok(success),), None)
        }
    };
    Some(quote! {
        #alias
        impl Response {
            #[doc = #doc]
            pub fn into_ok(self) -> Result<#success, Box<Self>> {
                match self { #arm #failed }
            }
        }
    })
}

/// Returns the type that the one success `resp` gives from `into_ok`, the match arm that takes it apart and the alias
/// that stands for a tuple: nothing, its body, its one header, or its body and headers as `Success` in order.
fn one_success(space: &Space<'_>, resp: &Resp, boxed_body: bool) -> (TokenStream, TokenStream, Option<TokenStream>) {
    let variant = pascal_ident(&resp.variant);
    let body = || {
        let name = format!("{}Body", pascal(&resp.variant));
        boxed(boxed_body, crate::emit::rust::http::shared::resp_body_ty(space, resp, &name))
    };
    match variant_shape(resp) {
        VariantShape::Unit => (quote!(()), quote!(Self::#variant => Ok(()),), None),
        VariantShape::Tuple => (body(), quote!(Self::#variant(body) => Ok(body),), None),
        VariantShape::Struct => {
            let mut names = vec![];
            let mut types = vec![];
            if !resp.contents.is_empty() {
                names.push(format_ident!("body"));
                types.push(body());
            }
            for header in &resp.headers {
                names.push(snake_ident(&header.name));
                types.push(ty(space, &header.ty, true));
            }
            if let ([name], [single]) = (&names[..], &types[..]) {
                return (single.clone(), quote!(Self::#variant { #name, .. } => Ok(#name),), None);
            }
            let alias = quote! {
                /// The body and headers of the success, in order: what `Response::into_ok` gives.
                pub type Success = (#(#types),*);
            };
            (quote!(Success), quote!(Self::#variant { #(#names),* } => Ok((#(#names),*)),), Some(alias))
        }
    }
}

pub(crate) fn content_enum(space: &Space<'_>, name: &str, contents: &[Content], plan: &[bool]) -> TokenStream {
    let tys: Vec<&Ty> = contents.iter().map(|c| &c.ty).collect();
    let eq = eq_derive(space, &tys);
    let n = ident(name);
    let variants = contents.iter().zip(plan).map(|(c, b)| {
        let d = doc_opt(c.doc.as_deref());
        let (v, t) = (pascal_ident(&c.variant), boxed(*b, content_ty(space, c)));
        quote!(#d #v(#t),)
    });
    quote! {
        #[derive(Debug, Clone, PartialEq, #eq)]
        pub enum #n { #(#variants)* }
    }
}
