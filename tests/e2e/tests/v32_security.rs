//! Security per operation, scope and optionality, offline. An identity provider in memory answers the `verify_` method
//! of each scheme. A fixed table states what each case answers. A reference model of the rules then predicts the
//! server for any operation, any set of credentials and any scopes, and a property test compares the two.
//!
//! The rules: requirements are OR of ANDs. An empty alternative (`{}`) lets an anonymous request in. A credential that
//! the `Api` refuses is 401, also on an operation that takes anonymous requests. A missing scope is 403. An operation
//! with `security: []` needs nothing.

use std::sync::{Arc, Mutex};

use e2e::v32_security::api::{
    self, operations as ops,
    runtime::Operation as _,
    server::{OauthScope, OidcScope},
};
use proptest::prelude::*;

/// What a handler saw of who is calling.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    /// An operation without `security`: no principal at all.
    Public,
    /// An operation with an empty alternative and no credential that passed.
    Anonymous,
    /// The principal: the scheme that passed first in its alternative and the subject.
    Principal(String),
}

/// What a request gets: the status, and what the handler saw when it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Ran(Seen),
    Refused(u16),
}

/// The `Authorization` header: one credential at most, of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Auth {
    None,
    Bearer(&'static str),
    Basic(&'static str),
}

/// The credentials that a request presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Creds {
    auth: Auth,
    key: Option<&'static str>,
    query_key: Option<&'static str>,
    session: Option<&'static str>,
    /// What the TLS terminator says of the client certificate (`x-client-verify`).
    cert: Option<&'static str>,
}

const NONE: Creds = Creds { auth: Auth::None, key: None, query_key: None, session: None, cert: None };

/// What the identity provider says of a credential for a scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    Absent,
    Refused,
    Valid { principal: String, scopes: Vec<&'static str> },
}

/// The identity provider: the subject and scopes that each credential stands for. A token of one scheme is not valid
/// for another, also when both read `Authorization: Bearer`.
///
/// - `bearer`: `b-<sub>`
/// - `oauth`: `o-<scope>+<scope>`, for the client `app`
/// - `oidc`: `i-<scope>+<scope>`, for the user `ann`
/// - `key`, `query_key`, `session`: `k-<sub>`, `q-<sub>`, `s-<sub>`
/// - `login`: `ann:pw`
/// - `cert`: `SUCCESS`
fn verdict(scheme: &str, creds: &Creds) -> Verdict {
    let token = |prefix: &str, principal: &dyn Fn(&str) -> String, scoped: bool| match creds.auth {
        Auth::Bearer(t) => match t.strip_prefix(prefix) {
            Some(rest) if scoped => {
                let scopes = rest.split('+').filter(|s| !s.is_empty()).collect();
                Verdict::Valid { principal: principal(rest), scopes }
            }
            Some(rest) => Verdict::Valid { principal: principal(rest), scopes: vec![] },
            None => Verdict::Refused,
        },
        Auth::None | Auth::Basic(_) => Verdict::Absent,
    };
    let keyed = |value: Option<&str>, prefix: &str| match value {
        None => Verdict::Absent,
        Some(v) => v
            .strip_prefix(prefix)
            .map_or(Verdict::Refused, |sub| Verdict::Valid { principal: format!("{scheme}:{sub}"), scopes: vec![] }),
    };
    match scheme {
        "bearer" => token("b-", &|sub| format!("bearer:{sub}"), false),
        "oauth" => token("o-", &|_| "oauth:app".to_owned(), true),
        "oidc" => token("i-", &|_| "oidc:ann".to_owned(), true),
        "key" => keyed(creds.key, "k-"),
        "query_key" => keyed(creds.query_key, "q-"),
        "session" => keyed(creds.session, "s-"),
        "login" => match creds.auth {
            Auth::Basic("ann:pw") => Verdict::Valid { principal: "login:ann".to_owned(), scopes: vec![] },
            Auth::Basic(_) => Verdict::Refused,
            Auth::None | Auth::Bearer(_) => Verdict::Absent,
        },
        // A mutualTLS scheme has no credential in the request: its method always decides.
        "cert" if creds.cert == Some("SUCCESS") => {
            Verdict::Valid { principal: "cert:client".to_owned(), scopes: vec![] }
        }
        "cert" => Verdict::Refused,
        other => panic!("no scheme {other}"),
    }
}

