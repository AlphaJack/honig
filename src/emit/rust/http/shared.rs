//! What the generated operations, client, server and tests share: names, shapes and body plans.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::emit::rust::names::snake;
use crate::emit::rust::space::Space;
use crate::emit::rust::space::WORD;
use crate::emit::rust::syntax::{box_plan, has_float, ident, lit, ty};
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::Api;
use crate::ir::http::{Content, OpKind, Operation, Param, Resp, Status, Style};
use crate::ir::media::MediaKind;
use crate::ir::names::Name;
use crate::ir::types::{JsonKind, Ty};

/// Operations per router dispatch function (keeps each under `too_many_lines`).
pub(crate) const DISPATCH_CHUNK: usize = 25;

/// Returns whether the API serves the operation under `paths` (not a webhook or callback it sends).
pub(crate) fn is_path(op: &Operation) -> bool {
    op.kind == OpKind::Path
}

/// Returns the routing key of a webhook or callback: its name in the description.
pub(crate) fn hook_name(op: &Operation) -> String {
    match &op.kind {
        OpKind::Path => snake(&op.name),
        OpKind::Webhook { name } | OpKind::Callback { name, .. } => name.clone(),
    }
}

/// Does the API declare webhooks or callbacks?
pub(crate) fn has_hooks(api: &Api) -> bool {
    api.http.operations.iter().any(|o| !is_path(o))
}

pub(crate) fn num(n: usize) -> Literal {
    Literal::usize_unsuffixed(n)
}

pub(crate) fn code(c: u16) -> Literal {
    Literal::u16_unsuffixed(c)
}

/// The variant of the generated `Operation` (or `WebhookOperation`) for `op`.
pub(crate) fn operation_variant(op: &Operation) -> proc_macro2::Ident {
    pascal_ident(&op.name)
}

pub(crate) fn op_static(op: &Operation) -> proc_macro2::Ident {
    format_ident!("{}_OP", snake(&op.name).trim_start_matches("r#").to_uppercase())
}

