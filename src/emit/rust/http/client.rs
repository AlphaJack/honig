//! `client.rs`: the client over a `Transport`.

mod auth;

pub(crate) use auth::*;

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use super::codec::{Decode, Held};
use super::codec::{Generality, decode_body, decode_contents, encode_contents, media_rank, needs_ct};
use super::paths::{path_param, template, value_expr};
use super::security::Securities;
use super::shared::{Boxing, VariantShape};
use super::shared::{
    blob, boxing, code, is_path, is_success, op_path, param_enc, server_url, shape, status_code, user_types,
    variant_shape, wrap,
};
use crate::emit::rust::names::{pascal, snake};
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{doc_opt, ident, lit, mentions, ty};
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::Api;
use crate::ir::http::{Location, Operation, Param, Resp, Status};
use crate::ir::media::MediaKind;
use crate::ir::types::{JsonKind, Ty};

/// Returns the descriptions of the servers as a numbered list. The number is the index into `SERVERS`.
fn servers_doc(api: &Api) -> TokenStream {
    let text: Vec<String> = api
        .servers
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.doc.as_deref().map(|d| format!("{i}. {}", d.trim())))
        .collect();
    doc_opt(Some(&text.join("\n")))
}

/// Returns `client.rs` for the operations to call (`client_operations`: `operationId` or generated name, all without
/// it). A binary calls part of the client.
pub(crate) fn client(api: &Api, space: &Space<'_>, opts: &crate::emit::Options) -> TokenStream {
    let (only, binary) = (opts.client_operations.as_deref(), opts.rust.binary);
    let called = |op: &&Operation| is_path(op) && only.is_none_or(|names| super::shared::listed(names, op));
    let servers = api.servers.iter().map(server_url);
    let sec = Securities::new(api);
    let ops = api.http.operations.iter().filter(called).map(|op| client_op(space, api, &sec, op, false));
    let setters = credential_setters(api);
    let servers_doc = servers_doc(api);
    let tokens = token_methods(api);
    let devices = device_methods(api);
    let (flows, flow_urls) = oauth_methods(api);
    let webhook_client = webhook_client(space, api, &sec);
    let header = binary.then(|| quote!(#![allow(dead_code, reason = "a binary calls part of the client: its server list and setters may go unused")]));
    // The operations read credentials. The setters and flows write them. Without any operation none are kept.
    let keeps = api.http.operations.iter().any(|op| called(&op))
        || [&setters, &tokens, &devices, &flows].iter().any(|methods| !methods.is_empty());
    let (credentials, no_credentials) = if keeps {
        (
            quote! {
                pub(super) credentials: runtime::Credentials,
            },
            quote!(credentials: runtime::Credentials::new()),
        )
    } else {
        (TokenStream::new(), TokenStream::new())
    };
    // `<op>_stream` methods spell out their item type.
    let uses_types =
        api.http.operations.iter().filter(called).filter_map(stream_target).any(|(_, item, _)| mentions(item).0);
    let types_import = crate::emit::rust::http::shared::types_import(uses_types);
    quote! {
        #header
        use super::{operations as ops, runtime};
        #types_import

        #servers_doc
        pub const SERVERS: &[runtime::ServerUrl] = &[#(#servers),*];

        #[derive(Debug, Clone)]
        pub struct Client<T> {
            pub transport: T,
            #credentials
        }

        impl<T> Client<T> {
            #(#flow_urls)*

            pub const fn new(transport: T) -> Self {
                Self { transport, #no_credentials }
            }

            #(#setters)*
        }

        impl<T: runtime::Transport> Client<T> {
            #(#tokens)*
            #(#devices)*
            #(#flows)*
            #(#ops)*
        }

        #webhook_client
    }
}

/// Emits `WebhookClient`, which sends webhooks and callbacks if the API declares any.
pub(crate) fn webhook_client(space: &Space<'_>, api: &Api, sec: &Securities) -> Option<TokenStream> {
    let hooks: Vec<TokenStream> =
        api.http.operations.iter().filter(|o| !is_path(o)).map(|op| client_op(space, api, sec, op, true)).collect();
    let setters = credential_setters(api);
    (!hooks.is_empty()).then(|| {
        quote! {
            #[derive(Debug, Clone)]
            pub struct WebhookClient<T> {
                pub transport: T,
                pub(super) credentials: runtime::Credentials,
            }

            impl<T> WebhookClient<T> {
                pub const fn new(transport: T) -> Self {
                    Self { transport, credentials: runtime::Credentials::new() }
                }

                #(#setters)*
            }

            impl<T: runtime::Transport> WebhookClient<T> {
                #(#hooks)*
            }
        }
    })
}

/// Returns the path statements of the client (`path` ends up holding the request target).
/// For webhooks and callbacks also the `url` argument.
/// Returns `server: <op>::Server,`: the argument that names one of several servers of an operation.
fn server_arg(op: &Operation, m: &TokenStream) -> Option<TokenStream> {
    (op.servers.len() > 1).then(|| quote!(server: #m::Server,))
}

pub(crate) fn client_path(op: &Operation, m: &TokenStream, hook: bool) -> (Option<TokenStream>, TokenStream) {
    let mut pieces = template(&op.path).into_iter().peekable();
    // A leading literal starts the string and not an
    // empty `String` plus `push_str`.
    let start = match pieces.next_if(Result::is_ok) {
        Some(Ok(l)) => {
            let l = lit(&l);
            quote!(String::from(#l))
        }
        _ => quote!(String::new()),
    };
    let path: Vec<TokenStream> = pieces
        .map(|piece| match piece {
            Ok(l) if l.chars().count() == 1 => {
                let c = Literal::character(l.chars().next().unwrap_or('/'));
                quote!(path.push(#c);)
            }
            Ok(l) => {
                let l = lit(&l);
                quote!(path.push_str(#l);)
            }
            Err(name) => match path_param(op, &name) {
                Some(p) => {
                    let (wire, v, e) = (lit(&p.wire), value_expr(p), param_enc(p));
                    quote!(path.push_str(&runtime::encode_path(#wire, &#v, #e)?);)
                }
                None => {
                    let l = lit(&format!("{{{name}}}"));
                    quote!(path.push_str(#l);)
                }
            },
        })
        .collect();
    if hook {
        (Some(quote!(url: &str,)), quote!(let path = url.to_owned();))
    } else {
        // Operation or path-level `servers`: this operation goes there. Of several, the caller names one.
        let init = if path.is_empty() { quote!(let path = #start;) } else { quote!(let mut path = #start;) };
        match op.servers.len() {
            0 => (None, quote!(#init #(#path)*)),
            1 => (None, quote!(#init #(#path)* let path = runtime::operation_path(#m::SERVERS, path);)),
            _ => {
                (server_arg(op, m), quote!(#init #(#path)* let path = runtime::operation_path(&[server.url()], path);))
            }
        }
    }
}

pub(crate) fn client_op(space: &Space<'_>, api: &Api, sec: &Securities, op: &Operation, hook: bool) -> TokenStream {
    let m = op_path(op);
    let (params_ty, response_ty) = user_types(op);
    let name = snake_ident(&op.name);
    let bx = boxing(space, op);
    let doc = doc_opt(op.doc.as_deref());
    let deprecated = op.deprecated.then(|| quote!(#[deprecated]));
    let chooses = crate::emit::rust::http::shared::chooses_media(op);
    let p = ident(if op.params.is_empty() && op.body.is_none() && !chooses { "_p" } else { "p" });
    let security = sec.of(api, op);
    let (url_arg, path) = client_path(op, &m, hook);
    let query_decl = if op.params.iter().any(|p| matches!(p.location, Location::Query | Location::QueryString)) {
        quote!(let mut query: Vec<String> = Vec::new();)
    } else {
        quote!(let query = Vec::new();)
    };
    let query = op.params.iter().filter_map(|p| match p.location {
        Location::Query => Some(query_param(space, p)),
        Location::QueryString => {
            let (v, form) = (value_expr(p), p.content == Some(MediaKind::Form));
            Some(quote!(runtime::encode_querystring(&mut query, &#v, #form);))
        }
        Location::Path | Location::Header | Location::Cookie | Location::Part => None,
    });
    let method = lit(&op.method);
    let headers_and_body = client_headers_and_body(op, &m, &bx);
    let respond = |skip: &[u16]| client_response(space, op, &m, &bx, skip);
    let response = respond(&[]);
    let accept = if chooses {
        // The success media type asked for, the other success media types at a lower weight and then the own media
        // types of the error responses. A server whose answer depends on the resource (a tile that is PNG or
        // protobuf) then answers one the client decodes and not `406`.
        let produces = crate::emit::rust::http::shared::produces(op);
        let errors: String = error_media(op).iter().map(|m| format!(", {m}")).collect();
        let arms = produces.iter().map(|(chosen, v)| {
            let others: String =
                produces.iter().filter(|(o, _)| o != chosen).map(|(o, _)| format!(", {o};q=0.9")).collect();
            let (v, a) = (pascal_ident(v), lit(&format!("{chosen}{others}{errors}")));
            quote!(#m::Media::#v => #a,)
        });
        Some(quote!(runtime::Message::set_header(&mut req, "Accept", match p.media { #(#arms)* })?;))
    } else {
        accept(op).map(|a| {
            let a = lit(&a);
            quote!(runtime::Message::set_header(&mut req, "Accept", #a)?;)
        })
    };
    let take = take_params(op, &p);
    let prelude = quote! {
        #take
        #path
        #query_decl
        #(#query)*
        let mut req = runtime::new_request(#method, path, query)?;
        #accept
        #headers_and_body
        runtime::apply_auth(&mut req, &self.credentials, #security)?;
        runtime::propagate_trace(&mut req);
    };
    let stream = (!hook).then(|| stream_method(space, op, &p, &prelude, &respond)).flatten();
    quote! {
        #doc
        #deprecated
        pub async fn #name(&self, #url_arg #p: impl runtime::IntoArg<'_, #params_ty>) -> Result<#response_ty, runtime::ClientError> {
            #prelude
            let resp = runtime::send(&self.transport, req).await?;
            #response
        }
        #stream
    }
}

/// Encodes query parameter `p` into `query`. A parameter at its `default` means the same left out. Some servers refuse
/// it anyway (Meilisearch rejects `csvDelimiter` on JSON even at its default `,`), so it stays out.
fn query_param(space: &Space<'_>, p: &Param) -> TokenStream {
    let (wire, v, e) = (lit(&p.wire), value_expr(p), param_enc(p));
    let encode = quote!(runtime::encode_query(&mut query, #wire, &#v, #e)?;);
    crate::emit::rust::http::shared::spec_default(space, p).map_or_else(
        || encode.clone(),
        |d| {
            // An enum default spells out its type, `types::Kind::User`. The client module does not import `types`.
            let d =
                if matches!(space.types.unalias(&p.ty), Ty::Named(_)) { quote!({ use super::types; #d }) } else { d };
            quote! {
                let default = #d;
                if #v != default {
                    #encode
                }
            }
        },
    )
}

/// Returns the `Accept` header. It has every media type that the responses declare once each in the order of the
/// description. A server that can answer in several (the OCS of Nextcloud: XML unless asked) then answers in one
/// that the client decodes. `None` when no response has a body.
/// The media types of the other responses of an operation (errors, redirects) that its success responses do not use.
fn error_media(op: &Operation) -> Vec<&str> {
    let success: Vec<&str> = crate::emit::rust::http::shared::produces(op).into_iter().map(|(m, _)| m).collect();
    let mut out: Vec<&str> = vec![];
    for c in op.responses.iter().flat_map(|r| &r.contents) {
        if !success.contains(&c.media.as_str()) && !out.contains(&c.media.as_str()) {
            out.push(&c.media);
        }
    }
    out
}

fn accept(op: &Operation) -> Option<String> {
    let mut media: Vec<&str> = vec![];
    for c in op.responses.iter().flat_map(|r| &r.contents) {
        if !media.contains(&c.media.as_str()) {
            media.push(&c.media);
        }
    }
    (!media.is_empty()).then(|| media.join(", "))
}

/// (response index, item type and per sequential media type: content index, media, framing).
pub(crate) type StreamTarget<'a> = (usize, &'a Ty, Vec<(usize, &'a str, TokenStream)>);

/// Returns the success response that a `<op>_stream` method reads. That is its index, the item
/// type and per sequential media type (content index, media, framing).
/// Every sequential media type must carry the same item type.
pub(crate) fn stream_target(op: &Operation) -> Option<StreamTarget<'_>> {
    let (ri, r) = op.responses.iter().enumerate().find(|(_, r)| is_success(r))?;
    let mut item: Option<&Ty> = None;
    let mut seq = vec![];
    for (ci, c) in r.contents.iter().enumerate() {
        let framing = match c.kind {
            MediaKind::JsonLines => {
                quote!(runtime::ItemStream::new(status, body, runtime::Framing::JsonLines))
            }
            MediaKind::JsonSeq => quote!(runtime::ItemStream::new(status, body, runtime::Framing::JsonSeq)),
            MediaKind::Sse => quote!(
                runtime::ItemStream::new(status, body, runtime::Framing::Sse)
                    .reconnecting(self.transport.clone(), again)
            ),
            // An `itemSchema` part list (3.2 "Streaming Multipart" and byte ranges). Its boundary is in `media`.
            MediaKind::MultipartMixed if c.positional_prefix.is_none() => {
                quote!(runtime::ItemStream::multipart(status, body, &media))
            }
            MediaKind::Json
            | MediaKind::Linkset
            | MediaKind::Text
            | MediaKind::Bytes
            | MediaKind::Form
            | MediaKind::Multipart
            | MediaKind::MultipartMixed
            | MediaKind::Xml => continue,
        };
        let Ty::Vec(t) = &c.ty else { continue };
        if item.is_some_and(|i| i != t.as_ref()) {
            return None;
        }
        item = Some(t);
        seq.push((ci, c.media.as_str(), framing));
    }
    Some((ri, item?, seq))
}

/// Returns `<op>_stream` for a success response of a sequential media type
/// (`itemSchema`). Items are decoded as they arrive and not as one `Vec`. `respond` decodes a whole response except
/// for the statuses that it is given.
pub(crate) fn stream_method(
    space: &Space<'_>,
    op: &Operation,
    p: &proc_macro2::Ident,
    prelude: &TokenStream,
    respond: &dyn Fn(&[u16]) -> TokenStream,
) -> Option<TokenStream> {
    let Some((ri, item, seq)) = stream_target(op) else { return blob_stream_method(op, p, prelude, respond) };
    let Status::Code(status) = op.responses[ri].status else { return None };
    // Each item has the text of its library formats checked before the library reads it, as on the server.
    let multipart = seq.iter().any(|(_, _, f)| f.to_string().contains("multipart"));
    let validated =
        crate::emit::rust::syntax::holds(space, item).filter(|_| !multipart).map(|h| quote!(.validated::<#h>()));
    // `minItems`, `maxItems` and `uniqueItems` of the sequence hold on the items as they arrive.
    let limits = op.responses[ri].contents.iter().find(|c| c.kind.is_sequence()).map(|c| c.item_limits);
    let validated = match limits.and_then(super::codec::item_limits) {
        Some(l) => Some(quote!(#validated.limited(#l))),
        None => validated,
    };
    let item = ty(space, item, true);
    // The response's `Content-Type` picks the framing; the first one otherwise.
    // Media types sharing a framing share a branch.
    let mut groups: Vec<(String, TokenStream, Vec<Literal>, Generality)> = vec![];
    for (_, media, f) in &seq {
        let key = f.to_string();
        match groups.iter_mut().find(|(k, ..)| *k == key) {
            Some((_, _, ms, rank)) => {
                ms.push(lit(media));
                *rank = (*rank).min(media_rank(media));
            }
            None => groups.push((key, f.clone(), vec![lit(media)], media_rank(media))),
        }
    }
    let fallback = groups.first().map(|(_, f, ..)| f.clone())?;
    // Most specific media type first as in `decode_contents`. The first group
    // is also the fallback. It needs no check of its own when it comes last.
    let mut checked: Vec<_> = groups.iter().enumerate().collect();
    checked.sort_by_key(|(_, g)| g.3);
    if checked.last().is_some_and(|(i, _)| *i == 0) {
        checked.pop();
    }
    // One `else if` chain (clippy `collapsible_else_if`).
    let branches = checked.iter().map(|(_, (_, f, ms, _))| quote!(if #(runtime::media_matches(&ct, #ms))||* { #f }));
    let framing = if checked.is_empty() { fallback } else { quote!(#(#branches)else* else { #fallback }) };
    let framing = match validated {
        Some(v) => quote!((#framing) #v),
        None => framing,
    };
    let ct = (groups.len() > 1).then(|| quote!(let ct = runtime::Message::content_type(&resp).to_owned();));
    let media = multipart.then(|| quote!(let media = runtime::Message::content_type(&resp).to_owned();));
    let all_media = seq.iter().map(|(_, m, _)| lit(m));
    // An event stream reconnects with the request that it sent. It is kept before the send.
    let again = seq
        .iter()
        .any(|(_, _, f)| f.to_string().contains("Sse"))
        .then(|| quote!(let again = runtime::resendable(&req);));
    let m = op_path(op);
    let server = server_arg(op, &m);
    let f = format_ident!("{}_stream", snake(&op.name));
    let code = code(status);
    let doc = doc_opt(op.doc.as_deref());
    let deprecated = op.deprecated.then(|| quote!(#[deprecated]));
    // An event stream that reconnects keeps a clone of the transport.
    let owned = again.is_some().then(|| quote!(where T: Clone + 'static));
    // The streamed status returned already: the rest of the body decodes every other.
    let other = other_statuses(&m, &respond(&[status]));
    Some(quote! {
        #doc
        #deprecated
        pub async fn #f(&self, #server #p: impl runtime::IntoArg<'_, #m::Params>) -> Result<runtime::Streamed<runtime::ItemStream<#item>, #m::Response>, runtime::ClientError> #owned {
            #prelude
            #again
            let resp = runtime::send_streaming(&self.transport, req).await?;
            let status = resp.status().as_u16();
            if status == #code {
                runtime::check_response_media(status, runtime::Message::content_type(&resp), None, &[#(#all_media),*])?;
                #ct
                #media
                let body = resp.into_body().into_stream();
                return Ok(runtime::Streamed::Ok(#framing));
            }
            #other
        }
    })
}

/// Returns the answer of a `<op>_stream` method to a status that it does not stream. The body is read whole and decoded
/// as the method without `_stream` decodes it. So a declared error arrives typed and an undeclared status is still an
/// error.
fn other_statuses(m: &TokenStream, response: &TokenStream) -> TokenStream {
    quote! {
        let (head, body) = resp.into_parts();
        let resp = runtime::Response::from_parts(head, runtime::Body::new(body.collect().await?));
        let other: Result<#m::Response, runtime::ClientError> = async move { #response }.await;
        Ok(runtime::Streamed::Whole(other?))
    }
}

/// Returns `<op>_stream` for binary success responses. It gives the chunks of the body as they
/// arrive for downloads too large to hold in memory. Every success status whose
/// body is binary streams (a `200` and a `206` for a range).
pub(crate) fn blob_stream_method(
    op: &Operation,
    p: &proc_macro2::Ident,
    prelude: &TokenStream,
    respond: &dyn Fn(&[u16]) -> TokenStream,
) -> Option<TokenStream> {
    let streamed: Vec<(u16, &Resp)> = op
        .responses
        .iter()
        .filter(|r| is_success(r) && !r.contents.is_empty() && r.contents.iter().all(blob))
        .filter_map(|r| match r.status {
            Status::Code(c) => Some((c, r)),
            Status::Range(_) | Status::Default => None,
        })
        .collect();
    let server = server_arg(op, &op_path(op));
    // The first success response decides as before. A JSON `200` next to a binary `206` has no stream method.
    let first = op.responses.iter().find(|r| is_success(r))?;
    if !streamed.iter().any(|(_, r)| std::ptr::eq(*r, first)) {
        return None;
    }
    let (m, f) = (op_path(op), format_ident!("{}_stream", snake(&op.name)));
    let branches = streamed.iter().map(|(c, r)| {
        let (c, media) = (code(*c), r.contents.iter().map(|c| lit(&c.media)));
        quote! {
            if resp.status() == #c {
                runtime::check_response_media(#c, runtime::Message::content_type(&resp), None, &[#(#media),*])?;
                return Ok(runtime::Streamed::Ok(resp));
            }
        }
    });
    let doc = doc_opt(op.doc.as_deref());
    let deprecated = op.deprecated.then(|| quote!(#[deprecated]));
    // The streamed statuses returned already: the rest of the body decodes every other.
    let codes: Vec<u16> = streamed.iter().map(|(c, _)| *c).collect();
    let other = other_statuses(&m, &respond(&codes));
    Some(quote! {
        #doc
        #deprecated
        pub async fn #f(&self, #server #p: impl runtime::IntoArg<'_, #m::Params>) -> Result<runtime::Streamed<runtime::Response, #m::Response>, runtime::ClientError> {
            #prelude
            let resp = runtime::send_streaming(&self.transport, req).await?;
            #(#branches)*
            #other
        }
    })
}

/// Returns whether the client only reads the request body. Then it can encode it from borrowed parameters. Bytes, text
/// and streamed items move into the request. Borrowed parameters are cloned for them as the caller would have done.
fn reads_body(op: &Operation) -> bool {
    op.body.as_ref().is_none_or(|b| {
        b.contents.iter().all(|c| match c.kind {
            MediaKind::Json
            | MediaKind::Linkset
            | MediaKind::Xml
            | MediaKind::Form
            | MediaKind::Multipart
            | MediaKind::MultipartMixed => true,
            MediaKind::JsonLines | MediaKind::JsonSeq | MediaKind::Sse => !c.streams(),
            MediaKind::Text | MediaKind::Bytes => false,
        })
    })
}

/// Returns the parameters as the method body uses them. They are borrowed when it only reads them. Otherwise owned
/// (cloned if lent).
fn take_params(op: &Operation, p: &proc_macro2::Ident) -> Option<TokenStream> {
    (p == "p").then(|| {
        if reads_body(op) {
            quote!(let p = runtime::IntoArg::into_arg(p);)
        } else {
            quote!(let p = runtime::IntoArg::into_arg(p).into_owned();)
        }
    })
}

/// Returns headers, cookies and the request body.
pub(crate) fn client_headers_and_body(op: &Operation, m: &TokenStream, bx: &Boxing) -> TokenStream {
    let headers = op.params.iter().filter(|p| p.location == Location::Header).map(|p| {
        let (v, explode, wire) = (value_expr(p), p.explode, lit(&p.wire));
        quote!(if let Some(h) = runtime::encode_header(&#v, #explode)? { runtime::Message::set_header(&mut req, #wire, h)?; })
    });
    let cookie_params: Vec<&Param> = op.params.iter().filter(|p| p.location == Location::Cookie).collect();
    let cookies = (!cookie_params.is_empty()).then(|| {
        let each = cookie_params.iter().map(|p| {
            let (wire, v, e) = (lit(&p.wire), value_expr(p), param_enc(p));
            quote!(runtime::encode_cookie(&mut cookies, #wire, &#v, #e)?;)
        });
        quote! {
            let mut cookies: Vec<String> = Vec::new();
            #(#each)*
            runtime::set_cookies(&mut req, cookies)?;
        }
    });
    let part_params: Vec<&Param> = op.params.iter().filter(|p| p.location == Location::Part).collect();
    let has_parts = !part_params.is_empty();
    // Part headers are encoded before the body moves out of `p`.
    let parts = has_parts.then(|| {
        let each = part_params.iter().map(|p| {
            let (v, explode, wire) = (value_expr(p), p.explode, lit(&p.wire));
            let part = lit(p.part.as_deref().unwrap_or_default());
            quote!(if let Some(h) = runtime::encode_header(&#v, #explode)? { parts.push((#part, #wire, h)); })
        });
        quote! {
            let mut parts: Vec<(&str, &str, String)> = Vec::new();
            #(#each)*
        }
    });
    let borrowed = reads_body(op);
    let body = op.body.as_ref().map(|b| {
        let enum_path = quote!(#m::Body);
        let target = quote!(req);
        let (plan, contents) = (&bx.body_enum, &b.contents);
        match (b.required, borrowed) {
            (true, true) => encode_contents(&target, &quote!(&p.body), contents, &enum_path, plan, has_parts, true),
            (true, false) => encode_contents(&target, &quote!(p.body), contents, &enum_path, plan, has_parts, false),
            (false, borrowed) => {
                let enc = encode_contents(&target, &quote!(body), contents, &enum_path, plan, has_parts, borrowed);
                let opt = if borrowed { quote!(&p.body) } else { quote!(p.body) };
                quote!(if let Some(body) = #opt { #enc })
            }
        }
    });
    quote! { #(#headers)* #cookies #parts #body }
}

/// Emits the check that a body decodes as a type its status declares: the response must say it is one.
fn media_check(r: &Resp) -> Option<TokenStream> {
    let media = r.contents.iter().map(|c| lit(&c.media));
    (!r.contents.is_empty()).then(|| quote!(runtime::check_media_of(&head, &body, &[#(#media),*])?;))
}

/// Matches the status and decode the documented response. The statuses in `skip` are left out: a `<op>_stream` method
/// returned them already.
pub(crate) fn client_response(
    space: &Space<'_>,
    op: &Operation,
    op_mod: &TokenStream,
    bx: &Boxing,
    skip: &[u16],
) -> TokenStream {
    let guard = not_modified_guard(op);
    let kept: Vec<(usize, &Resp)> = op
        .responses
        .iter()
        .enumerate()
        .filter(|(_, r)| !matches!(r.status, Status::Code(c) if skip.contains(&c)))
        .collect();
    // The response in parts: its head and its bytes. A body that a transport held whole is not copied.
    let parts = quote! {
        let (head, body) = resp.into_parts();
        let (body, _) = body.into_parts();
    };
    let unexpected = quote!(Err(runtime::ClientError::UnexpectedStatus { status: head.status, body }));
    if kept.is_empty() {
        return quote! { #parts #guard #unexpected };
    }
    let ct = kept
        .iter()
        .any(|(_, r)| needs_ct(&r.contents))
        .then(|| quote!(let ct = runtime::content_type_of(&head.headers);));
    let arms: Vec<(&Resp, TokenStream, TokenStream)> = kept
        .iter()
        .map(|&(i, r)| {
            let (pat, e) = response_arm(space, op_mod, bx, i, r);
            (r, pat, e)
        })
        .collect();
    // A lone `default` takes every status: no match.
    if let [(r, _, only)] = arms.as_slice()
        && r.status == Status::Default
    {
        // A response with neither a body nor headers reads only its status, and only as the variant's field.
        if r.contents.is_empty() && r.headers.is_empty() {
            return quote! { let (head, _) = resp.into_parts(); #guard #ct #only };
        }
        return quote! { #parts #guard let status = head.status.as_u16(); #ct #only };
    }
    let dispatch = dispatch(&arms, ct.as_ref(), &unexpected);
    quote! { #parts #guard #dispatch }
}

/// Returns the match arm of response `i` with its status pattern and the expression that decodes it.
fn response_arm(
    space: &Space<'_>,
    op_mod: &TokenStream,
    bx: &Boxing,
    i: usize,
    r: &Resp,
) -> (TokenStream, TokenStream) {
    let pat = match r.status {
        Status::Code(c) => {
            let c = code(c);
            quote!(#c)
        }
        Status::Range(n) => {
            let (lo, hi) = (code(n * 100), code(n * 100 + 100));
            quote!(s if (#lo..#hi).contains(&s))
        }
        Status::Default => quote!(_),
    };
    let variant = pascal_ident(&r.variant);
    let enum_path = {
        let e = format_ident!("{}Body", pascal(&r.variant));
        quote!(#op_mod::#e)
    };
    let decode = Decode {
        status: quote!(status),
        ct: quote!(ct),
        body: quote!(body),
        q: quote!(?),
        held: if r.headers.is_empty() { Held::Moved } else { Held::Borrowed },
        stream: None,
    };
    // A body of one type in every media type decodes into it whichever arrived.
    let collapsed = crate::emit::rust::http::shared::collapses(space, r);
    let plan = if collapsed { vec![false; r.contents.len()] } else { bx.resp_enums[i].clone() };
    let path = (!collapsed).then_some(&enum_path);
    let decoded = || wrap(bx.resp[i], decode_contents(space, &r.contents, &plan, path, &decode));
    let check = media_check(r);
    match variant_shape(r) {
        VariantShape::Unit => (pat, quote!(Ok(#op_mod::Response::#variant))),
        VariantShape::Tuple => {
            // One fallible body that is not boxed maps straight into its variant.
            if let ([c], false) = (r.contents.as_slice(), bx.resp[i])
                && let (e, true) = decode_body(space, c, &decode)
            {
                return (pat, quote!({ #check #e.map(#op_mod::Response::#variant) }));
            }
            let e = decoded();
            (pat, quote!({ #check Ok(#op_mod::Response::#variant(#e)) }))
        }
        VariantShape::Struct => {
            let status = status_code(r).is_none().then(|| quote!(status: head.status,));
            let body = (!r.contents.is_empty()).then(|| {
                let e = decoded();
                quote!(body: #e,)
            });
            let headers = r.headers.iter().map(|h| {
                let (n, wire, s) = (snake_ident(&h.name), lit(&h.wire), shape(space, &h.ty));
                let raw = if h.wire.eq_ignore_ascii_case("set-cookie") {
                    let array = matches!(space.kind_of(h.ty.non_null()), JsonKind::Array);
                    quote!(runtime::header_lines(&head.headers, #wire, #array))
                } else if h.json {
                    quote!(runtime::json_header(runtime::first_header(&head.headers, #wire).as_deref()))
                } else if h.linkset {
                    quote!(runtime::linkset_header(runtime::joined_header(&head.headers, #wire).as_deref()))
                } else {
                    let explode = h.explode;
                    quote!(runtime::decode_header(runtime::joined_header(&head.headers, #wire).as_deref(), #s, #explode))
                };
                let at = lit(&format!("/header/{}", h.wire.replace('~', "~0").replace('/', "~1")));
                // A library type has the text of its format checked first, as in a request.
                let checked = crate::emit::rust::http::codec::validated_map(space, &h.ty);
                quote! {
                    #n: runtime::from_param(#raw, #at) #checked
                        .map_err(|e| runtime::ClientError::Decode { status, message: e.to_string(), body: body.clone(), pointer: String::new() })?,
                }
            });
            (pat, quote!({ #check Ok(#op_mod::Response::#variant { #status #body #(#headers)* }) }))
        }
    }
}

/// Returns a `304` to an operation that sends `If-None-Match` or `If-Modified-Since` but declares no `304`. The client
/// already holds the representation. No declared response describes it (RFC 9110 section 15.4.5).
fn not_modified_guard(op: &Operation) -> Option<TokenStream> {
    let conditional = op.params.iter().any(|p| {
        matches!(p.location, Location::Header)
            && (p.wire.eq_ignore_ascii_case("if-none-match") || p.wire.eq_ignore_ascii_case("if-modified-since"))
    });
    let not_modified = runtime::http::StatusCode::NOT_MODIFIED.as_u16();
    let declared = op.responses.iter().any(|r| {
        r.status == Status::Code(not_modified) || r.status == Status::Range(not_modified / runtime::__private::CLASS)
    });
    (conditional && !declared).then(|| {
        quote!(if head.status == 304 {
            return Err(runtime::ClientError::NotModified { headers: head.headers });
        })
    })
}

/// Returns the status match: an `if` for one status and the rest unexpected (clippy `single_match_else`).
fn dispatch(
    arms: &[(&Resp, TokenStream, TokenStream)],
    ct: Option<&TokenStream>,
    unexpected: &TokenStream,
) -> TokenStream {
    // The one specific status and what every other status gets. That is the arm of `default` or else an error.
    let specific: Vec<&(&Resp, TokenStream, TokenStream)> =
        arms.iter().filter(|(r, ..)| r.status != Status::Default).collect();
    let otherwise = arms.iter().find(|(r, ..)| r.status == Status::Default).map(|(_, _, e)| e);
    let has_default = otherwise.is_some();
    if let [(r, _, only)] = specific.as_slice() {
        let otherwise = otherwise.unwrap_or(unexpected);
        let cond = match r.status {
            Status::Code(c) => {
                let c = code(c);
                Some(quote!(status == #c))
            }
            Status::Range(n) => {
                let (lo, hi) = (code(n * 100), code(n * 100 + 100));
                Some(quote!((#lo..#hi).contains(&status)))
            }
            Status::Default => None,
        };
        if let Some(cond) = cond {
            return quote! { let status = head.status.as_u16(); #ct if #cond { #only } else { #otherwise } };
        }
    }
    let arms = arms.iter().map(|(_, pat, e)| quote!(#pat => #e,));
    let unexpected = (!has_default).then(|| quote!(_ => #unexpected,));
    quote! {
        let status = head.status.as_u16();
        #ct
        match status {
            #(#arms)*
            #unexpected
        }
    }
}