/// The reference model: what the server answers `creds` on an operation with these requirements.
fn expected(requirements: &[&[api::runtime::Requirement]], creds: &Creds) -> Outcome {
    if requirements.is_empty() {
        return Outcome::Ran(Seen::Public);
    }
    let (mut refused, mut lacking) = (false, false);
    'alternative: for alt in requirements.iter().filter(|alt| !alt.is_empty()) {
        let mut principal = None;
        for (scheme, required) in *alt {
            match verdict(scheme.name(), creds) {
                Verdict::Absent => continue 'alternative,
                Verdict::Refused => {
                    refused = true;
                    continue 'alternative;
                }
                Verdict::Valid { scopes, .. } if !required.iter().all(|s| scopes.contains(s)) => {
                    lacking = true;
                    continue 'alternative;
                }
                Verdict::Valid { principal: p, .. } => {
                    principal.get_or_insert(p);
                }
            }
        }
        return Outcome::Ran(principal.map_or(Seen::Anonymous, Seen::Principal));
    }
    if lacking {
        return Outcome::Refused(403);
    }
    if !refused && requirements.iter().any(|alt| alt.is_empty()) {
        return Outcome::Ran(Seen::Anonymous);
    }
    Outcome::Refused(401)
}

/// The server: each `verify_` method asks the identity provider. Each handler records what it saw.
#[derive(Default)]
struct Server {
    seen: Mutex<Option<Seen>>,
}

type Answer<T> = Result<T, api::ApiError>;

impl Server {
    fn record(&self, seen: Seen) {
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(seen);
        }
    }

    fn check(scheme: &str, request: &api::Request) -> Option<String> {
        match verdict(scheme, &creds_of(request)) {
            Verdict::Valid { principal, .. } => Some(principal),
            Verdict::Absent | Verdict::Refused => None,
        }
    }

    fn scopes<S>(scheme: &str, request: &api::Request, from: fn(&str) -> Option<S>) -> Option<api::Granted<String, S>> {
        match verdict(scheme, &creds_of(request)) {
            Verdict::Valid { principal, scopes } => {
                Some(api::Granted { principal, scopes: scopes.into_iter().filter_map(from).collect() })
            }
            Verdict::Absent | Verdict::Refused => None,
        }
    }
}

impl api::Api for Server {
    type Principal = String;

    async fn verify_bearer(&self, _: &str, request: &api::Request) -> Answer<Option<String>> {
        Ok(Self::check("bearer", request))
    }
    async fn verify_oauth(&self, _: &str, request: &api::Request) -> Answer<Option<api::Granted<String, OauthScope>>> {
        Ok(Self::scopes("oauth", request, OauthScope::from_name))
    }
    async fn verify_oidc(&self, _: &str, request: &api::Request) -> Answer<Option<api::Granted<String, OidcScope>>> {
        Ok(Self::scopes("oidc", request, OidcScope::from_name))
    }
    async fn verify_key(&self, _: &str, request: &api::Request) -> Answer<Option<String>> {
        Ok(Self::check("key", request))
    }
    async fn verify_query_key(&self, _: &str, request: &api::Request) -> Answer<Option<String>> {
        Ok(Self::check("query_key", request))
    }
    async fn verify_session(&self, _: &str, request: &api::Request) -> Answer<Option<String>> {
        Ok(Self::check("session", request))
    }
    async fn verify_login(&self, _: api::Basic<'_>, request: &api::Request) -> Answer<Option<String>> {
        Ok(Self::check("login", request))
    }
    async fn verify_cert(&self, request: &api::Request) -> Answer<Option<String>> {
        Ok(Self::check("cert", request))
    }

