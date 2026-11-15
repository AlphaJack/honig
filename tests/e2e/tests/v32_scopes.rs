//! Roles on a bearer token (3.1+ Security Requirement role names) in the way that a
//! fine-grained personal access token carries permissions. Requirements are
//! OR. A token that holds either permission passes. The filter layer
//! matches the typed `Scope` exhaustively.

use std::future::Future;

use e2e::v32_scopes::api::{
    self, operations as ops,
    server::{self, Credential, Scope},
};

fn ok<T: Send>(r: T) -> impl Future<Output = Result<T, api::ApiError>> + Send {
    std::future::ready(Ok(r))
}

/// The token text names the permissions it holds: `Bearer read+write`.
struct Server;

// ANCHOR: scopes
/// Checks the permissions the operation requires: every request runs through it.
fn permitted(req: &api::Request) -> Result<(), api::ApiError> {
    let token = server::credentials(req).into_iter().map(|Credential::Pat(t)| t.to_owned()).next();
    let held = |s: Scope| {
        let t = token.as_deref().unwrap_or("");
        match s {
            Scope::IssuesRead => t.split('+').any(|p| p == "read"),
            Scope::IssuesWrite => t.split('+').any(|p| p == "write"),
        }
    };
    if server::scopes_held(req, held) {
        Ok(())
    } else {
        Err(api::ApiError::new(api::http::StatusCode::FORBIDDEN, "missing permission"))
    }
}
// ANCHOR_END: scopes

impl api::Api for Server {
    type Principal = ();

