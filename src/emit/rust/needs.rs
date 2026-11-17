//! The runtime features the generated code needs from the target crate.

use std::collections::BTreeSet;

use super::syntax::untyped;
use crate::ir::Api;
use crate::ir::http::{Location, Operation};
use crate::ir::media::MediaKind;
use crate::ir::security::{SchemeKind, SecurityScheme};
use crate::ir::types::{Format, Ty, def_types};
use crate::settings::{
    BASE64, CLIENT, COOKIE, DateLib, FormatLibs, IDN, IPNET, JIFF, MULTIPART, OAUTH, PKCE, REGRESS, SERVER, XML,
};

/// Returns the runtime features that the description needs whatever the crate depends on. That is the halves that it
/// generates and the codecs, checks and flows that they call. `patterns`: it has a `pattern`. [`uses_pattern`] finds
/// it.
pub(crate) fn runtime_features(
    api: &Api,
    libs: FormatLibs,
    client: bool,
    server: bool,
    uses: Uses,
) -> Vec<&'static str> {
    let mut features = codecs(api, server);
    features.extend(client.then_some(CLIENT));
    features.extend(server.then_some(SERVER));
    features.extend((client && api.schemes.iter().any(|s| s.code.is_some())).then_some(PKCE));
    features.extend((client && api.schemes.iter().any(has_flows)).then_some(OAUTH));
    let basic = api.schemes.iter().any(|s| s.kind == SchemeKind::Basic);
    let binary = every_ty(api).any(holds_binary);
    features.extend((basic || binary).then_some(BASE64));
    features.extend(format_checks(api, libs));
    if uses.patterns && !features.contains(&REGRESS) {
        features.push(REGRESS);
    }
    // A mail or host name `format` that no type holds (a schema without `type`) is checked at run time with `idn`.
    if uses.mail_formats && !features.contains(&IDN) {
        features.push(IDN);
    }
    features
}

/// Returns the runtime codecs that the generated code calls. That is for a body or part in their media types, a
/// `Cookie` API key or a cookie parameter that the server reads. The client writes one without the parser of the
/// runtime.
fn codecs(api: &Api, server: bool) -> Vec<&'static str> {
    let contents = || api.http.operations.iter().flat_map(Operation::contents);
    // A part's `encoding.contentType` may list several types.
    let declared = |kind: MediaKind| {
        contents().any(|c| {
            c.kind == kind || c.encoding.iter().any(|(_, types)| types.split(',').any(|t| MediaKind::of(t) == kind))
        })
    };
    let multipart = contents().any(|c| c.kind.is_multipart());
    let cookie_key = |name: &str| {
        api.schemes
            .iter()
            .any(|s| s.name == name && matches!(&s.kind, SchemeKind::ApiKey { location, .. } if location == "cookie"))
    };
    let cookie = api.http.operations.iter().any(|o| {
        o.security.iter().flatten().any(|r| cookie_key(&r.scheme))
            || (server && o.params.iter().any(|p| p.location == Location::Cookie))
    });
    let needed = [(COOKIE, cookie), (MULTIPART, multipart), (XML, declared(MediaKind::Xml))];
    needed.into_iter().filter_map(|(feature, on)| on.then_some(feature)).collect()
}

/// Returns the runtime features of the string formats that a description uses. `idn` is for the types of mail addresses
/// and host names. `regress` checks `regex` without its library type. `ipnet` checks the CIDR blocks with or without
/// theirs.
fn format_checks(api: &Api, libs: FormatLibs) -> Vec<&'static str> {
    let mut used = BTreeSet::new();
    for ty in every_ty(api) {
        formats_in(ty, &mut used);
    }
    let checked = |formats: &[Format]| formats.iter().any(|f| used.contains(f) && untyped(libs, *f));
    let mail = [Format::Email, Format::IdnEmail, Format::Hostname, Format::IdnHostname];
    let needed = [
        // The types of mail addresses and host names are the runtime's own.
        (IDN, mail.iter().any(|f| used.contains(f))),
        // The libraries that would type these formats check them.
        (REGRESS, checked(&[Format::Regex])),
        // The CIDR blocks: as text, and before `ipnet::Ipv4Net` reads one (`runtime::checked`).
        (IPNET, used.contains(&Format::Ipv4Cidr) || used.contains(&Format::Ipv6Cidr)),
        // `runtime::FullTime`: a `time` with its offset.
        (JIFF, used.contains(&Format::Time) && libs.dates == Some(DateLib::Jiff)),
    ];
    needed.into_iter().filter_map(|(feature, on)| on.then_some(feature)).collect()
}

/// Returns the types of every definition, parameter, body and response header.
fn every_ty(api: &Api) -> impl Iterator<Item = &Ty> {
    let in_types = api.types.defs.iter().flat_map(|d| def_types(&d.kind));
    let in_ops = api.http.operations.iter().flat_map(|o| {
        let headers = o.responses.iter().flat_map(|r| r.headers.iter().map(|h| &h.ty));
        o.params.iter().map(|p| &p.ty).chain(o.contents().map(|c| &c.ty)).chain(headers)
    });
    in_types.chain(in_ops)
}

/// Does `ty` hold binary data that JSON carries as base64 (`Binary` or `base64url`), itself or in what it holds?
fn holds_binary(ty: &Ty) -> bool {
    matches!(ty, Ty::Binary | Ty::Format(Format::Base64Url)) || ty.children().iter().any(holds_binary)
}

/// Does the client have a helper for an `OAuth` 2.0 flow of `s`: a grant, the device flow, refresh or discovery?
fn has_flows(s: &SecurityScheme) -> bool {
    s.token_url.is_some()
        || s.device.is_some()
        || s.password_url.is_some()
        || s.code.is_some()
        || s.implicit_url.is_some()
        || s.discovery_url.is_some()
}

/// Collects every string format that `ty` holds itself or in what it holds.
fn formats_in(ty: &Ty, out: &mut BTreeSet<Format>) {
    if let Ty::Format(f) = ty {
        out.insert(*f);
    }
    // A named type's own fields are walked with the rest of the type space.
    ty.children().iter().for_each(|x| formats_in(x, out));
}

/// What the document's schemas use that turns on a runtime feature, found by scanning them as written.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Uses {
    /// `pattern` or `patternProperties`: [`uses_pattern`].
    pub patterns: bool,
    /// A mail or host name `format`: [`uses_format`].
    pub mail_formats: bool,
}

/// Does any schema of the document have a `format` among `names`? Data (examples, enums, defaults) does not count.
pub(crate) fn uses_format(v: &serde_json::Value, parent: &str, names: &[&str]) -> bool {
    match v {
        serde_json::Value::Object(o) => o.iter().any(|(k, c)| {
            (k == "format" && c.as_str().is_some_and(|f| names.contains(&f)) && parent != "properties")
                || (!crate::json::is_data(parent, k) && uses_format(c, k, names))
        }),
        serde_json::Value::Array(a) => a.iter().any(|c| uses_format(c, parent, names)),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}

/// Does any schema of the document use `pattern` or `patternProperties`? Then the generated code checks it with
/// `regress`. Data (examples, enums, defaults) does not count.
pub(crate) fn uses_pattern(v: &serde_json::Value, parent: &str) -> bool {
    match v {
        serde_json::Value::Object(o) => o.iter().any(|(k, c)| {
            (k == "pattern" && c.is_string() && parent != "properties")
                || (k == "patternProperties" && c.is_object())
                || (!crate::json::is_data(parent, k) && uses_pattern(c, k))
        }),
        serde_json::Value::Array(a) => a.iter().any(|c| uses_pattern(c, parent)),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => false,
    }
}
