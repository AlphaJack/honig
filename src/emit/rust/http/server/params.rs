//! Reading an operation's parameters from a request.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::emit::rust::http::codec::decode_contents;
use crate::emit::rust::http::codec::{Decode, Held};
use crate::emit::rust::http::paths::template;
use crate::emit::rust::http::shared::Boxing;
use crate::emit::rust::http::shared::{num, shape, style};
use crate::emit::rust::names::snake;
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::snake_ident;
use crate::emit::rust::syntax::{json_tokens, lit};
use crate::ir::http::{Location, Operation, Param};
use crate::ir::media::MediaKind;
use crate::ir::types::{JsonKind, Ty};

pub(crate) struct ParamInputs {
    pub(crate) req: bool,
    pub(crate) caps: bool,
    pub(crate) raw_query: bool,
    pub(crate) query: bool,
    pub(crate) body: bool,
    pub(crate) fallible: bool,
    /// A binary or sequential body: moved out of the request into the parameters.
    pub(crate) take: bool,
}

/// Does the operation accept a binary body (`application/octet-stream`, ...) or a sequential one (`application/jsonl`,
/// ...)? It moves into the parameters and is not copied. It reaches the handler as it arrives.
pub(crate) fn takes_body(op: &Operation) -> bool {
    op.body.as_ref().is_some_and(|b| b.contents.iter().any(|c| c.kind == MediaKind::Bytes || c.streams()))
}

pub(crate) fn param_inputs(op: &Operation) -> ParamInputs {
    let has = |l: Location| op.params.iter().any(|p| p.location == l);
    ParamInputs {
        // A body needs the request for its content type. It is checked against the declared media types. It is read to
        // pick one of several (multipart) or to tell an absent body.
        req: has(Location::Header) || has(Location::Cookie) || has(Location::Part) || op.body.is_some(),
        caps: has(Location::Path),
        raw_query: has(Location::QueryString),
        query: has(Location::Query),
        body: op.body.is_some() || has(Location::Part),
        // A body of another media type than the declared ones is refused.
        fallible: !op.params.is_empty() || op.body.is_some(),
        take: takes_body(op),
    }
}