    fn list_issues(
        &self,
        _: ops::list_issues::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::list_issues::Response, api::ApiError>> + Send {
        ok(ops::list_issues::Response::NoContent)
    }
    fn create_issue(
        &self,
        _: ops::create_issue::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::create_issue::Response, api::ApiError>> + Send {
        ok(ops::create_issue::Response::NoContent)
    }
    fn open(
        &self,
        _: ops::open::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::open::Response, api::ApiError>> + Send {
        ok(ops::open::Response::NoContent)
    }
}

fn send(method: &'static str, path: &str, token: Option<&str>) -> u16 {
    let mut req = api::runtime::new_request(method, path.to_owned(), vec![]).unwrap();
    if let Some(t) = token {
        req.headers_mut().append("authorization", honig::http::HeaderValue::try_from(format!("Bearer {t}")).unwrap());
    }
    api::block_on(
        api::Server::new(Server)
            .operation_layer(tower::filter::AsyncFilterLayer::new(|req: api::Request| {
                std::future::ready(permitted(&req).map(|()| req))
            }))
            .into_service()
            .handle(req),
    )
    .status()
    .as_u16()
}

#[test]
fn either_permission_passes_an_or_requirement() {
    assert_eq!(send("GET", "/issues", Some("read")), 204);
    assert_eq!(send("GET", "/issues", Some("write")), 204, "the second requirement counts too");
    assert_eq!(send("GET", "/issues", Some("admin")), 403);
}

#[test]
fn a_single_requirement_needs_its_permission() {
    assert_eq!(send("POST", "/issues", Some("write")), 204);
    assert_eq!(send("POST", "/issues", Some("read")), 403);
}

#[test]
fn a_missing_token_is_401_before_any_layer() {
    assert_eq!(send("GET", "/issues", None), 401);
}

#[test]
fn an_operation_without_security_needs_no_scope() {
    assert_eq!(send("GET", "/public", None), 204);
}

#[test]
fn scopes_roundtrip_their_names() {
    for s in [Scope::IssuesRead, Scope::IssuesWrite] {
        assert_eq!(Scope::from_name(s.as_str()), Some(s));
    }
    assert_eq!(Scope::from_name("issues:admin"), None);
}

#[test]
fn alternatives_are_typed() {
    let mut req = api::runtime::new_request("GET", "/issues".to_owned(), vec![]).unwrap();
    req.headers_mut().append("authorization", honig::http::HeaderValue::try_from("Bearer x".to_owned()).unwrap());
    let satisfied = api::runtime::check_auth(
        req.headers(),
        &[],
        &[
            &[(api::runtime::Scheme::Bearer("pat"), &["issues:read"])],
            &[(api::runtime::Scheme::Bearer("pat"), &["issues:write"])],
        ],
    )
    .expect("token present");
    let event = api::RequestEvent::new(Some(api::Operation::ListIssues), "GET");
    let ctx = api::Context::new(&req, api::Operation::ListIssues, satisfied, &event);
    assert_eq!(server::scope_alternatives(&ctx), [vec![Scope::IssuesRead], vec![Scope::IssuesWrite]]);
}

/// Names who is calling: the text of the token or nobody for `Bearer anon`.
struct Named;

/// The token's text as the principal; `anon` is a token that names nobody.
fn named(req: &api::Request) -> Option<String> {
    let token = server::credentials(req).into_iter().map(|Credential::Pat(t)| t.to_owned()).next();
    token.filter(|t| t != "anon")
}

impl api::Api for Named {
    type Principal = String;

    fn list_issues(
        &self,
        _: ops::list_issues::Params,
        ctx: &api::Context<'_, String>,
    ) -> impl Future<Output = Result<ops::list_issues::Response, api::ApiError>> + Send {
        // A secured operation sees the principal itself and not an `Option`.
        assert!(!ctx.principal.is_empty());
        ok(ops::list_issues::Response::NoContent)
    }
    fn create_issue(
        &self,
        _: ops::create_issue::Params,
        _: &api::Context<'_, String>,
    ) -> impl Future<Output = Result<ops::create_issue::Response, api::ApiError>> + Send {
        ok(ops::create_issue::Response::NoContent)
    }
    fn open(
        &self,
        _: ops::open::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::open::Response, api::ApiError>> + Send {
        // An operation without `security` sees no principal.
        ok(ops::open::Response::NoContent)
    }
}

fn send_named(path: &str, token: Option<&str>) -> u16 {
    let mut req = api::runtime::new_request("GET", path.to_owned(), vec![]).unwrap();
    if let Some(t) = token {
        req.headers_mut().append("authorization", honig::http::HeaderValue::try_from(format!("Bearer {t}")).unwrap());
    }
    let service =
        api::Server::new(Named).authenticate(|req: &api::Request| std::future::ready(Ok(named(req)))).into_service();
    api::block_on(service.handle(req)).status().as_u16()
}

#[test]
fn each_operation_takes_the_caller_its_security_asks_for() {
    assert_eq!(send_named("/issues", Some("ann")), 204, "a principal reaches a secured operation");
    assert_eq!(send_named("/issues", Some("anon")), 401, "nobody does not");
    assert_eq!(send_named("/public", None), 204, "a public operation takes nobody");
}

/// A server whose handler refuses every token it sees: it was revoked.
struct Revoked;

impl api::Api for Revoked {
    type Principal = ();

    fn list_issues(
        &self,
        _: ops::list_issues::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::list_issues::Response, api::ApiError>> + Send {
        std::future::ready(Err(api::ApiError::new(api::http::StatusCode::UNAUTHORIZED, "the token was revoked")))
    }
    fn create_issue(
        &self,
        _: ops::create_issue::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::create_issue::Response, api::ApiError>> + Send {
        ok(ops::create_issue::Response::NoContent)
    }
    fn open(
        &self,
        _: ops::open::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::open::Response, api::ApiError>> + Send {
        ok(ops::open::Response::NoContent)
    }
}

/// RFC 9110 section 15.5.2: a 401 carries a challenge for the operation's schemes. That holds for the one the server
/// sends without a token and for the one a handler returns for a token it refuses.
#[test]
fn every_401_carries_a_challenge() {
    let challenge = |token: Option<&str>| {
        let mut req = api::runtime::new_request("GET", "/issues".to_owned(), vec![]).unwrap();
        if let Some(t) = token {
            req.headers_mut()
                .append("Authorization", honig::http::HeaderValue::try_from(format!("Bearer {t}")).unwrap());
        }
        let resp = api::block_on(api::Server::new(Revoked).into_service().handle(req));
        (
            resp.status().as_u16(),
            resp.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).map(str::to_owned),
        )
    };
    assert_eq!(challenge(None), (401, Some("Bearer".to_owned())), "no token");
    assert_eq!(challenge(Some("read")), (401, Some("Bearer".to_owned())), "a token the handler refuses");
}
