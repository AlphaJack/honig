//! The error schema of the server: which response renders a rejection and the conversions to it.

use std::collections::BTreeMap;

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::emit::rust::http::shared::{send_media, some_ref};
use crate::emit::rust::names::snake;
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::lit;
use crate::emit::rust::syntax::{pascal_ident, snake_ident};
use crate::ir::Api;
use crate::ir::http::{Content, Status};
use crate::ir::media::MediaKind;
use crate::ir::types::{JsonKind, Kind, Ty};

/// Returns the `runtime::ErrorSchema` of the JSON body of an error response if the generator can fill
/// it. It needs at least a status or a message field. Every other required field must be
/// a `const`, or a scalar, array or object that it can leave empty.
pub(crate) fn error_schema(space: &Space<'_>, c: &Content) -> Option<TokenStream> {
    let Ty::Named(id) = &c.ty else { return None };
    let Kind::Struct(st) = &space.types.get(*id).kind else { return None };
    let mut fields = vec![];
    let mut useful = false;
    for f in &st.fields {
        let wire = plain_name(&f.wire);
        // A `const` has one valid value: whatever name it has, any other value would break it.
        if let Some(value) = const_value(space, &f.ty) {
            if f.required {
                let (w, v) = (lit(&f.wire), lit(&value.to_string()));
                fields.push(quote!((#w, runtime::ErrorField::Const(#v))));
            }
            continue;
        }
        // An enum (a closed `code` list) has no value that the runtime can choose. So the schema cannot be filled
        // when it requires one. Any value written would break it.
        if is_enum(space, &f.ty) {
            if f.required {
                return None;
            }
            continue;
        }
        let kind = space.kind_of(f.ty.non_null());
        let item_is_string = matches!(f.ty.non_null(), Ty::Vec(t) if space.kind_of(t) == JsonKind::String);
        let (status, detail) = (STATUS_FIELDS.contains(&wire.as_str()), DETAIL_FIELDS.contains(&wire.as_str()));
        let what = match (wire.as_str(), kind) {
            (_, JsonKind::Integer | JsonKind::Number) if status => quote!(Status),
            (_, JsonKind::String) if status => quote!(StatusText),
            (_, JsonKind::String) if detail => quote!(Detail),
            ("title", JsonKind::String) => quote!(Title),
            ("type", JsonKind::String) => quote!(Type),
            ("errors" | "violations" | "issues" | "details", JsonKind::Array) if item_is_string => {
                quote!(ViolationMessages)
            }
            ("errors" | "violations" | "issues" | "details", JsonKind::Array) => quote!(Violations),
            _ if !f.required => continue,
            (_, JsonKind::String) => quote!(Empty("string")),
            (_, JsonKind::Integer | JsonKind::Number) => quote!(Empty("integer")),
            (_, JsonKind::Bool) => quote!(Empty("boolean")),
            (_, JsonKind::Array) => quote!(Empty("array")),
            _ => return None,
        };
        useful |= !what.to_string().starts_with("Empty");
        let w = lit(&f.wire);
        fields.push(quote!((#w, runtime::ErrorField::#what)));
    }
    let media = lit(&send_media(c));
    let open = !st.closed || st.additional.is_some();
    useful.then(|| quote!(runtime::ErrorSchema { media: #media, fields: &[#(#fields),*], open: #open }))
}

/// Returns a warning per error response whose body the server cannot fill for a request that it refuses itself. Such a
/// refusal goes out as RFC 9457 `problem+json` and not in the declared body. A handler's typed error still can.
pub(crate) fn unfillable_errors(api: &Api, space: &Space<'_>) -> Vec<crate::diag::Diagnostic> {
    let mut out = vec![];
    for op in &api.http.operations {
        for r in op.responses.iter().filter(|r| has_error_body(r)) {
            if declared_error_schema(space, r).is_some() {
                continue;
            }
            out.push(crate::diag::Diagnostic::warning(
                format!("{}/responses/{}", op.pointer, status_key(r.status)),
                "the server cannot fill this error body for the requests it refuses itself, so they get RFC 9457 \
                 `application/problem+json`. A JSON body needs a status or message property, and every other \
                 required property must be a `const`, a scalar, an array or an object (a required enum has no value \
                 to choose). A `text/*` body needs `type: string`",
            ));
        }
    }
    out
}

/// Returns a warning per required header of an error response. The server refuses some requests itself (`400` for an
/// invalid one, `401`, `406`, `415`) and cannot know the value. Such a refusal goes out without the header. A client
/// that requires it then reads none of them.
pub(crate) fn unset_error_headers(api: &Api) -> Vec<crate::diag::Diagnostic> {
    let mut out = vec![];
    for op in &api.http.operations {
        for r in op.responses.iter().filter(|r| is_error(r)) {
            for h in r.headers.iter().filter(|h| !h.ty.is_option()) {
                out.push(crate::diag::Diagnostic::warning(
                    format!(
                        "{}/responses/{}/headers/{}",
                        op.pointer,
                        status_key(r.status),
                        crate::json::escape_token(&h.wire)
                    ),
                    "a required header of an error response: the requests that the server refuses itself (an \
                     invalid one, a missing credential) go out without it, so a client that requires it cannot read \
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
    is_error(r) && !r.contents.is_empty()
}

/// Returns the `runtime::ErrorSchema` of the body of a response if the generator can fill it: a JSON body by its
/// fields, or a `text/*` string that holds the detail alone.
fn declared_error_schema(space: &Space<'_>, r: &crate::ir::http::Resp) -> Option<TokenStream> {
    let json = r.contents.iter().find(|c| c.kind == MediaKind::Json).and_then(|c| error_schema(space, c));
    json.or_else(|| {
        let text = r.contents.iter().find(|c| c.kind == MediaKind::Text && c.ty == Ty::String)?;
        let media = send_media(text);
        media.starts_with("text/").then(|| {
            let media = lit(&media);
            quote!(runtime::ErrorSchema { media: #media, fields: &[], open: false })
        })
    })
}

/// Returns an error response: 4XX, 5XX or `default`.
fn is_error(r: &crate::ir::http::Resp) -> bool {
    !matches!(r.status, Status::Code(c) if c < runtime::http::StatusCode::BAD_REQUEST.as_u16())
}

/// Error schema properties that hold the HTTP status as [`plain_name`] spells them.
const STATUS_FIELDS: [&str; 6] = ["code", "status", "statuscode", "httpstatus", "httpcode", "errorcode"];
/// Error schema properties that say what went wrong as [`plain_name`] spells them.
const DETAIL_FIELDS: [&str; 9] =
    ["message", "detail", "details", "error", "description", "reason", "msg", "errordescription", "errormessage"];

/// Returns a property name in lower case without `_` and `-`. So `errorCode`, `error_code` and `Error-Code` are one
/// name.
fn plain_name(wire: &str) -> String {
    wire.chars().filter(|c| !matches!(c, '_' | '-')).map(|c| c.to_ascii_lowercase()).collect()
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

/// Is `ty` a string or integer enum through aliases?
fn is_enum(space: &Space<'_>, ty: &Ty) -> bool {
    let Ty::Named(id) = ty.non_null() else { return false };
    match &space.types.get(*id).kind {
        Kind::StrEnum(_) | Kind::IntEnum(_) => true,
        Kind::Alias(t) => is_enum(space, t),
        Kind::Struct(_) | Kind::Union(_) | Kind::Newtype(_) | Kind::Const(_) | Kind::Checked(_) | Kind::Pending => {
            false
        }
    }
}

/// Returns `impl From<types::X> for runtime::ApiError` for each error schema with a required integer status. A handler
/// returns `Err(types::X { .. }.into())`. The body goes out as it is at that status with its detail as the message.
/// `aliases`: types declared as an alias of another. The other one gets the impl.
pub(crate) fn error_conversions(api: &Api, space: &Space<'_>, aliases: &BTreeMap<usize, usize>) -> TokenStream {
    let mut seen: Vec<crate::ir::types::TypeId> = vec![];
    let mut out = vec![];
    let errors = api.http.operations.iter().flat_map(|op| &op.responses).filter(|r| is_error(r));
    for c in errors.flat_map(|r| &r.contents).filter(|c| c.kind == MediaKind::Json) {
        let Ty::Named(id) = &c.ty else { continue };
        let id = &crate::ir::types::TypeId(aliases.get(&id.0).copied().unwrap_or(id.0));
        let Kind::Struct(st) = &space.types.get(*id).kind else { continue };
        if seen.contains(id) || space.types.get(*id).rust_type.is_some() {
            continue;
        }
        let field = |names: &[&str], kind: JsonKind| {
            st.fields.iter().find(|f| {
                f.required
                    && !is_enum(space, &f.ty)
                    && space.types.nullable(&f.ty).is_none()
                    && names.contains(&plain_name(&f.wire).as_str())
                    && space.kind_of(&f.ty) == kind
            })
        };
        let Some(status) = field(&STATUS_FIELDS, JsonKind::Integer) else { continue };
        seen.push(*id);
        let status_ident = snake_ident(&status.name);
        let status = quote!(runtime::status_of(body.#status_ident));
        let message = field(&DETAIL_FIELDS, JsonKind::String).map_or_else(
            || quote!(String::new()),
            |f| {
                let n = snake_ident(&f.name);
                quote!(body.#n.clone())
            },
        );
        let (name, media) = (pascal_ident(&space.types.get(*id).name), lit(&send_media(c)));
        out.push(quote! {
            impl From<super::types::#name> for runtime::ApiError {
                fn from(body: super::types::#name) -> Self {
                    Self::new(#status, #message).with_body(#media, &body)
                }
            }
        });
    }
    quote!(#(#out)*)
}

/// Returns `error_schema`: the error schema that the description declares for an operation and status.
/// `runtime::Rejection::response` fills it. `None` gives RFC 9457.
pub(crate) fn error_schema_fn(api: &Api, space: &Space<'_>) -> TokenStream {
    let mut schemas: Vec<String> = vec![];
    let mut consts = vec![];
    let mut schema_ident = |t: TokenStream| {
        let key = t.to_string();
        let i = schemas.iter().position(|k| *k == key).unwrap_or_else(|| {
            schemas.push(key);
            let n = format_ident!("ERROR_SCHEMA_{}", schemas.len() - 1);
            consts.push(quote!(const #n: runtime::ErrorSchema = #t;));
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
            const fn error_schema(_operation: Option<&str>, _status: u16) -> Option<&'static runtime::ErrorSchema> {
                None
            }
        };
    }
    // The most common error schema of the description. It is for requests that reach no operation. It is also for a
    // status that an operation declares no error for (one error schema across the API). Never for a status whose
    // declared body cannot be filled: that one gets RFC 9457 and not a schema the description gives elsewhere.
    let global = uses.iter().max_by_key(|(_, n)| *n).map_or_else(|| quote!(None), |(n, _)| some_ref(n));
    // (status match, whether it reads `status`, operation names). Operations with the same match share an arm.
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
            const ERROR_SCHEMA: Option<&runtime::ErrorSchema> = #global;

            const fn error_schema(_operation: Option<&str>, _status: u16) -> Option<&'static runtime::ErrorSchema> {
                ERROR_SCHEMA
            }
        };
    }
    let status = if arms.iter().any(|(_, _, reads, _)| *reads) { quote!(status) } else { quote!(_status) };
    let arms = arms.into_iter().map(|(_, inner, _, names)| quote!(Some(#(#names)|*) => #inner,));
    quote! {
        #(#consts)*

        fn error_schema(operation: Option<&str>, #status: u16) -> Option<&'static runtime::ErrorSchema> {
            match operation {
                #(#arms)*
                _ => #global,
            }
        }
    }
}

/// Returns the error schema of one operation by status as an expression of type `Option<&runtime::ErrorSchema>`:
/// `answer` per declared status and `otherwise` for the rest. A status that the next broader entry already answers the
/// same way is dropped (`400` under a same-schema `4XX`). The rest share one arm per answer (`400 | 404 => ..`). The
/// flag says whether it reads `status`.
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
                add(&mut ranges, n, (d * runtime::__private::CLASS, (d + 1) * runtime::__private::CLASS - 1));
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
