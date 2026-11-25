//! Writing an operation's response.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

use crate::emit::rust::http::codec::{body_bytes, encode_body, encode_contents, fallible_encode};
use crate::emit::rust::http::shared::{Boxing, VariantShape};
use crate::emit::rust::http::shared::{code, collapses, num, produces, send_media, status_code, variant_shape};
use crate::emit::rust::names::{pascal, snake};
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::{ident, lit};
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::http::{Content, Operation};
use crate::ir::media::MediaKind;
use crate::ir::names::Name;
use crate::ir::types::Ty;

/// Does any response variant contain a body or a header the encoder moves out? A status alone is copied.
/// Does the encoder take the response by value? Only when an arm moves a body out (an owned `String` text body, bytes, a
/// multipart body, a stream) or encodes XML, whose shape is the type's own and not a reference's. Every other body and
/// every header encodes from a borrow.
pub(crate) fn carries_data(op: &Operation) -> bool {
    op.responses.iter().flat_map(|r| &r.contents).any(|c| match c.kind {
        MediaKind::Text => matches!(c.ty, Ty::String),
        MediaKind::Json | MediaKind::Linkset | MediaKind::Form => false,
        MediaKind::JsonLines | MediaKind::JsonSeq | MediaKind::Sse => c.streams(),
        MediaKind::Xml | MediaKind::Bytes | MediaKind::Multipart | MediaKind::MultipartMixed => true,
    })
}

pub(crate) fn encode_fallible(op: &Operation) -> bool {
    op.responses.iter().any(|r| !r.headers.is_empty() || r.contents.iter().any(fallible_encode))
}

/// Does the encoder receive the media type `Accept` chose? When a response carries one type in several media types.
pub(crate) fn encodes_by_media(space: &Space<'_>, op: &Operation) -> bool {
    op.responses.iter().any(|r| collapses(space, r))
}

