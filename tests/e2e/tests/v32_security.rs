//! Security per operation, scope and optionality, offline. An identity provider in memory answers the `verify_` method
//! of each scheme. A fixed table states what each case answers. A reference model of the rules then predicts the
//! server for any operation, any set of credentials and any scopes, and a property test compares the two.
//!
//! The rules: requirements are OR of ANDs. An empty alternative (`{}`) lets an anonymous request in. A credential that
//! the `Api` refuses is 401, also on an operation that accepts anonymous requests. A missing scope is 403. An operation
//! with `security: []` needs nothing.

use std::sync::{Arc, Mutex};

use e2e::v32_security::api::{
    self, operations as ops,
    server::{OauthScope, OidcScope},
};
use honig::Operation as _;
use proptest::prelude::*;

/// What a handler saw of who is calling.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
    /// An operation without `security`: no principal at all.
    Public,
    /// An operation with an empty alternative and no credential that passed.
    Anonymous,
    /// Who is calling: the scheme and the subject.
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
    Valid { value: String, scopes: Vec<&'static str> },
}

/// The identity provider: the subject and scopes that each credential stands for. A token of one scheme is not valid
/// for another, also when both use `Authorization: Bearer`.
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
                Verdict::Valid { value: principal(rest), scopes }
            }
            Some(rest) => Verdict::Valid { value: principal(rest), scopes: vec![] },
            None => Verdict::Refused,
        },
        Auth::None | Auth::Basic(_) => Verdict::Absent,
    };
    let keyed = |value: Option<&str>, prefix: &str| match value {
        None => Verdict::Absent,
        Some(v) => v
            .strip_prefix(prefix)
            .map_or(Verdict::Refused, |sub| Verdict::Valid { value: format!("{scheme}:{sub}"), scopes: vec![] }),
    };
    match scheme {
        "bearer" => token("b-", &|sub| format!("bearer:{sub}"), false),
        "oauth" => token("o-", &|_| "oauth:app".to_owned(), true),
        "oidc" => token("i-", &|_| "oidc:ann".to_owned(), true),
        "key" => keyed(creds.key, "k-"),
        "query_key" => keyed(creds.query_key, "q-"),
        "session" => keyed(creds.session, "s-"),
        "login" => match creds.auth {
            Auth::Basic("ann:pw") => Verdict::Valid { value: "login:ann".to_owned(), scopes: vec![] },
            Auth::Basic(_) => Verdict::Refused,
            Auth::None | Auth::Bearer(_) => Verdict::Absent,
        },
        // A mutualTLS scheme has no credential in the request: its method always decides.
        "cert" if creds.cert == Some("SUCCESS") => Verdict::Valid { value: "cert:client".to_owned(), scopes: vec![] },
        "cert" => Verdict::Refused,
        other => panic!("no scheme {other}"),
    }
}

/// The reference model: what the server answers `creds` on an operation with these requirements.
fn expected(requirements: &[&[honig::__private::Requirement]], creds: &Creds) -> Outcome {
    if requirements.is_empty() {
        return Outcome::Ran(Seen::Public);
    }
    let (mut refused, mut lacking) = (false, false);
    'alternative: for alt in requirements.iter().filter(|alt| !alt.is_empty()) {
        let mut values = vec![];
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
                Verdict::Valid { value, .. } => values.push(value),
            }
        }
        // The handler of an alternative of several schemes refuses values of two subjects.
        let subject = |v: &String| v.split_once(':').map(|(_, who)| who.to_owned());
        if values.iter().any(|v| subject(v) != values.first().and_then(subject)) {
            return Outcome::Refused(401);
        }
        return Outcome::Ran(values.into_iter().next().map_or(Seen::Anonymous, Seen::Principal));
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
    /// The credentials that the handler of `read` found with `credentials`.
    presented: Mutex<Vec<String>>,
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
            Verdict::Valid { value, .. } => Some(value),
            Verdict::Absent | Verdict::Refused => None,
        }
    }

    fn scopes<S>(
        scheme: &str,
        request: &api::Request,
        from: fn(&str) -> Option<S>,
    ) -> Option<honig::Granted<String, S>> {
        match verdict(scheme, &creds_of(request)) {
            Verdict::Valid { value, scopes } => {
                Some(honig::Granted { value, scopes: scopes.into_iter().filter_map(from).collect() })
            }
            Verdict::Absent | Verdict::Refused => None,
        }
    }
}

