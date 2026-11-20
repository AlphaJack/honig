//! `documented_examples.rs`: the own examples of the description. They are decoded into the generated types and sent
//! the way that the client sends them.

use proc_macro2::TokenStream;
use quote::quote;

use super::shared::param_enc;
use crate::emit::rust::space::Space;
use crate::emit::rust::syntax::pascal_ident;
use crate::emit::rust::syntax::{lit, mentions, ty};
use crate::ir::Api;
use crate::ir::http::{Location, Operation, Param};
use crate::ir::media::MediaKind;
use crate::ir::types::{Kind, Ty};

/// Returns every parameter's `serializedValue` check.
pub(crate) fn wire_checks(api: &Api, space: &Space<'_>) -> Vec<TokenStream> {
    let mut out = vec![];
    for p in api.http.operations.iter().flat_map(|op| &op.params) {
        let inner = if let Ty::Option(t) = &p.ty { t.as_ref() } else { &p.ty };
        let t = ty(space, inner, true);
        for (at, v, want) in &p.serialized {
            out.extend(wire_check(p, &t, at, v, want));
        }
    }
    out
}

/// Returns `wire`: compares a parameter as sent with its documented `serializedValue`.
pub(crate) fn wire_fn() -> TokenStream {
    quote! {
        fn wire(bad: &mut Vec<String>, at: &str, got: Result<String, runtime::ClientError>, want: &str) {
            match got {
                Ok(got) if got == want || runtime::pct_decode(&got) == want => {}
                Ok(got) => bad.push(format!("{at}: sent `{got}`, documented `{want}`")),
                Err(e) => bad.push(format!("{at}: {e}")),
            }
        }
    }
}

