//! The error schema of the server: which response renders a rejection and the conversions to it.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::emit::rust::http::shared::{send_media, some_ref};
use crate::emit::rust::names::snake;
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::lit;
use crate::ir::Api;
use crate::ir::http::{Content, Status};
use crate::ir::media::MediaKind;
use crate::ir::types::{JsonKind, Kind, Ty};

/// Returns the `::honig::ProblemSchema` of the JSON body of an error response if the generator can fill it: RFC 9457
/// `problem+json`, whose members the RFC names. Each of `type`, `title`, `status` and `detail` of the RFC's type is
/// filled. A required `const` gets its value. Any other required member, or a member of the RFC of another type, has
/// no value that the server knows: then `None`, and the server needs your `error_response`.
pub(crate) fn error_schema(space: &Space<'_>, c: &Content) -> Option<TokenStream> {
    let media = send_media(c);
    if !media.split(';').next().unwrap_or_default().trim().eq_ignore_ascii_case("application/problem+json") {
        return None;
    }
    let Ty::Named(id) = &c.ty else { return None };
    let Kind::Struct(st) = &space.types.get(*id).kind else { return None };
    let mut fields = vec![];
    for f in &st.fields {
        let w = lit(&f.wire);
        if let Some(value) = const_value(space, &f.ty) {
            if f.required {
                let v = lit(&value.to_string());
                fields.push(quote!((#w, ::honig::ProblemMember::Const(#v))));
            }
            continue;
        }
        let what = match (f.wire.as_str(), space.kind_of(f.ty.non_null())) {
            ("status", JsonKind::Integer) => quote!(Status),
            ("title", JsonKind::String) => quote!(Title),
            ("detail", JsonKind::String) => quote!(Detail),
            ("type", JsonKind::String) => quote!(Type),
            _ if !f.required => continue,
            _ => return None,
        };
        fields.push(quote!((#w, ::honig::ProblemMember::#what)));
    }
    let media = lit(&media);
    let open = !st.closed || st.additional.is_some();
    Some(quote!(::honig::ProblemSchema { media: #media, fields: &[#(#fields),*], open: #open }))
}

/// Does the server fill every error body that the description declares by itself ([`declared_error_schema`])? Then
/// its `ErrorBodies` is `::honig::__private::Filled`. Otherwise `::honig::__private::Unfilled`, and the server needs `error_response`.
pub(crate) fn all_filled(api: &Api, space: &Space<'_>) -> bool {
    (api.http.operations.iter())
        .flat_map(|op| op.responses.iter().filter(|r| has_error_body(r)))
        .all(|r| declared_error_schema(space, r).is_some())
}

/// Returns a warning per required header of an error response. The server refuses some requests itself (`400` for an
/// invalid one, `401`, `406`, `415`) and cannot know the value. Such a refusal goes out without the header. A client
/// that requires it then decodes none of them.
pub(crate) fn unset_error_headers(api: &Api) -> Vec<crate::diag::Diagnostic> {
    let mut out = vec![];
    for op in &api.http.operations {
        for r in op.responses.iter().filter(|r| r.is_error()) {
            for h in r.headers.iter().filter(|h| !h.ty.is_option()) {
                out.push(crate::diag::Diagnostic::warning(
                    format!(
                        "{}/responses/{}/headers/{}",
                        op.pointer,
                        status_key(r.status),
                        crate::json::escape_token(&h.wire)
                    ),
                    "a required header of an error response: the requests that the server refuses itself (an \
                     invalid one, a missing credential) go out without it, so a client that requires it cannot decode \
                     them. Set it in a tower layer around the server",
                ));
            }
        }
    }
    out
}

/// Returns the key of a status in `responses`.
fn status_key(status: Status) -> String {
    match status {
        Status::Code(c) => c.to_string(),
        Status::Range(d) => format!("{d}XX"),
        Status::Default => "default".to_owned(),
    }
}

/// The error statuses of an operation with their schema. `None`: a body the generator cannot fill.
type Declared = Vec<(Status, Option<proc_macro2::Ident>)>;

/// Returns an error response that declares a body.
fn has_error_body(r: &crate::ir::http::Resp) -> bool {
    r.is_error() && !r.contents.is_empty()
}

/// Returns the `::honig::ProblemSchema` of the body of a response if the generator can fill it: a JSON body by its
/// fields, or a `text/*` string that contains the detail alone.
fn declared_error_schema(space: &Space<'_>, r: &crate::ir::http::Resp) -> Option<TokenStream> {
    let json = r.contents.iter().find(|c| c.kind == MediaKind::Json).and_then(|c| error_schema(space, c));
    json.or_else(|| {
        let text = r.contents.iter().find(|c| c.kind == MediaKind::Text && c.ty == Ty::String)?;
        let media = send_media(text);
        media.starts_with("text/").then(|| {
            let media = lit(&media);
            quote!(::honig::ProblemSchema { media: #media, fields: &[], open: false })
        })
    })
}

/// Returns the value of `ty` when it is a `const` through aliases.
fn const_value<'s>(space: &'s Space<'_>, ty: &Ty) -> Option<&'s serde_json::Value> {
    let Ty::Named(id) = ty.non_null() else { return None };
    match &space.types.get(*id).kind {
        Kind::Const(v) => Some(v),
        Kind::Alias(t) => const_value(space, t),
        Kind::Struct(_)
        | Kind::Union(_)
        | Kind::Newtype(_)
        | Kind::StrEnum(_)
        | Kind::IntEnum(_)
        | Kind::Checked(_)
        | Kind::Pending => None,
    }
}

/// Returns `error_schema`: the error schema that the description declares for an operation and status.
/// `::honig::Rejection::response` fills it. `None` gives RFC 9457.
pub(crate) fn error_schema_fn(api: &Api, space: &Space<'_>) -> TokenStream {
    let mut schemas: Vec<String> = vec![];
    let mut consts = vec![];
    let mut schema_ident = |t: TokenStream| {
        let key = t.to_string();
        let i = schemas.iter().position(|k| *k == key).unwrap_or_else(|| {
            schemas.push(key);
            let n = format_ident!("ERROR_SCHEMA_{}", schemas.len() - 1);
            consts.push(quote!(const #n: ::honig::ProblemSchema = #t;));
            schemas.len() - 1
        });
        format_ident!("ERROR_SCHEMA_{i}")
    };
    let mut uses: Vec<(proc_macro2::Ident, usize)> = vec![];
    // Each operation's error statuses with their schema.
    let mut declared: Vec<(Literal, Declared)> = vec![];
    for op in &api.http.operations {
        let mut by_status = vec![];
        for r in op.responses.iter().filter(|r| has_error_body(r)) {
            let n = declared_error_schema(space, r).map(&mut schema_ident);
            if let Some(n) = &n {
                match uses.iter_mut().find(|(u, _)| u == n) {
                    Some((_, count)) => *count += 1,
                    None => uses.push((n.clone(), 1)),
                }
            }
            by_status.push((r.status, n));
        }
        if !by_status.is_empty() {
            declared.push((lit(&snake(&op.name)), by_status));
        }
    }
    if uses.is_empty() {
        return quote! {
            const fn error_schema(_operation: Option<&str>, _status: u16) -> Option<&'static ::honig::ProblemSchema> {
                None
            }
        };
    }
    // The most common error schema of the description. It is for requests that reach no operation. It is also for a
    // status that an operation declares no error for (one error schema across the API). Never for a status whose
    // declared body cannot be filled: that one gets RFC 9457 and not a schema the description gives elsewhere.
    let global = uses.iter().max_by_key(|(_, n)| *n).map_or_else(|| quote!(None), |(n, _)| some_ref(n));
    // (status match, whether it uses `status`, operation names). Operations with the same match share an arm.
    let mut arms: Vec<(String, TokenStream, bool, Vec<Literal>)> = vec![];
    for (name, by_status) in declared {
        let by_status: Vec<(Status, TokenStream)> =
            by_status.into_iter().map(|(status, n)| (status, n.map_or_else(|| quote!(None), some_ref))).collect();
        let (inner, reads) = status_match(&by_status, &global);
        let key = inner.to_string();
        match arms.iter_mut().find(|(k, ..)| *k == key) {
            Some((.., names)) => names.push(name),
            None => arms.push((key, inner, reads, vec![name])),
        }
    }
    // Every other operation gets `global`: an arm that gives it for every status changes nothing.
    let fallback = global.to_string();
    arms.retain(|(key, ..)| *key != fallback);
    if arms.is_empty() {
        // A const and not `Some(..)` in the body. The signature belongs to the server. The `unnecessary_wraps` of
        // clippy cannot see that.
        return quote! {
            #(#consts)*
            const ERROR_SCHEMA: Option<&::honig::ProblemSchema> = #global;

            const fn error_schema(_operation: Option<&str>, _status: u16) -> Option<&'static ::honig::ProblemSchema> {
                ERROR_SCHEMA
            }
        };
    }
    let status = if arms.iter().any(|(_, _, reads, _)| *reads) { quote!(status) } else { quote!(_status) };
    let arms = arms.into_iter().map(|(_, inner, _, names)| quote!(Some(#(#names)|*) => #inner,));
    quote! {
        #(#consts)*

        fn error_schema(operation: Option<&str>, #status: u16) -> Option<&'static ::honig::ProblemSchema> {
            match operation {
                #(#arms)*
                _ => #global,
            }
        }
    }
}

/// Returns the error schema of one operation by status as an expression of type `Option<&::honig::ProblemSchema>`:
/// `answer` per declared status and `otherwise` for the rest. A status that the next broader entry already answers the
/// same way is dropped (`400` under a same-schema `4XX`). The rest share one arm per answer (`400 | 404 => ..`). The
/// flag says whether it uses `status`.
pub(crate) fn status_match(by_status: &[(Status, TokenStream)], otherwise: &TokenStream) -> (TokenStream, bool) {
    /// Status intervals per answer in first-seen order.
    type Tier<'a> = Vec<(&'a TokenStream, Vec<(u16, u16)>)>;
    fn add<'a>(tier: &mut Tier<'a>, n: &'a TokenStream, pat: (u16, u16)) {
        match tier.iter_mut().find(|(k, _)| k.to_string() == n.to_string()) {
            Some((_, pats)) => pats.push(pat),
            None => tier.push((n, vec![pat])),
        }
    }
    let same = |a: Option<&TokenStream>, b: &TokenStream| a.is_some_and(|a| a.to_string() == b.to_string());
    let default = by_status.iter().find(|(s, _)| *s == Status::Default).map(|(_, n)| n);
    let range = |d: u16| by_status.iter().find(|(s, _)| *s == Status::Range(d)).map(|(_, n)| n);
    let fallback = default.unwrap_or(otherwise);
    // Exact codes a different-answer range covers must come before the ranges;
    // every other code joins its answer's range arm.
    let mut codes: Tier<'_> = vec![];
    let mut ranges: Tier<'_> = vec![];
    for (status, n) in by_status {
        match *status {
            Status::Code(c) => {
                let covering = range(c / 100);
                if same(Some(covering.unwrap_or(fallback)), n) {
                    continue;
                }
                add(if covering.is_some() { &mut codes } else { &mut ranges }, n, (c, c));
            }
            Status::Range(d) if !same(Some(fallback), n) => {
                add(&mut ranges, n, (d * honig::__private::CLASS, (d + 1) * honig::__private::CLASS - 1));
            }
            Status::Range(_) | Status::Default => {}
        }
    }
    if codes.is_empty() && ranges.is_empty() {
        return (fallback.clone(), false);
    }
    let arms = codes.into_iter().chain(ranges).map(|(n, spans)| {
        let pats = coalesce(spans).into_iter().map(|(lo, hi)| {
            let (l, h) = (Literal::u16_unsuffixed(lo), Literal::u16_unsuffixed(hi));
            if lo == hi { quote!(#l) } else { quote!(#l..=#h) }
        });
        quote!(#(#pats)|* => #n,)
    });
    (quote!(match status { #(#arms)* _ => #fallback }), true)
}

/// Sorts and joins inclusive intervals with touching ones joined: `400..=499` and
/// `500..=599` become `400..=599` (clippy `manual_range_patterns`).
pub(crate) fn coalesce(mut spans: Vec<(u16, u16)>) -> Vec<(u16, u16)> {
    spans.sort_unstable();
    let mut out: Vec<(u16, u16)> = Vec::with_capacity(spans.len());
    for (lo, hi) in spans {
        match out.last_mut() {
            Some(last) if lo <= last.1.saturating_add(1) => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}
