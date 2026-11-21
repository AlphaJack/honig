//! Roles on a bearer token (3.1+ Security Requirement role names) in the way that a
//! fine-grained personal access token carries permissions. Requirements are
//! OR. A token that grants either permission passes. `verify_pat` returns the
//! permissions of a token as the typed `PatScope`, and the server checks them.

use std::future::Future;

use e2e::v32_scopes::api::{self, operations as ops, server::PatScope};

fn ok<T: Send>(r: T) -> impl Future<Output = Result<T, api::ApiError>> + Send {
    std::future::ready(Ok(r))
}

/// The token text lists the permissions it grants: `Bearer read+write`. `bad` is no token at all.
struct Server;

impl api::Api for Server {
    type Principal = ();

    // ANCHOR: scopes
    /// Who owns the token and the permissions that it grants. The server checks them against the operation.
    async fn verify_pat(
        &self,
        token: &str,
        _: &api::Request,
    ) -> Result<Option<api::Granted<(), PatScope>>, api::ApiError> {
        if token == "bad" {
            return Ok(None);
        }
        let scopes = token
            .split('+')
            .filter_map(|p| match p {
                "read" => Some(PatScope::IssuesRead),
                "write" => Some(PatScope::IssuesWrite),
                _ => None,
            })
            .collect();
        Ok(Some(api::Granted { principal: (), scopes }))
    }
    // ANCHOR_END: scopes

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

fn answer(api: impl api::Api, method: &'static str, path: &str, token: Option<&str>) -> api::Response {
    let mut req = api::runtime::new_request(method, path.to_owned(), vec![]).unwrap();
    if let Some(t) = token {
        req.headers_mut().append("authorization", honig::http::HeaderValue::try_from(format!("Bearer {t}")).unwrap());
    }
    api::block_on(api::Server::new(api).into_service().handle(req))
}

fn send(method: &'static str, path: &str, token: Option<&str>) -> u16 {
    answer(Server, method, path, token).status().as_u16()
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
fn a_missing_permission_is_403_with_insufficient_scope() {
    let resp = answer(Server, "POST", "/issues", Some("read"));
    assert_eq!(resp.status().as_u16(), 403);
    assert_eq!(
        resp.headers().get("www-authenticate").and_then(|v| v.to_str().ok()),
        Some("Bearer error=\"insufficient_scope\"")
    );
}

#[test]
fn a_token_that_the_api_refuses_is_401_not_403() {
    assert_eq!(send("POST", "/issues", Some("bad")), 401);
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
    for s in [PatScope::IssuesRead, PatScope::IssuesWrite] {
        assert_eq!(PatScope::from_name(s.as_str()), Some(s));
    }
    assert_eq!(PatScope::from_name("issues:admin"), None);
}

/// Names who is calling: the text of the token, or nobody for `Bearer anon`.
struct Named;

impl api::Api for Named {
    type Principal = String;

    async fn verify_pat(
        &self,
        token: &str,
        _: &api::Request,
    ) -> Result<Option<api::Granted<String, PatScope>>, api::ApiError> {
        let scopes = vec![PatScope::IssuesRead, PatScope::IssuesWrite];
        Ok((token != "anon").then(|| api::Granted { principal: token.to_owned(), scopes }))
    }

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

#[test]
fn each_operation_takes_the_caller_its_security_asks_for() {
    assert_eq!(answer(Named, "GET", "/issues", Some("ann")).status(), 204, "a principal reaches a secured operation");
    assert_eq!(answer(Named, "GET", "/issues", Some("anon")).status(), 401, "nobody does not");
    assert_eq!(answer(Named, "GET", "/public", None).status(), 204, "a public operation needs nobody");
}

/// A server whose handler refuses every token it sees: it was revoked after `verify_pat` checked it.
struct Revoked;

impl api::Api for Revoked {
    type Principal = ();

    async fn verify_pat(&self, _: &str, _: &api::Request) -> Result<Option<api::Granted<(), PatScope>>, api::ApiError> {
        Ok(Some(api::Granted { principal: (), scopes: vec![PatScope::IssuesRead, PatScope::IssuesWrite] }))
    }

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

/// RFC 9110 section 15.5.2: a 401 carries a challenge for the operation's schemes. That applies to the one the server
/// sends without a token and for the one a handler returns for a token it refuses.
#[test]
fn every_401_carries_a_challenge() {
    let challenge = |token: Option<&str>| {
        let resp = answer(Revoked, "GET", "/issues", token);
        (
            resp.status().as_u16(),
            resp.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).map(str::to_owned),
        )
    };
    assert_eq!(challenge(None), (401, Some("Bearer".to_owned())), "no token");
    assert_eq!(challenge(Some("read")), (401, Some("Bearer".to_owned())), "a token the handler refuses");
}

// ANCHOR: problem-instance
/// The default RFC 9457 body, with the request ID of the client as `instance`.
fn with_request_id(rejection: api::Rejection) -> api::Response {
    let mut problem = rejection.problem();
    problem.instance =
        rejection.headers.get("x-request-id").and_then(|id| id.to_str().ok()).map(|id| format!("urn:request:{id}"));
    problem.into_response()
}
// ANCHOR_END: problem-instance

#[test]
fn a_problem_carries_the_request_id_as_instance() {
    let service = api::Server::new(Server).error_response(with_request_id).into_service();
    let mut req = api::runtime::new_request("GET", "/issues".to_owned(), vec![]).unwrap();
    req.headers_mut().append("x-request-id", honig::http::HeaderValue::from_static("7f3a"));
    let resp = api::block_on(service.handle(req));
    assert_eq!(resp.status().as_u16(), 401);
    let body: serde_json::Value = serde_json::from_slice(resp.body().bytes()).unwrap();
    assert_eq!(body["instance"], "urn:request:7f3a");
    assert_eq!(body["status"], 401);
}