/// Encodes the implementation's response.
pub(crate) fn server_encode(space: &Space<'_>, op: &Operation, op_mod: &TokenStream, bx: &Boxing) -> TokenStream {
    let fn_name = format_ident!("{}_encode", snake(&op.name));
    let (fallible, carries) = (encode_fallible(op), carries_data(op));
    let produces = produces(op);
    // (bindings + types + body, patterns). Variants that bind the same types
    // and encode alike share one arm (clippy `match_same_arms`).
    let mut groups: Vec<(String, Vec<TokenStream>, TokenStream)> = vec![];
    for (i, r) in op.responses.iter().enumerate() {
        let variant = pascal_ident(&r.variant);
        let mut fields = vec![];
        if status_code(r).is_none() {
            fields.push(ident("status"));
        }
        if !r.contents.is_empty() {
            fields.push(ident("body"));
        }
        fields.extend(r.headers.iter().map(|h| snake_ident(&h.name)));
        let pat = match variant_shape(r) {
            VariantShape::Unit => quote!(#op_mod::Response::#variant),
            VariantShape::Tuple => quote!(#op_mod::Response::#variant(body)),
            VariantShape::Struct => quote!(#op_mod::Response::#variant { #(#fields),* }),
        };
        let code = status_code(r).map_or_else(
            || quote!(status.as_u16()),
            |c| {
                let c = code(c);
                quote!(#c)
            },
        );
        let tys: Vec<&Ty> = r.contents.iter().map(|c| &c.ty).chain(r.headers.iter().map(|h| &h.ty)).collect();
        let shape = format!("{fields:?} {tys:?} {} {:?}", bx.resp[i], r.status);
        let headers = r.headers.iter().map(|h| {
            let (n, wire) = (snake_ident(&h.name), lit(&h.wire));
            if h.wire.eq_ignore_ascii_case("set-cookie") {
                // One line per cookie (RFC 9110 section 5.3).
                return quote!(for h in ::honig::__private::encode_header_lines(&#n)? { ::honig::Message::set_header(&mut r, #wire, h)?; });
            }
            let value = if h.json {
                quote!(::honig::__private::encode_json_header(&::honig::__private::to_value(&#n)?))
            } else if h.linkset {
                quote!(::honig::__private::encode_linkset_header(&::honig::__private::to_value(&#n)?))
            } else {
                let e = crate::emit::rust::http::shared::enc(crate::ir::http::Style::Simple, h.explode, false);
                quote!(::honig::__private::encode_header(&#n, #e)?)
            };
            quote!(if let Some(h) = #value { ::honig::Message::set_header(&mut r, #wire, h)?; })
        });
        // A borrowed response binds a reference: the body encodes from it as it is, also when it is boxed.
        let e = if bx.resp[i] && carries { quote!(*body) } else { quote!(body) };
        let body = (!r.contents.is_empty()).then(|| {
            if collapses(space, r) {
                return in_chosen_media(&r.contents, &e, &produces, !carries);
            }
            let enum_path = {
                let n = format_ident!("{}Body", pascal(&r.variant));
                quote!(#op_mod::#n)
            };
            encode_contents(&quote!(r), &e, &r.contents, &enum_path, &bx.resp_enums[i], false, !carries)
        });
        let whole = match (r.headers.is_empty(), r.contents.as_slice()) {
            (true, [c]) => body_bytes(&e, c, !carries).map(|b| (lit(&send_media(c)), b)),
            _ => None,
        };
        let arm = if r.headers.is_empty() && r.contents.is_empty() {
            quote!(::honig::__private::new_response(#code))
        } else if let Some((media, bytes)) = whole {
            quote!(::honig::__private::body_response(#code, #media, #bytes))
        } else {
            quote! {{
                let mut r = ::honig::__private::new_response(#code);
                #(#headers)*
                #body
                r
            }}
        };
        let key = format!("{shape} {arm}");
        match groups.iter_mut().find(|(k, ..)| *k == key) {
            Some((_, pats, _)) => pats.push(pat),
            None => groups.push((key, vec![pat], arm)),
        }
    }
    encode_fn(&fn_name, op_mod, &groups, fallible, carries, encodes_by_media(space, op))
}

/// Returns one value in the media type `Accept` chose: its index in the operation's `produces`.
fn in_chosen_media(contents: &[Content], e: &TokenStream, produces: &[(&str, &Name)], borrowed: bool) -> TokenStream {
    let arms = contents.iter().skip(1).map(|c| {
        let i = num(produces.iter().position(|(m, _)| *m == c.media).unwrap_or(0));
        let stmts = encode_body(&quote!(r), e, c, false, borrowed);
        quote!(#i => { #stmts })
    });
    let first = encode_body(&quote!(r), e, &contents[0], false, borrowed);
    quote!(match media { #(#arms)* _ => { #first } })
}

/// Returns the encoder over `groups` (key, patterns, arm). It is a `Result` when encoding can fail. It is a borrowed
/// response when no variant carries anything to move out. A response with one variant destructures it with no match.
fn encode_fn(
    name: &proc_macro2::Ident,
    op_mod: &TokenStream,
    groups: &[(String, Vec<TokenStream>, TokenStream)],
    fallible: bool,
    carries: bool,
    by_media: bool,
) -> TokenStream {
    let (bind, expr) = match groups {
        [(_, pats, arm)] if pats.len() == 1 => {
            let pat = &pats[0];
            (Some(quote!(let #pat = resp;)), arm.clone())
        }
        _ => {
            let arms = groups.iter().map(|(_, pats, arm)| crate::emit::rust::syntax::match_arm(pats, arm));
            (None, quote!(match resp { #(#arms)* }))
        }
    };
    let resp = if carries { quote!(#op_mod::Response) } else { quote!(&#op_mod::Response) };
    let media = by_media.then(|| quote!(, media: usize));
    if fallible {
        quote! {
            fn #name(resp: #resp #media) -> Result<::honig::Response, ::honig::ClientError> {
                #bind
                Ok(#expr)
            }
        }
    } else {
        quote! {
            fn #name(resp: #resp #media) -> ::honig::Response {
                #bind
                #expr
            }
        }
    }
}
