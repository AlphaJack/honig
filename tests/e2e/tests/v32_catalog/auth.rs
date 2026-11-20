//! Who is calling. `authenticate` finds the principal from credentials or from what outer layers attached.

use std::net::SocketAddr;

use e2e::v32_catalog::api::{self, operations as ops};
use tower::ServiceExt;

/// What an authentication layer in front of the router attached.
#[derive(Clone)]
struct Principal(&'static str);

pub(super) struct Server;

// ANCHOR: request-context
/// Who is calling: the layer in front of the router and the client's address.
fn find_caller(req: &api::Request) -> Result<Option<String>, api::ApiError> {
    let principal = req
        .extensions()
        .get::<Principal>()
        .ok_or_else(|| api::ApiError::new(api::http::StatusCode::UNAUTHORIZED, "no principal"))?;
    let peer = api::peer(req.extensions())
        .ok_or_else(|| api::ApiError::new(api::http::StatusCode::UNAUTHORIZED, "no peer"))?;
    Ok(Some(format!("{} from {peer}", principal.0)))
}
// ANCHOR_END: request-context

impl api::Api for Server {
    fn error_response(rejection: api::Rejection) -> api::Response {
        super::error_body(rejection)
    }
    type Principal = String;

    async fn list_products(
        &self,
        _: ops::list_products::Params,
        ctx: &api::Context<'_, String>,
    ) -> Result<ops::list_products::Response, api::ApiError> {
        Ok(ops::list_products::Response::Ok { body: vec![], x_next: Some(ctx.principal.clone()) })
    }
    async fn create_product(
        &self,
        _: ops::create_product::Params,
        _: &api::Context<'_, String>,
    ) -> Result<ops::create_product::Response, api::ApiError> {
        Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not here"))
    }
    async fn get_product(
        &self,
        _: ops::get_product::Params,
        _: &api::Context<'_, String>,
    ) -> Result<ops::get_product::Response, api::ApiError> {
        Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not here"))
    }
}

fn serve() -> axum::Router {
    api::Server::new(Server).authenticate(|req: &api::Request| std::future::ready(find_caller(req))).into_axum()
}

fn list(router: axum::Router) -> axum::http::Response<axum::body::Body> {
    let req =
        axum::http::Request::get("/v1/products").header("X-API-Key", "k").body(axum::body::Body::empty()).unwrap();
    api::block_on(router.oneshot(req)).unwrap()
}

#[test]
fn layer_values_and_connect_info_reach_the_operation_layers() {
    let peer: SocketAddr = "192.0.2.7:4100".parse().unwrap();
    let router = serve()
        .layer(axum::Extension(Principal("ann")))
        // What `into_make_service_with_connect_info` attaches.
        .layer(axum::Extension(axum::extract::ConnectInfo(peer)));
    let resp = list(router);
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-next"], "ann from 192.0.2.7:4100");
}

#[test]
fn without_the_layer_authentication_refuses() {
    assert_eq!(list(serve()).status(), 401);
}

// ANCHOR: credentials
/// The principal that an API key names. Nobody without a key. 401 for a key that nobody holds.
fn caller_of_key(req: &api::Request) -> Result<Option<String>, api::ApiError> {
    let key = api::server::credentials(req).into_iter().map(|api::server::Credential::ApiKey(key)| key).next();
    match key {
        Some("secret") => Ok(Some("ann".to_owned())),
        Some(_) => Err(api::ApiError::new(api::http::StatusCode::UNAUTHORIZED, "unknown key")),
        None => Ok(None),
    }
}

fn authenticated() -> axum::Router {
    api::Server::new(Server).authenticate(|req: &api::Request| std::future::ready(caller_of_key(req))).into_axum()
}
// ANCHOR_END: credentials

fn list_with_key(key: &str) -> axum::http::Response<axum::body::Body> {
    let req =
        axum::http::Request::get("/v1/products").header("X-API-Key", key).body(axum::body::Body::empty()).unwrap();
    api::block_on(authenticated().oneshot(req)).unwrap()
}

#[test]
fn a_known_key_names_the_caller() {
    let resp = list_with_key("secret");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["x-next"], "ann");
}

#[test]
fn an_unknown_key_is_refused() {
    assert_eq!(list_with_key("guess").status(), 401);
}