/// Returns `ops::<operation>`.
pub(crate) fn op_path(op: &Operation) -> TokenStream {
    let n = snake_ident(&op.name);
    quote!(ops::#n)
}

pub(crate) fn shape(space: &Space<'_>, t: &Ty) -> TokenStream {
    match space.kind_of(t.non_null()) {
        JsonKind::Array => quote!(runtime::Shape::Array),
        JsonKind::Object => quote!(runtime::Shape::Object),
        JsonKind::Null | JsonKind::Bool | JsonKind::Integer | JsonKind::Number | JsonKind::String | JsonKind::Any => {
            quote!(runtime::Shape::Primitive)
        }
    }
}

pub(crate) fn style(p: &Param) -> TokenStream {
    style_variant(p.style)
}

/// Returns the runtime's `Style` variant for a parameter style.
pub(crate) fn style_variant(style: Style) -> TokenStream {
    let name = match style {
        Style::Simple => "Simple",
        Style::Label => "Label",
        Style::Matrix => "Matrix",
        Style::Form => "Form",
        Style::SpaceDelimited => "SpaceDelimited",
        Style::PipeDelimited => "PipeDelimited",
        Style::DeepObject => "DeepObject",
        Style::Cookie => "Cookie",
        Style::TabDelimited => "TabDelimited",
    };
    let s = ident(name);
    quote!(runtime::Style::#s)
}

/// Returns `runtime::Enc` for a style. A named constant for the common defaults. Otherwise
/// a struct literal whose field names say what each flag means.
pub(crate) fn enc(style: Style, explode: bool, allow_reserved: bool) -> TokenStream {
    match (style, explode, allow_reserved) {
        (Style::Form, true, false) => quote!(runtime::Enc::FORM),
        (Style::Simple, false, false) => quote!(runtime::Enc::SIMPLE),
        _ => {
            let s = style_variant(style);
            quote!(runtime::Enc { style: #s, explode: #explode, allow_reserved: #allow_reserved })
        }
    }
}

pub(crate) fn param_enc(p: &Param) -> TokenStream {
    enc(p.style, p.explode, p.allow_reserved)
}

pub(crate) fn status_code(r: &Resp) -> Option<u16> {
    if let Status::Code(c) = r.status { Some(c) } else { None }
}

/// Returns the name of the status enum of range or `default` response `r`.
pub(crate) fn status_enum(r: &Resp) -> proc_macro2::Ident {
    format_ident!("{}Status", crate::emit::rust::names::pascal(&r.variant))
}

/// Returns the registered codes that range or `default` response `r` stands for: those of its class, or every final
/// status for `default`, that no other response of `op` declares.
pub(crate) fn variant_codes(op: &Operation, r: &Resp) -> Vec<u16> {
    let taken = |c: u16| {
        op.responses.iter().any(|x| match x.status {
            Status::Code(d) => d == c,
            Status::Range(n) => r.status == Status::Default && c / 100 == n,
            Status::Default => false,
        })
    };
    let codes = match r.status {
        Status::Range(n) => n * 100..n * 100 + 100,
        Status::Code(_) | Status::Default => runtime::__private::FINAL,
    };
    codes.filter(|c| runtime::__private::reason(*c).is_some() && !taken(*c)).collect()
}

/// How a response variant carries its data.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum VariantShape {
    /// `NoContent`: fixed status with no body and no headers.
    Unit,
    /// `Ok(Order)`: fixed status and a body with no headers.
    Tuple,
    /// `Ok { body, x_next }`: anything else.
    Struct,
}

pub(crate) fn variant_shape(r: &Resp) -> VariantShape {
    match (status_code(r).is_some(), r.contents.is_empty(), r.headers.is_empty()) {
        (true, true, true) => VariantShape::Unit,
        (true, false, true) => VariantShape::Tuple,
        _ => VariantShape::Struct,
    }
}

/// Returns the media type to put in `Content-Type` when sending this content.
pub(crate) fn send_media(c: &Content) -> String {
    if c.media.contains('*') { "application/octet-stream".into() } else { c.media.clone() }
}

pub(crate) fn boxed(b: bool, t: TokenStream) -> TokenStream {
    if b { quote!(Box<#t>) } else { t }
}

pub(crate) fn wrap(b: bool, e: TokenStream) -> TokenStream {
    if b { quote!(Box::new(#e)) } else { e }
}

/// Which variants of the per-operation enums contain a `Box`.
pub(crate) struct Boxing {
    /// Per response: is the body boxed.
    pub(crate) resp: Vec<bool>,
    /// Per request body content: is the `Body` variant boxed.
    pub(crate) body_enum: Vec<bool>,
    /// Per response and per content: is the `<Status>Body` variant boxed.
    pub(crate) resp_enums: Vec<Vec<bool>>,
}

pub(crate) fn content_plan(space: &Space<'_>, contents: &[Content]) -> Vec<bool> {
    if contents.len() < 2 {
        return vec![false; contents.len()];
    }
    box_plan(
        &contents
            .iter()
            .map(|c| {
                let s = space.approx_size(&c.ty);
                (s, Some(s))
            })
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn body_size(space: &Space<'_>, contents: &[Content], plan: &[bool]) -> usize {
    if let [c] = contents {
        return space.approx_size(&c.ty);
    }
    contents.iter().zip(plan).map(|(c, b)| if *b { WORD } else { space.approx_size(&c.ty) }).max().unwrap_or(0) + WORD
}

pub(crate) fn boxing(space: &Space<'_>, op: &Operation) -> Boxing {
    let body_enum = op.body.as_ref().map_or_else(Vec::new, |b| content_plan(space, &b.contents));
    let resp_enums: Vec<Vec<bool>> = op.responses.iter().map(|r| content_plan(space, &r.contents)).collect();
    let sizes: Vec<(usize, Option<usize>)> = op
        .responses
        .iter()
        .zip(&resp_enums)
        .map(|(r, plan)| {
            let body = if r.contents.is_empty() { 0 } else { body_size(space, &r.contents, plan) };
            let headers: usize = r.headers.iter().map(|h| space.approx_size(&h.ty)).sum();
            let status = if status_code(r).is_none() { 2 } else { 0 };
            (body + headers + status, (!r.contents.is_empty()).then_some(body))
        })
        .collect();
    Boxing { resp: box_plan(&sizes), body_enum, resp_enums }
}

/// Is `c` a binary body typed `runtime::Blob`? It is in memory, streamed out or (for a request) still arriving.
pub(crate) fn blob(c: &Content) -> bool {
    c.kind == MediaKind::Bytes && c.ty == Ty::Bytes
}

/// Returns the Rust type of one content: `runtime::Items<T>` for sequential bodies, `runtime::Blob` for binary bodies.
pub(crate) fn content_ty(space: &Space<'_>, c: &Content) -> TokenStream {
    if blob(c) {
        return quote!(runtime::Blob);
    }
    if let Ty::Vec(item) = &c.ty
        && c.streams()
    {
        let t = ty(space, item, true);
        return quote!(runtime::Items<#t>);
    }
    ty(space, &c.ty, true)
}

/// Returns the Rust type of a body made of `contents`: one type or an enum per operation.
pub(crate) fn body_ty(space: &Space<'_>, contents: &[Content], enum_name: &str) -> TokenStream {
    if let [c] = contents {
        content_ty(space, c)
    } else {
        let e = ident(enum_name);
        quote!(#e)
    }
}

/// Does a success response carry one Rust type in every media type that it declares? Then it is that type. The server
/// encodes it in the media type that `Accept` chose. Streamed and binary bodies keep a variant each. So does a
/// response declared once in `components/responses`, whose enum the operations share.
pub(crate) fn collapses(space: &Space<'_>, r: &Resp) -> bool {
    let structured =
        |c: &Content| matches!(c.kind, MediaKind::Json | MediaKind::Xml | MediaKind::Form | MediaKind::Text);
    let first = r.contents.first().map(|c| content_ty(space, c).to_string());
    r.is_success()
        && r.shared.is_none()
        && r.contents.len() > 1
        && r.contents.iter().all(|c| structured(c) && Some(content_ty(space, c).to_string()) == first)
}

/// Returns the type of the body of a response. Its one type when it [`collapses`]. Otherwise as [`body_ty`] says.
pub(crate) fn resp_body_ty(space: &Space<'_>, r: &Resp, enum_name: &str) -> TokenStream {
    if collapses(space, r) { content_ty(space, &r.contents[0]) } else { body_ty(space, &r.contents, enum_name) }
}

pub(crate) fn eq_derive(space: &Space<'_>, tys: &[&Ty]) -> Option<TokenStream> {
    (!tys.iter().any(|t| has_float(space, t))).then(|| quote!(Eq,))
}

/// Returns the spec `default` of parameter `p` as an expression when it gives a usable one.
pub(crate) fn spec_default(space: &Space<'_>, p: &Param) -> Option<TokenStream> {
    p.default.as_ref().and_then(|d| crate::emit::rust::types::default_expr(space, &p.ty, d, true))
}

/// Returns a field initializer for parameter `p` named `n`. Its spec `default` or else `None` or `Default::default()`.
pub(crate) fn param_init(space: &Space<'_>, p: &Param, n: &proc_macro2::Ident) -> TokenStream {
    let start = param_start(space, p);
    quote!(#n: #start,)
}

/// Returns the value an optional parameter starts at: its spec `default`, else `None`. A parameter without either is
/// required: it has no start.
pub(crate) fn param_start(space: &Space<'_>, p: &Param) -> TokenStream {
    spec_default(space, p).unwrap_or_else(|| quote!(None))
}

/// Returns `use super::types;` when `needed`: a module of `api` that refers to a generated type.
pub(crate) fn types_import(needed: bool) -> Option<TokenStream> {
    needed.then(|| {
        quote!(
            use super::types;
        )
    })
}

/// Returns `server` as a `runtime::ServerUrl`.
pub(crate) fn server_url(server: &crate::ir::Server) -> TokenStream {
    let (template, vars) = (lit(&server.url), server_variables(server));
    let name = server.name.as_deref().map_or_else(
        || quote!(None),
        |n| {
            let n = lit(n);
            quote!(Some(#n))
        },
    );
    quote!(runtime::ServerUrl { template: #template, name: #name, variables: #vars })
}

fn server_variables(server: &crate::ir::Server) -> TokenStream {
    let v = server.variables.iter().map(|(n, d, allowed)| {
        let (n, d) = (lit(n), lit(d));
        let a = allowed.iter().map(|x| lit(x));
        quote!((#n, #d, &[#(#a),*]))
    });
    quote!(&[#(#v),*])
}

/// Returns a multipart body some of whose parts list several media types (`contentType: "image/png, image/jpeg"`).
pub(crate) fn lists_part_types(c: &Content) -> bool {
    c.kind.is_multipart() && c.encoding.iter().any(|(_, ct)| ct.contains(','))
}

/// Returns whether the operation's `Params` carry `part_types`: a request body that [`lists_part_types`].
pub(crate) fn chooses_part_types(op: &Operation) -> bool {
    op.body.as_ref().is_some_and(|b| b.contents.iter().any(lists_part_types))
}

/// Returns the `part_types` field of `Params` where [`chooses_part_types`].
pub(crate) fn part_types_field(op: &Operation) -> Option<TokenStream> {
    chooses_part_types(op).then(|| {
        quote! {
            pub part_types: runtime::PartTypes,
        }
    })
}

/// Returns `part_types: runtime::PartTypes::new(),` where [`chooses_part_types`]. No choice, so every part goes as its
/// first listed type.
pub(crate) fn part_types_init(op: &Operation) -> Option<TokenStream> {
    chooses_part_types(op).then(|| quote!(part_types: runtime::PartTypes::new(),))
}

/// Returns the media types of the success responses of an operation in the order of the description as (media type,
/// variant name). `Accept` chooses among them.
pub(crate) fn produces(op: &Operation) -> Vec<(&str, &Name)> {
    let mut out: Vec<(&str, &Name)> = vec![];
    for c in op.responses.iter().filter(|r| r.is_success()).flat_map(|r| &r.contents) {
        if !out.iter().any(|(m, _)| *m == c.media) {
            out.push((&c.media, &c.variant));
        }
    }
    out
}

/// Does `Params` carry the response media type? Only with two or more to choose from.
pub(crate) fn chooses_media(op: &Operation) -> bool {
    produces(op).len() > 1
}

/// Returns `Media`: one variant per media type of the success responses where [`chooses_media`].
pub(crate) fn media_enum(op: &Operation) -> Option<TokenStream> {
    if !chooses_media(op) {
        return None;
    }
    let media = produces(op);
    let n = num(media.len());
    let variants: Vec<proc_macro2::Ident> = media.iter().map(|(_, v)| pascal_ident(v)).collect();
    let names = media.iter().map(|(m, _)| lit(m));
    let (first, rest) = variants.split_first()?;
    let at = (1..media.len()).map(num);
    Some(quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Media {
            #(#variants,)*
        }

        impl Media {
            pub const ALL: [Self; #n] = [#(Self::#variants),*];

            #[must_use]
            pub const fn from_index(index: usize) -> Self {
                match index {
                    #(#at => Self::#rest,)*
                    _ => Self::#first,
                }
            }

            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    #(Self::#variants => #names,)*
                }
            }
        }
    })
}

/// Returns `media` where [`chooses_media`]. The response media type that the server answers in.
pub(crate) fn media_field(op: &Operation) -> Option<TokenStream> {
    chooses_media(op).then(|| {
        quote! {
            pub media: Media,
        }
    })
}

/// Returns `media: Media::ALL[0],` where [`chooses_media`].
pub(crate) fn media_init(op: &Operation) -> Option<TokenStream> {
    chooses_media(op).then(|| quote!(media: Media::ALL[0],))
}

/// Returns `#[deprecated]` on what a deprecated security scheme generates.
pub(crate) fn deprecated_scheme(deprecated: bool) -> Option<TokenStream> {
    deprecated.then(|| quote!(#[deprecated = "the security scheme is deprecated"]))
}

/// Returns `Some(&name)`.
pub(crate) fn some_ref(name: impl quote::ToTokens) -> TokenStream {
    quote!(Some(&#name))
}

/// Is `op` one of `names`: its `operationId`, or its generated name in snake case?
pub(crate) fn listed(names: &[String], op: &Operation) -> bool {
    let name = snake(&op.name);
    names.iter().any(|n| op.operation_id.as_deref() == Some(n.as_str()) || *n == name || snake(&Name::new(n)) == name)
}

/// The first status code that is a success.
const SUCCESS_FROM: u16 = 200;
/// The first status code past the successes.
const SUCCESS_TO: u16 = 300;
/// The class of the first error status: `4XX`.
const ERROR_CLASS: u16 = 4;

impl Resp {
    /// Is it a success: a code from 200 to 299 or the `2XX` range? `default` is not: it stands for every status not
    /// listed.
    pub(crate) fn is_success(&self) -> bool {
        matches!(self.status, Status::Code(c) if (SUCCESS_FROM..SUCCESS_TO).contains(&c))
            || self.status == Status::Range(2)
    }

    /// Is it an error: a code of 400 or more, the `4XX` or `5XX` range, or `default`? `1XX` and `3XX` are neither.
    pub(crate) fn is_error(&self) -> bool {
        matches!(self.status, Status::Code(c) if c >= runtime::http::StatusCode::BAD_REQUEST.as_u16())
            || matches!(self.status, Status::Range(r) if r >= ERROR_CLASS)
            || self.status == Status::Default
    }
}