/// Returns a check that the `dataValue` of a parameter reads as its `serializedValue`. The value is decoded
/// into its type and sent the way that the client sends it: typed, so an object keeps the order of its fields.
pub(crate) fn wire_check(
    p: &Param,
    t: &TokenStream,
    at: &str,
    v: &serde_json::Value,
    want: &str,
) -> Option<TokenStream> {
    if p.content.is_some() {
        return None;
    }
    let (wire, e, explode) = (lit(&p.wire), param_enc(p), p.explode);
    let encode = match p.location {
        Location::Path => quote!(runtime::encode_path(#wire, &v, #e).unwrap_or_else(|e| panic!("{e}"))),
        Location::Query => quote!({
            let mut q = Vec::new();
            runtime::encode_query(&mut q, #wire, &v, #e).unwrap_or_else(|e| panic!("{e}"));
            q.join("&")
        }),
        Location::QueryString => return None,
        Location::Header | Location::Part => {
            quote!(runtime::encode_header(&v, #explode).unwrap_or_else(|e| panic!("{e}")).unwrap_or_default())
        }
        Location::Cookie => quote!({
            let mut c = Vec::new();
            runtime::encode_cookie(&mut c, #wire, &v, #e).unwrap_or_else(|e| panic!("{e}"));
            c.join("; ")
        }),
    };
    let (at, text, want) = (lit(at), lit(&v.to_string()), lit(want));
    Some(quote! {
        let got = serde_json::from_str::<#t>(#text)
            .map_err(|e| runtime::ClientError::Encode(e.to_string()))
            .map(|v| #encode);
        wire(&mut bad, #at, got, #want);
    })
}

/// Returns every documented example with the type that it must decode into as (JSON
/// pointer, type, value, decoded like a parameter).
pub(crate) fn documented_examples<'a>(
    api: &'a Api,
    space: &Space<'a>,
) -> Vec<(&'a String, TokenStream, &'a serde_json::Value, bool)> {
    let mut out = vec![];
    for def in &space.types.defs {
        if matches!(def.kind, Kind::Struct(_) | Kind::StrEnum(_) | Kind::IntEnum(_) | Kind::Union(_) | Kind::Newtype(_))
        {
            let n = pascal_ident(&def.name);
            for (at, v) in &def.examples {
                out.push((at, quote!(types::#n), v, false));
            }
        }
        // The own examples of a property decode into the type of its field. Binary
        // properties are left out. Their examples refer to a file and not base64.
        if let Kind::Struct(s) = &def.kind {
            for f in s.fields.iter().filter(|f| !mentions(&f.ty).1) {
                let t = ty(space, &f.ty, true);
                for (at, v) in &f.examples {
                    out.push((at, t.clone(), v, false));
                }
            }
        }
    }
    for op in &api.http.operations {
        for p in &op.params {
            let inner = if let Ty::Option(t) = &p.ty { t.as_ref() } else { &p.ty };
            let t = ty(space, inner, true);
            for (at, v) in &p.examples {
                out.push((at, t.clone(), v, p.content.is_none()));
            }
        }
        // Response headers: a header value decoded like a parameter.
        for h in op.responses.iter().flat_map(|r| &r.headers).filter(|h| !h.json && !h.linkset) {
            let inner = if let Ty::Option(t) = &h.ty { t.as_ref() } else { &h.ty };
            let t = ty(space, inner, true);
            for (at, v) in &h.examples {
                out.push((at, t.clone(), v, true));
            }
        }
        let contents = op.body.iter().flat_map(|b| &b.contents).chain(op.responses.iter().flat_map(|r| &r.contents));
        for c in contents.filter(|c| c.kind == MediaKind::Json) {
            let t = ty(space, &c.ty, true);
            for (at, v) in &c.examples {
                out.push((at, t.clone(), v, false));
            }
        }
    }
    out
}

/// Returns the generated `xml` check. The XML of an example decodes to its `dataValue`. The value encoded decodes back
/// to it.
fn xml_check_fn() -> TokenStream {
    quote! {
        fn xml<T>(bad: &mut Vec<String>, at: &str, data: &str, text: &str)
        where
            T: serde::de::DeserializeOwned + serde::Serialize + runtime::XmlShape + PartialEq + std::fmt::Debug,
        {
            let want: T = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(e) => {
                    bad.push(format!("{at}: the dataValue does not fit: {e}"));
                    return;
                }
            };
            let decoded = runtime::xml_decode::<T>(0, text.as_bytes());
            if decoded.as_ref().ok() != Some(&want) {
                bad.push(format!("{at}: the XML decodes to {decoded:?}, not {want:?}"));
            }
            let back = runtime::xml_encode(&want).and_then(|b| runtime::xml_decode::<T>(0, &b));
            if back.as_ref().ok() != Some(&want) {
                bad.push(format!("{at}: encoded and decoded, it is {back:?}"));
            }
        }
    }
}

/// Returns every XML media type example with its serialized form: `xml::<Body>(&mut bad, at, dataValue, text);`.
fn xml_checks(api: &Api, space: &Space<'_>) -> Vec<TokenStream> {
    api.http
        .operations
        .iter()
        .flat_map(Operation::contents)
        .flat_map(|c| c.serialized.iter().map(move |s| (c, s)))
        .map(|(c, (at, data, text))| {
            let (t, at, data, text) = (ty(space, &c.ty, true), lit(at), lit(&data.to_string()), lit(text));
            quote!(xml::<#t>(&mut bad, #at, #data, #text);)
        })
        .collect()
}

/// Returns `documented_examples.rs`: one test that every example in the description (schemas, media types, parameters)
/// decodes into its generated type. It also tests that every `serializedValue` is what the client sends. `None`
/// without examples.
pub(crate) fn tests(api: &Api, space: &Space<'_>) -> Option<TokenStream> {
    let mut checks = vec![];
    let (mut uses_value, mut uses_param) = (false, false);
    let mut push = |at: &str, t: TokenStream, v: &serde_json::Value, param: bool| {
        // `null` is a placeholder in many descriptions and not a sample of the type.
        if !v.is_null() {
            if param {
                uses_param = true;
            } else {
                uses_value = true;
            }
            let (at, json) = (lit(at), lit(&v.to_string()));
            let f = if param { quote!(param) } else { quote!(value) };
            checks.push(quote!(#f::<#t>(&mut bad, #at, #json);));
        }
    };
    for (at, t, v, param) in documented_examples(api, space) {
        push(at, t, v, param);
    }
    let xmls = xml_checks(api, space);
    let wires = wire_checks(api, space);
    if checks.is_empty() && wires.is_empty() && xmls.is_empty() {
        return None;
    }
    let wire_fn = (!wires.is_empty()).then(wire_fn);
    let xml_fn = (!xmls.is_empty()).then(xml_check_fn);
    let value_fn = uses_value.then(|| {
        quote! {
            fn value<T: serde::de::DeserializeOwned>(bad: &mut Vec<String>, at: &str, text: &str) {
                if let Err(e) = serde_json::from_str::<T>(text) {
                    bad.push(format!("{at}: {e}"));
                }
            }
        }
    });
    let param_fn = uses_param.then(|| {
        quote! {
            fn param<T: serde::de::DeserializeOwned>(bad: &mut Vec<String>, at: &'static str, text: &str) {
                // A parameter is read as its style coerces it: the documented value is parsed once for that. The error
                // points to `at`.
                if let Err(e) = runtime::from_param::<T>(serde_json::from_str(text).ok(), at) {
                    bad.push(e.to_string());
                }
            }
        }
    });
    let allow = crate::emit::rust::syntax::file_allow(crate::emit::rust::syntax::has_deprecated(api));
    Some(quote! {
        #allow
        #![allow(unused_imports, reason = "which of them the examples need depends on the description")]
        use super::{runtime, types};

        #value_fn
        #param_fn
        #wire_fn
        #xml_fn

        #[test]
        fn documented_examples_decode() {
            let mut bad = vec![];
            #(#checks)*
            #(#wires)*
            #(#xmls)*
            assert!(bad.is_empty(), "examples that do not fit their schema:\n{}", bad.join("\n"));
        }
    })
}