/// Returns an exploded form object spreads its properties over the whole query
/// (`?R=100&G=200`): `raw` without the operation's other query parameters.
pub(crate) fn spread_object(space: &Space<'_>, op: &Operation, p: &Param, raw: TokenStream) -> TokenStream {
    let spread = p.location == Location::Query
        && p.content.is_none()
        && matches!(p.style, crate::ir::http::Style::Form)
        && p.explode
        && matches!(space.kind_of(p.ty.non_null()), JsonKind::Object);
    if !spread {
        return raw;
    }
    let others = op.params.iter().filter(|o| o.location == Location::Query && o.wire != p.wire).map(|o| lit(&o.wire));
    quote!(runtime::without_keys(#raw, &[#(#others),*]))
}

/// Returns where parameter `p` is in the request as a JSON pointer: `/query/limit`. Validation names it the same way.
pub(crate) fn param_pointer(p: &Param) -> String {
    let loc = match p.location {
        Location::Path => "path",
        Location::Query | Location::QueryString => "query",
        Location::Header => "header",
        Location::Cookie => "cookie",
        // `/body/<property>/<header>`: the part the header travels in.
        Location::Part => "body",
    };
    let key = |k: &str| k.replace('~', "~0").replace('/', "~1");
    match p.part.as_deref() {
        Some(part) => format!("/{loc}/{}/{}", key(part), key(&p.wire)),
        None => format!("/{loc}/{}", key(&p.wire)),
    }
}

/// Returns where the server reads one parameter from as an `Option<Value>` expression. `at` is its
/// [`param_pointer`].
#[expect(clippy::too_many_arguments, reason = "the parts of one parameter that its source needs")]
pub(crate) fn param_source(
    p: &Param,
    vars: &[String],
    s: &TokenStream,
    shape: &TokenStream,
    explode: bool,
    wire: &Literal,
    at: &Literal,
    body: &TokenStream,
) -> TokenStream {
    match p.location {
        Location::Path => {
            let cap = match cap_index(vars, p) {
                0 => quote!(caps.first()),
                i => {
                    let i = num(i);
                    quote!(caps.get(#i))
                }
            };
            quote!(#cap.and_then(|c| runtime::decode_path(c, #wire, #s, #shape, #explode)))
        }
        // `allowEmptyValue`: a bare `?flag` turns a boolean on; other
        // non-string types read an empty value as absent.
        Location::Query if p.allow_empty => {
            let inner = if let Ty::Option(t) = &p.ty { t.as_ref() } else { &p.ty };
            let as_flag = *inner == Ty::Bool;
            let text = *inner == Ty::String;
            quote!(runtime::empty_value(runtime::decode_query(query, #wire, #at, #s, #shape, #explode)?, #as_flag, #text))
        }
        Location::Query => quote!(runtime::decode_query(query, #wire, #at, #s, #shape, #explode)?),
        Location::QueryString => {
            let form = p.content == Some(MediaKind::Form);
            quote!(runtime::decode_querystring(raw_query, #form))
        }
        Location::Header => {
            quote!(runtime::decode_header(runtime::joined_header(req.headers(), #wire).as_deref(), #shape, #explode))
        }
        Location::Cookie => {
            quote!(runtime::decode_cookie(runtime::Message::header(req, "cookie").as_deref(), #wire, #s, #shape, #explode))
        }
        Location::Part => {
            let part = lit(p.part.as_deref().unwrap_or_default());
            quote!(runtime::decode_header(runtime::part_header(runtime::Message::content_type(req), &#body, #part, #wire).as_deref(), #shape, #explode))
        }
    }
}

/// Returns the position of path parameter `p` among the template's variables.
fn cap_index(vars: &[String], p: &Param) -> usize {
    vars.iter().position(|v| v == &p.wire).unwrap_or(0)
}

/// Returns how the server decodes a plain scalar parameter from its text with no `Value` in between. That is an
/// integer, number, boolean or string without `content`, `default` or `allowEmptyValue`. It is a `Result` expression.
/// `None` for every other parameter. Those go through `runtime::from_param`.
pub(crate) fn text_decode(
    space: &Space<'_>,
    p: &Param,
    vars: &[String],
    s: &TokenStream,
    wire: &Literal,
    at: &Literal,
) -> Option<TokenStream> {
    let scalar = matches!(
        space.kind_of(p.ty.non_null()),
        JsonKind::Integer | JsonKind::Number | JsonKind::Bool | JsonKind::String
    );
    if !scalar || p.content.is_some() || p.default.is_some() || p.style == crate::ir::http::Style::DeepObject {
        return None;
    }
    let text = match p.location {
        Location::Path => {
            let i = num(cap_index(vars, p));
            return Some(quote!(runtime::path_param(caps, #i, #s, #at)));
        }
        Location::Query if p.allow_empty => return None,
        Location::Query => quote!(runtime::query_text(query, #wire, #at)?),
        Location::Header => quote!(runtime::joined_header(req.headers(), #wire).map(runtime::trimmed_header)),
        Location::Cookie => quote!(runtime::cookie_text(runtime::Message::header(req, "cookie").as_deref(), #wire)),
        Location::QueryString | Location::Part => return None,
    };
    Some(quote!(runtime::from_text(#text, #at)))
}

/// Decodes every parameter and the body into `Params`.
pub(crate) fn server_params(
    space: &Space<'_>,
    op: &Operation,
    m: &TokenStream,
    inputs: &ParamInputs,
    bx: &Boxing,
) -> TokenStream {
    let f = format_ident!("{}_params", snake(&op.name));
    let read = [
        (inputs.req, quote!(request: req)),
        (inputs.caps, quote!(caps)),
        (inputs.raw_query, quote!(raw_query)),
        (inputs.query, quote!(query)),
        (inputs.body, quote!(body)),
        (inputs.take, quote!(stream)),
        (crate::emit::rust::http::shared::chooses_media(op), quote!(media)),
    ]
    .into_iter()
    .filter_map(|(used, field)| used.then_some(field))
    .collect::<Vec<_>>();
    // The body. It is moved out of the request for a binary body. Otherwise it is borrowed from it.
    let body_expr = quote!(body);
    let vars: Vec<String> = template(&op.path).into_iter().filter_map(Result::err).collect();
    let fields = op.params.iter().map(|p| {
        let (wire, at) = (lit(&p.wire), lit(&param_pointer(p)));
        // A parameter with `content` is one serialized string on the wire.
        let shape = if p.content.is_some() { quote!(runtime::Shape::Prim) } else { shape(space, &p.ty) };
        let (s, explode) = (style(p), p.explode);
        // Scalars decode from their text: no `Value` in between.
        // A library type has the text of its format checked first.
        let checked = crate::emit::rust::http::codec::validated_map(space, &p.ty);
        if let Some(decode) = text_decode(space, p, &vars, &s, &wire, &at) {
            let n = snake_ident(&p.name);
            return quote!(#n: #decode #checked?,);
        }
        let raw = spread_object(space, op, p, param_source(p, &vars, &s, &shape, explode, &wire, &at, &body_expr));
        let from = if p.content == Some(MediaKind::Json) && p.location != Location::QueryString {
            quote!(runtime::from_json_param)
        } else {
            quote!(runtime::from_param)
        };
        let n = snake_ident(&p.name);
        // An absent parameter with a `default` decodes as that default.
        let raw = match &p.default {
            Some(default) => {
                let json = json_tokens(default);
                quote!(#raw.or_else(|| Some(#json)))
            }
            None => raw,
        };
        quote!(#n: #from(#raw, #at) #checked?,)
    });
    let body = op.body.as_ref().map(|b| {
        let d = Decode {
            status: quote!(0),
            ct: quote!(runtime::Message::content_type(req)),
            body: body_expr.clone(),
            q: quote!(?),
            held: if inputs.take { Held::OwnedCow } else { Held::Borrowed },
            stream: Some(quote!(stream)),
        };
        let e = decode_contents(space, &b.contents, &bx.body_enum, Some(&quote!(#m::Body)), &d);
        if b.required {
            quote!(body: #e,)
        } else {
            // Present when sent: an empty form body is still a body.
            quote!(body: if #body_expr.is_empty() && runtime::Message::header(req, "content-type").is_none() { None } else { Some(#e) },)
        }
    });
    // `415` before decoding: JSON sent as `text/plain` is not JSON the description accepts.
    let media_check = op.body.as_ref().map(|b| {
        let media = b.contents.iter().map(|c| lit(&c.media));
        quote!(runtime::check_media(runtime::Message::content_type(req), &[#(#media),*])?;)
    });
    // Which listed type each part went out as is the client's choice; the parts carry their own `Content-Type`.
    let part_types = crate::emit::rust::http::shared::part_types_init(op);
    // The media type that `Accept` chose by its index in the `produces` of the operation. Same list and same order.
    let media =
        crate::emit::rust::http::shared::chooses_media(op).then(|| quote!(media: #m::Media::from_index(media),));
    let value = quote!(#m::Params { #(#fields)* #body #part_types #media });
    decoder(op, &f, m, inputs, &read, media_check.as_ref(), &value)
}

/// Returns the decoder `f`. It binds what it reads of the `Incoming`, checks the media type of the body and builds
/// `value`.
fn decoder(
    op: &Operation,
    f: &proc_macro2::Ident,
    m: &TokenStream,
    inputs: &ParamInputs,
    read: &[TokenStream],
    media_check: Option<&TokenStream>,
    value: &TokenStream,
) -> TokenStream {
    // Every field except the body is a reference. It is copied out of a borrowed `Incoming` and moved out of an owned
    // one.
    let (param, bind) = if inputs.body {
        (quote!(incoming: runtime::Incoming<'_>), quote!(let runtime::Incoming { #(#read,)* .. } = incoming;))
    } else {
        (quote!(incoming: &runtime::Incoming<'_>), quote!(let runtime::Incoming { #(#read,)* .. } = *incoming;))
    };
    if inputs.fallible {
        quote! {
            fn #f(#param) -> Result<#m::Params, runtime::Reason> {
                #bind
                #media_check
                Ok(#value)
            }
        }
    } else {
        // With nothing to decode except the media type that `Accept` chose it is copied out. That is a `const fn`.
        let constness = (op.params.is_empty() && op.body.is_none()).then(|| quote!(const));
        quote! {
            #constness fn #f(#param) -> #m::Params {
                #bind
                #value
            }
        }
    }
}
