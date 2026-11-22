//! Security schemes and the requirements that operations make of them.

use serde_json::Value;

use super::refs::resolve_value;
use super::types::Builder;
use crate::ir::names::Name;
use crate::json;
use crate::json::escape_token;

#[derive(Debug, Clone)]
/// A security scheme.
pub(crate) struct SecurityScheme {
    /// Key in `components.securitySchemes`.
    pub name: String,
    /// How credentials travel.
    pub kind: SchemeKind,
    /// The `OAuth2` `clientCredentials` flow: its `tokenUrl`.
    pub token_url: Option<String>,
    /// The `OAuth2` `deviceAuthorization` flow (3.2) as (`deviceAuthorizationUrl`, `tokenUrl`).
    pub device: Option<(String, String)>,
    /// `deprecated: true` (3.2).
    pub deprecated: bool,
    /// `description`.
    pub doc: Option<String>,
    /// The `password` flow: its `tokenUrl`.
    pub password_url: Option<String>,
    /// The `authorizationCode` flow as (`authorizationUrl`, `tokenUrl`).
    pub code: Option<(String, String)>,
    /// The `implicit` flow: its `authorizationUrl`.
    pub implicit_url: Option<String>,
    /// `refreshUrl` of any flow or else the first `tokenUrl`.
    pub refresh_url: Option<String>,
    /// `openIdConnectUrl` or 3.2 `oauth2MetadataUrl`: where metadata is discovered.
    pub discovery_url: Option<String>,
    /// `OAuth2` scopes of every flow as (name, description). They are sorted without repeats.
    pub scopes: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A security scheme reduced to how credentials travel.
pub(crate) enum SchemeKind {
    /// Header / query / cookie API key.
    ApiKey {
        /// `query`, `header` or `cookie`.
        location: String,
        /// Parameter name.
        param: String,
    },
    /// HTTP basic authentication.
    Basic,
    /// `http bearer`, `OAuth2` and `OpenID` Connect use a bearer token.
    Bearer,
    /// `http` with another scheme: raw `Authorization: <scheme> <credential>`.
    OtherHttp(String),
    /// mutualTLS: handled by the transport.
    MutualTls,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One scheme of a security requirement with the scopes that it must grant.
pub(crate) struct Requirement {
    /// Key in `components.securitySchemes`.
    pub scheme: String,
    /// `OAuth2` / `OpenID` scopes (or roles for other schemes since 3.1).
    pub scopes: Vec<String>,
}

/// Returns every scheme in `components.securitySchemes`.
/// Two names that differ only in case or separators (`api-key`, `api_key`) are an error: numbering them would follow
/// the document order, so a reorder would swap their validators.
pub(crate) fn schemes(b: &mut Builder<'_>) -> Vec<SecurityScheme> {
    let root = &b.doc.root;
    let Some(Value::Object(defs)) = root.get("components").and_then(|c| c.get("securitySchemes")) else {
        return vec![];
    };
    let mut out = vec![];
    for (name, s) in defs {
        let Some((s, _)) = resolve_value(b, s, &format!("/components/securitySchemes/{}", escape_token(name))) else {
            continue;
        };
        let kind = match s.get("type").and_then(Value::as_str) {
            Some("apiKey") => SchemeKind::ApiKey {
                location: s.get("in").and_then(Value::as_str).unwrap_or("header").to_owned(),
                param: s.get("name").and_then(Value::as_str).unwrap_or("api_key").to_owned(),
            },
            // The scheme is case-insensitive (RFC 9110 section 11.1), but a server may compare it as written. So
            // `OtherHttp` keeps the spelling of the description.
            Some("http") => match s.get("scheme").and_then(Value::as_str) {
                Some(b) if b.eq_ignore_ascii_case("basic") => SchemeKind::Basic,
                Some(b) if b.eq_ignore_ascii_case("bearer") => SchemeKind::Bearer,
                None => SchemeKind::Bearer,
                Some(other) => SchemeKind::OtherHttp(other.to_owned()),
            },
            Some("oauth2" | "openIdConnect") => SchemeKind::Bearer,
            Some("mutualTLS") => SchemeKind::MutualTls,
            _ => continue,
        };
        let token_url = s.pointer("/flows/clientCredentials/tokenUrl").and_then(Value::as_str).map(String::from);
        let device = s.pointer("/flows/deviceAuthorization").and_then(|f| {
            let url = f.get("deviceAuthorizationUrl")?.as_str()?;
            Some((url.to_owned(), f.get("tokenUrl")?.as_str()?.to_owned()))
        });
        let url = |p: &str| s.pointer(p).and_then(Value::as_str).map(String::from);
        let password_url = url("/flows/password/tokenUrl");
        let code = url("/flows/authorizationCode/authorizationUrl").zip(url("/flows/authorizationCode/tokenUrl"));
        let implicit_url = url("/flows/implicit/authorizationUrl");
        let refresh_url = ["clientCredentials", "authorizationCode", "password", "deviceAuthorization", "implicit"]
            .iter()
            .find_map(|f| url(&format!("/flows/{f}/refreshUrl")))
            .or_else(|| token_url.clone())
            .or_else(|| code.as_ref().map(|(_, t)| t.clone()))
            .or_else(|| password_url.clone());
        let discovery_url = url("/openIdConnectUrl").or_else(|| url("/oauth2MetadataUrl"));
        out.push(SecurityScheme {
            password_url,
            code,
            implicit_url,
            refresh_url,
            discovery_url,
            name: name.clone(),
            kind,
            token_url,
            device,
            deprecated: s.get("deprecated") == Some(&Value::Bool(true)),
            doc: json::string(&s, "description"),
            scopes: {
                let mut scopes: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
                for flow in s.get("flows").and_then(Value::as_object).into_iter().flat_map(|f| f.values()) {
                    for (name, d) in flow.get("scopes").and_then(Value::as_object).into_iter().flatten() {
                        scopes.entry(name.clone()).or_insert_with(|| d.as_str().unwrap_or_default().to_owned());
                    }
                }
                scopes.into_iter().collect()
            },
        });
    }
    let mut seen: std::collections::BTreeMap<String, &str> = std::collections::BTreeMap::new();
    for s in &out {
        if let Some(other) = seen.insert(Name::new(&s.name).key(), &s.name) {
            let mut pair = [other, s.name.as_str()];
            pair.sort_unstable();
            let [first, second] = pair;
            b.diags.error(
                format!("/components/securitySchemes/{}", escape_token(second)),
                format!("security schemes `{first}` and `{second}` {}: rename one", crate::diag::SAME_ITEM),
            );
        }
    }
    out
}

/// Reports two scopes of one scheme that are one variant of its scope enum (`read:user` and `read_user`). Numbering them
/// would follow the sorted list, so adding `read-user` would move the others by one and a `verify_` method that grants
/// one scope would grant another.
pub(crate) fn one_variant_per_scope(b: &mut Builder<'_>, schemes: &[SecurityScheme], used: &[&Requirement]) {
    for scheme in schemes {
        let used = used.iter().filter(|r| r.scheme == scheme.name);
        let names: std::collections::BTreeSet<&str> = scheme
            .scopes
            .iter()
            .map(|(n, _)| n.as_str())
            .chain(used.flat_map(|r| r.scopes.iter().map(String::as_str)))
            .collect();
        let mut seen: std::collections::BTreeMap<String, &str> = std::collections::BTreeMap::new();
        for name in names {
            if let Some(first) = seen.insert(Name::new(name).key(), name) {
                b.diags.error(
                    format!("/components/securitySchemes/{}", escape_token(&scheme.name)),
                    format!("the scopes `{first}` and `{name}` {} (`{}`)", crate::diag::SAME_VARIANT, scheme.name),
                );
            }
        }
    }
}

/// Returns the requirements of scheme sets. An empty requirement (`{}`) makes
/// authentication optional. A requirement that refers to an unknown scheme can never
/// be met. So it is dropped and never shortened. Shortening `{unknown}` to `{}`
/// would make the operation public. When every requirement is dropped the
/// result is empty. That counts as public. So `generate` refuses to emit a
/// server for such a document (`resolve::unresolved_security`).
pub(crate) fn requirements(security: Option<&Value>, schemes: &[SecurityScheme]) -> Vec<Vec<Requirement>> {
    let Some(Value::Array(reqs)) = security else { return vec![] };
    let alts: Vec<Vec<Requirement>> = reqs
        .iter()
        .filter_map(Value::as_object)
        .filter(|r| r.keys().all(|k| schemes.iter().any(|s| &s.name == k)))
        .map(|r| {
            r.iter()
                .map(|(k, v)| Requirement {
                    scheme: k.clone(),
                    scopes: v.as_array().map(|a| json::strings(a)).unwrap_or_default(),
                })
                .collect()
        })
        .collect();
    // An empty requirement makes credentials optional. The others stay. So the server still receives the
    // credentials that a client sends.
    if alts.iter().all(Vec::is_empty) { vec![] } else { alts }
}