    async fn public(&self, _: ops::public::Params, _: &api::Context<'_>) -> Answer<ops::public::Response> {
        self.record(Seen::Public);
        Ok(ops::public::Response::NoContent)
    }
    async fn me(&self, _: ops::me::Params, ctx: &api::Context<'_, String>) -> Answer<ops::me::Response> {
        self.record(Seen::Principal(ctx.principal.clone()));
        Ok(ops::me::Response::NoContent)
    }
    async fn optional(
        &self,
        _: ops::optional::Params,
        ctx: &api::Context<'_, Option<String>>,
    ) -> Answer<ops::optional::Response> {
        self.record(ctx.principal.clone().map_or(Seen::Anonymous, Seen::Principal));
        Ok(ops::optional::Response::NoContent)
    }
    async fn read(&self, _: ops::read::Params, ctx: &api::Context<'_, String>) -> Answer<ops::read::Response> {
        self.record(Seen::Principal(ctx.principal.clone()));
        Ok(ops::read::Response::NoContent)
    }
    async fn write(&self, _: ops::write::Params, ctx: &api::Context<'_, String>) -> Answer<ops::write::Response> {
        self.record(Seen::Principal(ctx.principal.clone()));
        Ok(ops::write::Response::NoContent)
    }
    async fn both(&self, _: ops::both::Params, ctx: &api::Context<'_, String>) -> Answer<ops::both::Response> {
        self.record(Seen::Principal(ctx.principal.clone()));
        Ok(ops::both::Response::NoContent)
    }
    async fn login(&self, _: ops::login::Params, ctx: &api::Context<'_, String>) -> Answer<ops::login::Response> {
        self.record(Seen::Principal(ctx.principal.clone()));
        Ok(ops::login::Response::NoContent)
    }
    async fn cert(&self, _: ops::cert::Params, ctx: &api::Context<'_, String>) -> Answer<ops::cert::Response> {
        self.record(Seen::Principal(ctx.principal.clone()));
        Ok(ops::cert::Response::NoContent)
    }
    async fn oidc(&self, _: ops::oidc::Params, ctx: &api::Context<'_, String>) -> Answer<ops::oidc::Response> {
        self.record(Seen::Principal(ctx.principal.clone()));
        Ok(ops::oidc::Response::NoContent)
    }
}

/// Reads back the credentials that `request` carries, as the identity provider sees them.
fn creds_of(request: &api::Request) -> Creds {
    let header = |name: &str| request.headers().get(name).and_then(|v| v.to_str().ok()).map(leak);
    let auth = match header("authorization") {
        Some(a) => match a.split_once(' ') {
            Some(("Bearer", t)) => Auth::Bearer(t),
            Some(("Basic", b)) => {
                let bytes = api::runtime::unbase64(b).unwrap_or_default();
                Auth::Basic(leak(&String::from_utf8(bytes).unwrap_or_default()))
            }
            _ => Auth::None,
        },
        None => Auth::None,
    };
    let query = request.uri().query().unwrap_or_default();
    let query_key = query.split('&').find_map(|kv| kv.strip_prefix("key=")).map(leak);
    let session = header("cookie").and_then(|c| c.strip_prefix("session=")).map(leak);
    Creds { auth, key: header("x-key"), query_key, session, cert: header("x-client-verify") }
}

/// The few strings of a test case live as long as the test.
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

/// The operations of the fixture: (operation, method, path).
const OPS: [(api::Operation, &str, &str); 9] = [
    (api::Operation::Public, "GET", "/public"),
    (api::Operation::Me, "GET", "/me"),
    (api::Operation::Optional, "GET", "/optional"),
    (api::Operation::Read, "GET", "/read"),
    (api::Operation::Write, "POST", "/write"),
    (api::Operation::Both, "GET", "/both"),
    (api::Operation::Login, "GET", "/login"),
    (api::Operation::Cert, "GET", "/cert"),
    (api::Operation::Oidc, "GET", "/oidc"),
];

/// Sends `creds` to the operation at `path` and returns what the server did.
fn send(method: &'static str, path: &str, creds: &Creds) -> Outcome {
    let target = creds.query_key.map_or_else(|| path.to_owned(), |k| format!("{path}?key={k}"));
    let mut req = api::runtime::new_request(method, target, vec![]).unwrap();
    let mut set = |name: &'static str, value: String| {
        req.headers_mut().append(name, honig::http::HeaderValue::try_from(value).unwrap());
    };
    match creds.auth {
        Auth::None => {}
        Auth::Bearer(t) => set("authorization", format!("Bearer {t}")),
        Auth::Basic(login) => set("authorization", format!("Basic {}", api::runtime::base64(login.as_bytes()))),
    }
    if let Some(k) = creds.key {
        set("x-key", k.to_owned());
    }
    if let Some(s) = creds.session {
        set("cookie", format!("session={s}"));
    }
    if let Some(c) = creds.cert {
        set("x-client-verify", c.to_owned());
    }
    let server = Arc::new(Server::default());
    let resp = api::block_on(api::Server::new(Arc::clone(&server)).into_service().handle(req));
    let seen = server.seen.lock().unwrap().clone();
    match (resp.status().as_u16(), seen) {
        (204, Some(seen)) => Outcome::Ran(seen),
        (status, None) => Outcome::Refused(status),
        (status, Some(seen)) => panic!("{status} after the handler saw {seen:?}"),
    }
}

