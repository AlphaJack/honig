//! Encoding and decoding bodies as the client and the server write them.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use super::shared::{blob, enc, lists_part_types, send_media, style_variant, wrap};
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::pascal_ident;
use crate::emit::rust::syntax::{holds, ident, lit, ty};
use crate::ir::http::{Content, ItemLimits};
use crate::ir::media::MediaKind;
use crate::ir::types::Ty;

/// Can encoding body `c` fail? Only a plain `String` text body and bytes cannot.
pub(crate) fn fallible_encode(c: &Content) -> bool {
    !matches!((c.kind, &c.ty), (MediaKind::Text, Ty::String) | (MediaKind::Bytes, _))
}

/// Returns `::honig::Framing` of a sequential media kind.
pub(crate) fn framing(kind: MediaKind) -> TokenStream {
    if kind == MediaKind::JsonSeq {
        quote!(::honig::Framing::JsonSeq)
    } else if kind == MediaKind::Sse {
        quote!(::honig::Framing::Sse)
    } else {
        quote!(::honig::Framing::JsonLines)
    }
}

/// Returns the `&[(part, media)]` that a multipart body encodes with. A request whose parts list several
/// types gets the choice of the caller from `p.part_types`. Only the client encodes requests with
/// `p` in scope. Everything else sends the first listed type of each part.
fn part_types(c: &Content) -> TokenStream {
    if !c.response && lists_part_types(c) {
        return chosen_types(c);
    }
    let types = encoding_types(c);
    quote!(&[#(#types),*])
}

/// Statements that put `e` (an owned place, moved or borrowed) into `target` as body `c`.
/// `parts`: a `parts` list of (property, header, value) is in scope for multipart bodies.
/// `by_ref`: `e` is already a reference. A body that the client encodes from borrowed parameters is one. That is
/// only kinds that it borrows and never one that moves into the request.
pub(crate) fn encode_body(
    target: &TokenStream,
    e: &TokenStream,
    c: &Content,
    parts: bool,
    by_ref: bool,
) -> TokenStream {
    let m = lit(&send_media(c));
    if let Some(bytes) = body_bytes(e, c, by_ref) {
        return quote!(::honig::Message::set_body(&mut #target, #m, #bytes););
    }
    let arg = if by_ref { e.clone() } else { quote!(&#e) };
    match c.kind {
        // A range (`image/*`) goes out as the media type that the blob carries when it fits.
        MediaKind::Bytes if c.media.contains('*') => {
            let range = lit(&c.media);
            quote!(::honig::__private::set_blob_in(&mut #target, #range, #e);)
        }
        MediaKind::Bytes => quote!(::honig::__private::set_blob(&mut #target, #m, #e);),
        MediaKind::Multipart if !c.transfer.is_empty() => {
            let types = part_types(c);
            // `contentEncoding: base64` fields. Each part is base64 with its `Content-Transfer-Encoding`.
            let names = c.transfer.iter().map(|p| lit(p));
            let transfer = quote!([#((#names, "Content-Transfer-Encoding", "base64".to_owned())),*]);
            let headers = if parts { quote!(&[&parts[..], &#transfer[..]].concat()) } else { quote!(&#transfer) };
            quote! {
                let (ct, bytes) = ::honig::__private::multipart_encode_with(#arg, #types, #headers)?;
                ::honig::Message::set_body(&mut #target, ct, bytes);
            }
        }
        MediaKind::Multipart => {
            let types = part_types(c);
            if parts {
                quote! {
                    let (ct, bytes) = ::honig::__private::multipart_encode_with(#arg, #types, &parts)?;
                    ::honig::Message::set_body(&mut #target, ct, bytes);
                }
            } else {
                quote! {
                    let (ct, bytes) = ::honig::__private::multipart_encode(#arg, #types)?;
                    ::honig::Message::set_body(&mut #target, ct, bytes);
                }
            }
        }
        MediaKind::MultipartMixed => {
            let types = part_types(c);
            quote! {
                let (ct, bytes) = ::honig::__private::mixed_encode(#arg, #m, #types)?;
                ::honig::Message::set_body(&mut #target, ct, bytes);
            }
        }
        // What is left is a sequential body sent as it is produced. `body_bytes` handled every other kind.
        MediaKind::Json
        | MediaKind::Linkset
        | MediaKind::Text
        | MediaKind::Xml
        | MediaKind::Form
        | MediaKind::JsonLines
        | MediaKind::JsonSeq
        | MediaKind::Sse => {
            let framing = framing(c.kind);
            quote!(::honig::__private::set_items(&mut #target, #m, #e, #framing)?;)
        }
    }
}

/// Returns the bytes of body `c` from `e` when it is encoded whole (`set_body` accepts them). `None` for a blob, a
/// multipart body or one sent as it is produced. `by_ref` as for [`encode_body`].
pub(crate) fn body_bytes(e: &TokenStream, c: &Content, by_ref: bool) -> Option<TokenStream> {
    let arg = if by_ref { e.clone() } else { quote!(&#e) };
    Some(match c.kind {
        MediaKind::Json => quote!(::honig::__private::json_encode(#arg)?),
        MediaKind::Linkset => quote!(::honig::__private::linkset_encode(#arg)?.into_bytes()),
        // An owned `String` body moves into the response (no copy). A named string type converts first. The
        // function form also accepts `*body` (the body of a boxed variant). `*body.into_bytes()` would misparse that.
        MediaKind::Text if matches!(c.ty, Ty::String) => quote!(String::into_bytes(#e)),
        MediaKind::Text => quote!(::honig::__private::text_encode_as(#e)?),
        MediaKind::Xml => quote!(::honig::__private::xml_encode(#arg)?),
        MediaKind::Form if !form_types(c).is_empty() => {
            let styles = form_encode_styles(c);
            let types = form_types(c);
            quote!(::honig::__private::form_encode_with(#arg, &[#(#styles),*], &[#(#types),*])?)
        }
        MediaKind::Form if !c.form_styles.is_empty() => {
            let styles = form_encode_styles(c);
            quote!(::honig::__private::form_encode_styled(#arg, &[#(#styles),*])?)
        }
        MediaKind::Form => quote!(::honig::__private::form_encode(#arg)?),
        MediaKind::Bytes
        | MediaKind::Multipart
        | MediaKind::MultipartMixed
        | MediaKind::JsonLines
        | MediaKind::JsonSeq
        | MediaKind::Sse => return None,
    })
}

/// Emits the encoding of the contents. With `borrowed`, `e` is a reference to the body (the client encodes
/// borrowed parameters). So is the value of each variant.
pub(crate) fn encode_contents(
    target: &TokenStream,
    e: &TokenStream,
    contents: &[Content],
    enum_path: &TokenStream,
    plan: &[bool],
    parts: bool,
    borrowed: bool,
) -> TokenStream {
    if let [c] = contents {
        return encode_body(target, e, c, parts, borrowed);
    }
    let arms = contents.iter().zip(plan).map(|(c, b)| {
        let v = pascal_ident(&c.variant);
        let inner = match (*b, borrowed) {
            (true, true) => quote!(&**b),
            (true, false) => quote!(*b),
            (false, _) => quote!(b),
        };
        let stmts = encode_body(target, &inner, c, parts, borrowed);
        quote!(#enum_path::#v(b) => { #stmts })
    });
    quote!(match #e { #(#arms)* })
}

/// How a body is decoded: where it is and how errors propagate.
pub(crate) struct Decode {
    pub(crate) status: TokenStream,
    pub(crate) ct: TokenStream,
    pub(crate) body: TokenStream,
    /// Error propagation (`?` or `.map_err(...)?`).
    pub(crate) q: TokenStream,
    /// What the body is. It decides how a binary body becomes its own `Vec<u8>`.
    pub(crate) held: Held,
    /// The body of a request still to come (`stream`). A binary body consumes it as it arrives. `None` for a
    /// response.
    pub(crate) stream: Option<TokenStream>,
}

/// The body a decoder decodes.
#[derive(Clone, Copy)]
pub(crate) enum Held {
    /// A response's bytes nothing uses afterwards: moved out without a copy.
    Moved,
    /// Bytes that something uses afterwards: copied. That is a response kept for its headers or a borrowed request.
    Borrowed,
    /// A request's `Cow` the decoder owns: `into_owned` moves an owned buffer instead of copying it.
    OwnedCow,
}

/// Returns the `(name, style, shape, explode)` of each styled form field as the decoder expects them.
fn form_decode_styles(c: &Content) -> Vec<TokenStream> {
    c.form_styles
        .iter()
        .map(|(p, st, ex, _, shape)| {
            let shape = ident(shape);
            let (p, st) = (lit(p), style_variant(*st));
            quote!((#p, #st, ::honig::__private::Shape::#shape, #ex))
        })
        .collect()
}

/// Returns form fields whose `encoding.contentType` is XML or JSON. They travel in that syntax.
fn form_types(c: &Content) -> Vec<TokenStream> {
    c.encoding
        .iter()
        .filter(|(_, ct)| {
            let m = ct.split([';', ',']).next().unwrap_or("").trim().to_ascii_lowercase();
            m.ends_with("/xml") || m.ends_with("+xml") || m.ends_with("/json") || m.ends_with("+json")
        })
        .map(|(p, ct)| type_pair(p, ct))
        .collect()
}

/// Returns the `(name, style)` of each styled form field as the encoder expects them.
fn form_encode_styles(c: &Content) -> Vec<TokenStream> {
    c.form_styles
        .iter()
        .map(|(p, st, ex, ar, _)| {
            let (p, e) = (lit(p), enc(*st, *ex, *ar));
            quote!((#p, #e))
        })
        .collect()
}

/// Returns `.map(...)` that unwraps a decoded `::honig::__private::formats::Validated` of `t`: the decoder decodes `t` with the
/// text of its library formats checked first. `None` when `t` contains none.
pub(crate) fn validated_map(space: &Space<'_>, t: &Ty) -> Option<TokenStream> {
    let h = holds(space, t)?;
    let t = ty(space, t, true);
    Some(quote!(.map(::honig::__private::formats::Validated::<#t, #h>::into_inner)))
}

/// Returns the expression that decodes one content and whether it is a `Result`.
pub(crate) fn decode_body(space: &Space<'_>, c: &Content, d: &Decode) -> (TokenStream, bool) {
    let (status, ct, body) = (&d.status, &d.ct, &d.body);
    match c.kind {
        MediaKind::Json => {
            let checked = validated_map(space, &c.ty);
            (quote!(::honig::__private::json_decode(#status, &#body) #checked), true)
        }
        MediaKind::Linkset => {
            (quote!(::honig::__private::linkset_decode(#status, ::honig::__private::utf8_text(#status, &#body)?)), true)
        }
        MediaKind::Text if matches!(c.ty, Ty::String) => {
            (quote!(::honig::__private::text_decode(#status, &#body)), true)
        }
        MediaKind::Text => (quote!(::honig::__private::text_decode_as(#status, &#body)), true),
        MediaKind::Xml => (quote!(::honig::__private::xml_decode(#status, &#body)), true),
        MediaKind::Bytes => {
            let bytes = match d.held {
                Held::Moved => quote!(Vec::from(#body)),
                Held::Borrowed => quote!(#body.to_vec()),
                Held::OwnedCow => quote!(#body.into_owned()),
            };
            let typed = match (&d.stream, d.held) {
                (Some(stream), Held::OwnedCow) => quote!(::honig::Blob::received(#body, &#stream)),
                _ => quote!(::honig::Blob::from(#bytes)),
            };
            // A range (`image/*`) says less than the `Content-Type` that came: the blob keeps that.
            let typed = if c.media.contains('*') { quote!(#typed.received_as(#ct)) } else { typed };
            (if blob(c) { typed } else { bytes }, false)
        }
        MediaKind::Form if !form_types(c).is_empty() => {
            let styles = form_decode_styles(c);
            let types = form_types(c);
            (quote!(::honig::__private::form_decode_with(#status, &#body, &[#(#styles),*], &[#(#types),*])), true)
        }
        MediaKind::Form if !c.form_styles.is_empty() => {
            let styles = form_decode_styles(c);
            (quote!(::honig::__private::form_decode_styled(#status, &#body, &[#(#styles),*])), true)
        }
        MediaKind::Form => (quote!(::honig::__private::form_decode(#status, &#body)), true),
        MediaKind::Multipart => (quote!(::honig::__private::multipart_decode(#status, #ct, &#body)), true),
        MediaKind::MultipartMixed => {
            let prefix = c.positional_prefix.map_or_else(
                || quote!(None),
                |n| {
                    let n = Literal::usize_unsuffixed(n);
                    quote!(Some(#n))
                },
            );
            (quote!(::honig::__private::mixed_decode(#status, #ct, &#body, #prefix)), true)
        }
        MediaKind::JsonLines | MediaKind::JsonSeq | MediaKind::Sse => decode_sequence(space, c, d),
    }
}

/// Returns the expression that decodes a sequential body (JSON Lines, JSON text sequences, SSE) and whether it is a
/// `Result`. Each item has the text of its library formats checked first, and the sequence its `minItems`, `maxItems`
/// and `uniqueItems`.
fn decode_sequence(space: &Space<'_>, c: &Content, d: &Decode) -> (TokenStream, bool) {
    let (status, body) = (&d.status, &d.body);
    // A sequential request body the adapter left unread reaches the handler as it arrives.
    if let (true, Some(stream), Held::OwnedCow) = (c.streams(), &d.stream, d.held) {
        let framing = framing(c.kind);
        // Each item has the text of its library formats checked first.
        let checked = if let Ty::Vec(item) = &c.ty { holds(space, item) } else { None };
        let received = match checked {
            Some(h) => quote!(::honig::Items::received_validated::<#h>(&#body, &#stream, #framing)),
            None => quote!(::honig::Items::received(&#body, &#stream, #framing)),
        };
        // `minItems`, `maxItems` and `uniqueItems` apply to the items as they arrive.
        return match item_limits(c.item_limits) {
            Some(l) => (quote!(#received.and_then(|items| items.limited(#l))), true),
            None => (received, true),
        };
    }
    let f = format_ident!(
        "{}",
        if c.kind == MediaKind::JsonSeq {
            "json_seq_decode"
        } else if c.kind == MediaKind::Sse {
            "sse_decode"
        } else {
            "jsonl_decode"
        }
    );
    let items = c.streams().then(|| quote!(.map(::honig::Items::List)));
    // Each item has the text of its library formats checked first.
    let item = if let Ty::Vec(item) = &c.ty { Some(item.as_ref()) } else { None };
    let checked = item.and_then(|item| {
        holds(space, item).map(|h| {
            let item = ty(space, item, true);
            quote!(.map(|v: Vec<::honig::__private::formats::Validated<#item, #h>>| {
                v.into_iter().map(::honig::__private::formats::Validated::into_inner).collect::<Vec<_>>()
            }))
        })
    });
    let limited =
        item_limits(c.item_limits).map(|l| quote!(.and_then(|v| ::honig::__private::limit_items(#status, v, #l))));
    (quote!(::honig::__private::#f(#status, &#body) #checked #limited #items), true)
}

/// Returns `::honig::__private::Limits` of a sequence's `minItems`, `maxItems` and `uniqueItems`. `None` without any.
pub(crate) fn item_limits(l: ItemLimits) -> Option<TokenStream> {
    if l.is_empty() {
        return None;
    }
    let count = |n: Option<u64>| match n {
        Some(n) => {
            let n = Literal::u64_unsuffixed(n);
            quote!(Some(#n))
        }
        None => quote!(None),
    };
    let (min, max, unique) = (count(l.min), count(l.max), l.unique);
    Some(quote!(::honig::__private::Limits { min: #min, max: #max, unique: #unique }))
}

/// How general a `content` key is with the most specific first. "Only the most specific key is applicable" (3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Generality {
    /// `text/plain; charset=utf-8`.
    WithParameters,
    /// `text/plain`.
    Exact,
    /// `text/*`.
    Range,
    /// `*/*`.
    Any,
}

/// Returns how general a `content` key is.
pub(crate) fn media_rank(media: &str) -> Generality {
    let (essence, params) = media.split_once(';').map_or((media, ""), |(e, p)| (e, p));
    match essence.trim() {
        "*/*" => Generality::Any,
        e if e.ends_with("/*") => Generality::Range,
        _ if params.trim().is_empty() => Generality::Exact,
        _ => Generality::WithParameters,
    }
}

/// Returns the expression that decodes `contents` and chooses by content type.
///
/// `enum_path` is the path of the enum of one variant per media type; `None` when every one decodes into the same type.
pub(crate) fn decode_contents(
    space: &Space<'_>,
    contents: &[Content],
    plan: &[bool],
    enum_path: Option<&TokenStream>,
    d: &Decode,
) -> TokenStream {
    let one = |c: &Content| {
        let (e, fallible) = decode_body(space, c, d);
        let q = &d.q;
        if fallible { quote!(#e #q) } else { e }
    };
    if let [c] = contents {
        return one(c);
    }
    let ct = &d.ct;
    let variant = |c: &Content, x: TokenStream| {
        let v = pascal_ident(&c.variant);
        enum_path.map_or_else(|| x.clone(), |p| quote!(#p::#v(#x)))
    };
    let fallback = variant(&contents[0], wrap(plan[0], one(&contents[0])));
    let mut chain = quote!({ #fallback });
    // "For requests that match multiple keys, only the most specific key is
    // applicable" (3.2 `content`). `text/plain` is tried before `text/*`
    // whatever the document order. The chain is built inside out.
    // The first content is also the fallback. It needs no check of its own when last.
    let mut order: Vec<(usize, (&Content, &bool))> = contents.iter().zip(plan).enumerate().collect();
    order.sort_by_key(|(_, (c, _))| media_rank(&c.media));
    if order.last().is_some_and(|(i, _)| *i == 0) {
        order.pop();
    }
    for (_, (c, b)) in order.into_iter().rev() {
        let (m, x) = (lit(&c.media), variant(c, wrap(*b, one(c))));
        chain = quote!(if ::honig::__private::media_matches(#ct, #m) { #x } else #chain);
    }
    chain
}

/// Does decoding use the `Content-Type` (`ct`)? To choose among several, for a multipart boundary or for the media
/// type that a [`honig::Blob`] of a range keeps.
pub(crate) fn needs_ct(contents: &[Content]) -> bool {
    contents.len() > 1
        || contents
            .iter()
            .any(|c| c.kind.is_multipart() || (c.kind == MediaKind::Bytes && blob(c) && c.media.contains('*')))
}

/// Returns the `encoding.contentType` of each property. The first listed when there are several.
pub(crate) fn encoding_types(c: &Content) -> impl Iterator<Item = TokenStream> + '_ {
    c.encoding.iter().map(|(p, ct)| type_pair(p, ct))
}

/// Returns `(property, media type)` with the first of a comma-separated list.
pub(crate) fn type_pair(p: &str, ct: &str) -> TokenStream {
    let (p, ct) = (lit(p), lit(ct.split(',').next().unwrap_or(ct).trim()));
    quote!((#p, #ct))
}

/// Returns the part types a request sends: `p.part_types` chosen among each part's declared list.
pub(crate) fn chosen_types(c: &Content) -> TokenStream {
    let declared = c.encoding.iter().map(|(p, ct)| {
        let (p, ct) = (lit(p), lit(ct));
        quote!((#p, #ct))
    });
    quote!(&p.part_types.choose(&[#(#declared),*])?)
}

#[cfg(test)]
mod tests {
    use super::{Generality, media_rank};

    /// The rank of a `content` key, most specific first: parameters, exact type, range, any.
    #[test]
    fn a_media_type_ranks_by_how_much_it_names() {
        assert_eq!(media_rank("text/plain; charset=utf-8"), Generality::WithParameters);
        assert_eq!(media_rank("application/json"), Generality::Exact);
        assert_eq!(media_rank("application/json; "), Generality::Exact);
        assert_eq!(media_rank("text/*"), Generality::Range);
        assert_eq!(media_rank("*/*"), Generality::Any);
    }
}