impl api::Api for Server {
    type Bearer = String;
    type Oauth = String;
    type Oidc = String;
    type Key = String;
    type QueryKey = String;
    type Session = String;
    type Login = String;
    type Cert = String;

    async fn verify_bearer(&self, _: &str, request: &api::Request) -> Answer<Option<String>> {
        Ok(Self::check("bearer", request))
    }
    async fn verify_oauth(
        &self,
        _: &str,
        request: &api::Request,
    ) -> Answer<Option<honig::Granted<String, OauthScope>>> {
        Ok(Self::scopes("oauth", request, OauthScope::from_name))
    }
    async fn verify_oidc(&self, _: &str, request: &api::Request) -> Answer<Option<honig::Granted<String, OidcScope>>> {
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
    async fn me(
        &self,
        _: ops::me::Params,
        ctx: &api::Context<'_, ops::me::Security<Self>>,
    ) -> Answer<ops::me::Response> {
        let ops::me::Security::Bearer(who) = &ctx.security;
        self.record(Seen::Principal(who.clone()));
        Ok(ops::me::Response::NoContent)
    }
    async fn optional(
        &self,
        _: ops::optional::Params,
        ctx: &api::Context<'_, ops::optional::Security<Self>>,
    ) -> Answer<ops::optional::Response> {
        self.record(match &ctx.security {
            ops::optional::Security::Anonymous => Seen::Anonymous,
            ops::optional::Security::Bearer(who) => Seen::Principal(who.clone()),
        });
        Ok(ops::optional::Response::NoContent)
    }
    async fn read(
        &self,
        _: ops::read::Params,
        ctx: &api::Context<'_, ops::read::Security<Self>>,
    ) -> Answer<ops::read::Response> {
        let (ops::read::Security::Oauth(who) | ops::read::Security::Key(who)) = &ctx.security;
        self.record(Seen::Principal(who.clone()));
        *self.presented.lock().unwrap() = api::server::credentials(ctx).iter().map(|c| format!("{c:?}")).collect();
        Ok(ops::read::Response::NoContent)
    }
    async fn write(
        &self,
        _: ops::write::Params,
        ctx: &api::Context<'_, ops::write::Security<Self>>,
    ) -> Answer<ops::write::Response> {
        let ops::write::Security::Oauth(who) = &ctx.security;
        self.record(Seen::Principal(who.clone()));
        Ok(ops::write::Response::NoContent)
    }
    // ANCHOR: both
    async fn both(
        &self,
        _: ops::both::Params,
        ctx: &api::Context<'_, ops::both::Security<Self>>,
    ) -> Answer<ops::both::Response> {
        // Both credentials must be of one caller: a key of one and a session of another is no one.
        let ops::both::Security::KeyAndSession(key, session) = &ctx.security;
        let subject = |value: &str| value.split_once(':').map(|(_, who)| who.to_owned());
        if subject(key) != subject(session) {
            return Err(api::ApiError::new(
                honig::http::StatusCode::UNAUTHORIZED,
                "the key and the session are of two callers",
            ));
        }
        self.record(Seen::Principal(key.clone()));
        Ok(ops::both::Response::NoContent)
    }
    // ANCHOR_END: both
    async fn login(
        &self,
        _: ops::login::Params,
        ctx: &api::Context<'_, ops::login::Security<Self>>,
    ) -> Answer<ops::login::Response> {
        let (ops::login::Security::Login(who) | ops::login::Security::QueryKey(who)) = &ctx.security;
        self.record(Seen::Principal(who.clone()));
        Ok(ops::login::Response::NoContent)
    }
    async fn cert(
        &self,
        _: ops::cert::Params,
        ctx: &api::Context<'_, ops::cert::Security<Self>>,
    ) -> Answer<ops::cert::Response> {
        let ops::cert::Security::Cert(who) = &ctx.security;
        self.record(Seen::Principal(who.clone()));
        Ok(ops::cert::Response::NoContent)
    }
    async fn oidc(
        &self,
        _: ops::oidc::Params,
        ctx: &api::Context<'_, ops::oidc::Security<Self>>,
    ) -> Answer<ops::oidc::Response> {
        let ops::oidc::Security::Oidc(who) = &ctx.security;
        self.record(Seen::Principal(who.clone()));
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
                let bytes = honig::__private::unbase64(b).unwrap_or_default();
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
    let mut req = honig::__private::new_request(method, target, vec![]).unwrap();
    let mut set = |name: &'static str, value: String| {
        req.headers_mut().append(name, honig::http::HeaderValue::try_from(value).unwrap());
    };
    match creds.auth {
        Auth::None => {}
        Auth::Bearer(t) => set("authorization", format!("Bearer {t}")),
        Auth::Basic(login) => set("authorization", format!("Basic {}", honig::__private::base64(login.as_bytes()))),
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
    let resp = honig::block_on(api::Server::new(Arc::clone(&server)).into_service().handle(req));
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
        ("GET", "/both", Creds { key: Some("k-bob"), session: Some("s-eve"), ..NONE }, Outcome::Refused(401)),
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

/// A value of a scheme's type that sits in the extensions before the server, a request id stored as `String`, is not
/// the caller: an anonymous request reaches the handler as `Anonymous`.
#[test]
fn an_extension_of_a_scheme_type_is_not_the_caller() {
    let mut req = honig::__private::new_request("GET", "/optional".to_owned(), vec![]).unwrap();
    req.extensions_mut().insert("request-id-123".to_owned());
    let server = Arc::new(Server::default());
    let resp = honig::block_on(api::Server::new(Arc::clone(&server)).into_service().handle(req));
    assert_eq!(resp.status().as_u16(), 204);
    assert_eq!(server.seen.lock().unwrap().clone(), Some(Seen::Anonymous));
}

/// A handler gets the credentials that a `verify_` method accepted. A forged one beside a valid one of another
/// alternative is not among them.
#[test]
fn a_handler_gets_only_the_credentials_that_passed() {
    let read = |creds: Creds| {
        let mut req = honig::__private::new_request("GET", "/read".to_owned(), vec![]).unwrap();
        if let Auth::Bearer(t) = creds.auth {
            req.headers_mut()
                .append("authorization", honig::http::HeaderValue::try_from(format!("Bearer {t}")).unwrap());
        }
        if let Some(k) = creds.key {
            req.headers_mut().append("x-key", honig::http::HeaderValue::try_from(k).unwrap());
        }
        let server = Arc::new(Server::default());
        let resp = honig::block_on(api::Server::new(Arc::clone(&server)).into_service().handle(req));
        let presented = server.presented.lock().unwrap().clone();
        (resp.status().as_u16(), presented)
    };
    let valid_oauth = read(Creds { auth: Auth::Bearer("o-read"), key: Some("forged"), ..NONE });
    assert_eq!(valid_oauth, (204, vec![r#"Oauth("o-read")"#.to_owned()]));
    let valid_key = read(Creds { auth: Auth::Bearer("forged"), key: Some("k-bob"), ..NONE });
    assert_eq!(valid_key, (204, vec![r#"Key("k-bob")"#.to_owned()]));
}

// ANCHOR: optional-rate-limit
type Filter = std::future::Ready<Result<api::Request, api::ApiError>>;

/// Anonymous callers of `/optional` get at most `limit` requests. A caller whose token passed is never limited: the
/// layer runs after the server checked the credentials, so the alternative that passed is in the extensions. A real limiter keys the
/// anonymous count by the client's address (`honig::peer`) and a time window.
fn limit_anonymous(limit: u32) -> tower::filter::AsyncFilterLayer<impl Fn(api::Request) -> Filter + Clone> {
    let used = Arc::new(std::sync::atomic::AtomicU32::new(0));
    tower::filter::AsyncFilterLayer::new(move |req: api::Request| {
        let signed_in = req.extensions().get::<api::Passed<api::server::SchemeValue<Server>>>().is_some();
        let over = !signed_in && used.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= limit;
        let refusal = api::ApiError::new(honig::http::StatusCode::TOO_MANY_REQUESTS, "sign in for more requests");
        std::future::ready(if over { Err(refusal) } else { Ok(req) })
    })
}
// ANCHOR_END: optional-rate-limit

#[test]
fn anonymous_callers_are_limited_and_signed_in_ones_are_not() {
    let service = api::Server::new(Server::default()).operation_layer(limit_anonymous(2)).into_service();
    let get = |token: Option<&str>| {
        let mut req = honig::__private::new_request("GET", "/optional".to_owned(), vec![]).unwrap();
        if let Some(t) = token {
            req.headers_mut()
                .append("authorization", honig::http::HeaderValue::try_from(format!("Bearer {t}")).unwrap());
        }
        honig::block_on(service.handle(req)).status().as_u16()
    };
    assert_eq!([get(None), get(None), get(None)], [204, 204, 429], "the third anonymous request is over the limit");
    assert_eq!([get(Some("b-ann")), get(Some("b-ann"))], [204, 204], "a signed-in caller is not counted");
}