fn principal(p: &str) -> Outcome {
    Outcome::Ran(Seen::Principal(p.to_owned()))
}

/// Each rule on the operation that shows it, with the answer stated by hand.
#[test]
fn each_rule_answers_as_stated() {
    let bearer = |t| Creds { auth: Auth::Bearer(t), ..NONE };
    let cases: &[(&str, &str, Creds, Outcome)] = &[
        ("GET", "/public", NONE, Outcome::Ran(Seen::Public)),
        ("GET", "/public", bearer("garbage"), Outcome::Ran(Seen::Public)),
        ("GET", "/me", NONE, Outcome::Refused(401)),
        ("GET", "/me", bearer("b-ann"), principal("bearer:ann")),
        ("GET", "/me", bearer("o-read"), Outcome::Refused(401)),
        ("GET", "/optional", NONE, Outcome::Ran(Seen::Anonymous)),
        ("GET", "/optional", bearer("b-ann"), principal("bearer:ann")),
        ("GET", "/optional", bearer("forged"), Outcome::Refused(401)),
        ("GET", "/read", bearer("o-read"), principal("oauth:app")),
        ("GET", "/read", bearer("o-write"), Outcome::Refused(403)),
        ("GET", "/read", Creds { key: Some("k-bob"), ..bearer("o-write") }, principal("key:bob")),
        ("POST", "/write", bearer("o-read"), Outcome::Refused(403)),
        ("POST", "/write", bearer("o-read+write"), principal("oauth:app")),
        ("GET", "/both", Creds { key: Some("k-bob"), session: Some("s-bob"), ..NONE }, principal("key:bob")),
        ("GET", "/both", Creds { key: Some("forged"), session: Some("s-bob"), ..NONE }, Outcome::Refused(401)),
        ("GET", "/both", Creds { key: Some("k-bob"), ..NONE }, Outcome::Refused(401)),
        ("GET", "/login", Creds { auth: Auth::Basic("ann:pw"), ..NONE }, principal("login:ann")),
        ("GET", "/login", Creds { auth: Auth::Basic("ann:guess"), ..NONE }, Outcome::Refused(401)),
        (
            "GET",
            "/login",
            Creds { auth: Auth::Basic("ann:guess"), query_key: Some("q-cat"), ..NONE },
            principal("query_key:cat"),
        ),
        ("GET", "/cert", NONE, Outcome::Refused(401)),
        ("GET", "/cert", Creds { cert: Some("SUCCESS"), ..NONE }, principal("cert:client")),
        ("GET", "/oidc", bearer("i-profile"), principal("oidc:ann")),
        ("GET", "/oidc", bearer("i-email"), Outcome::Refused(403)),
    ];
    for (method, path, creds, want) in cases {
        assert_eq!(&send(method, path, creds), want, "{method} {path} with {creds:?}");
        let op = OPS.iter().find(|(_, _, p)| p == path).unwrap().0;
        assert_eq!(&expected(op.op().security, creds), want, "the model on {method} {path} with {creds:?}");
    }
}

fn auth() -> impl Strategy<Value = Auth> {
    prop_oneof![
        Just(Auth::None),
        prop::sample::select(vec!["b-ann", "b-bob", "o-read", "o-write", "o-read+write", "i-profile", "i-email", "x"])
            .prop_map(Auth::Bearer),
        prop::sample::select(vec!["ann:pw", "ann:guess", "bob:pw"]).prop_map(Auth::Basic),
    ]
}

fn value(valid: &'static str) -> impl Strategy<Value = Option<&'static str>> {
    prop::option::of(prop::sample::select(vec![valid, "forged"]))
}

fn creds() -> impl Strategy<Value = Creds> {
    (
        auth(),
        value("k-bob"),
        value("q-cat"),
        value("s-bob"),
        prop::option::of(prop::sample::select(vec!["SUCCESS", "FAILED"])),
    )
        .prop_map(|(auth, key, query_key, session, cert)| Creds { auth, key, query_key, session, cert })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    /// Any operation, any credentials: the server answers as the model.
    #[test]
    fn the_server_answers_as_the_model(at in 0..OPS.len(), creds in creds()) {
        let (op, method, path) = OPS[at];
        prop_assert_eq!(send(method, path, &creds), expected(op.op().security, &creds), "{} {} with {:?}", method, path, creds);
    }
}
