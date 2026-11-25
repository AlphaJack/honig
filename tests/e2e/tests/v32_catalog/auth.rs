//! Who is calling. `verify_api_key` finds them from the key or from what outer layers attached.

use std::net::SocketAddr;

use e2e::v32_catalog::api::{self, operations as ops};
use tower::ServiceExt;

/// What an authentication layer in front of the router attached.
#[derive(Clone)]
struct Principal(&'static str);

/// Finds the caller with its function: from the key, or from what a layer in front of the router attached.
pub(super) struct Server(pub(super) fn(&str, &api::Request) -> Option<String>);

// ANCHOR: request-context
/// Who is calling: the layer in front of the router and the client's address.
fn find_caller(_: &str, req: &api::Request) -> Option<String> {
    let principal = req.extensions().get::<Principal>()?;
    let peer = honig::peer(req.extensions())?;
    Some(format!("{} from {peer}", principal.0))
}
// ANCHOR_END: request-context

impl api::Api for Server {
    fn error_response(rejection: api::Rejection) -> api::Response {
        super::error_body(rejection)
    }
    type ApiKey = String;

    /// Who is calling, as the function of the server finds it. 401 for nobody.
    async fn verify_api_key(&self, key: &str, req: &api::Request) -> Result<Option<String>, api::ApiError> {
        Ok((self.0)(key, req))
    }

    async fn list_products(
        &self,
        _: ops::list_products::Params,
        ctx: &api::Context<'_, ops::list_products::Security<Self>>,
    ) -> Result<ops::list_products::Response, api::ApiError> {
        let ops::list_products::Security::ApiKey(who) = &ctx.security;
        Ok(ops::list_products::Response::Ok { body: vec![], x_next: Some(who.clone()) })
    }
    async fn create_product(
        &self,
        _: ops::create_product::Params,
        _: &api::Context<'_, ops::create_product::Security<Self>>,
    ) -> Result<ops::create_product::Response, api::ApiError> {
        Err(api::ApiError::new(honig::http::StatusCode::NOT_IMPLEMENTED, "not here"))
    }
    async fn get_product(
        &self,
        _: ops::get_product::Params,
        _: &api::Context<'_, ops::get_product::Security<Self>>,
    ) -> Result<ops::get_product::Response, api::ApiError> {
        Err(api::ApiError::new(honig::http::StatusCode::NOT_IMPLEMENTED, "not here"))
    }
}

fn serve() -> axum::Router {
    api::Server::new(Server(find_caller)).into_axum()
}

fn list(router: axum::Router) -> axum::http::Response<axum::body::Body> {
    let req =
        axum::http::Request::get("/v1/products").header("X-API-Key", "k").body(axum::body::Body::empty()).unwrap();
    honig::block_on(router.oneshot(req)).unwrap()
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
/// Who an API key belongs to. Nobody for a key that nobody has.
fn holder(key: &str, _: &api::Request) -> Option<String> {
    (key == "secret").then(|| "ann".to_owned())
}

fn authenticated() -> axum::Router {
    api::Server::new(Server(holder)).into_axum()
}
// ANCHOR_END: credentials

fn list_with_key(key: &str) -> axum::http::Response<axum::body::Body> {
    let req =
        axum::http::Request::get("/v1/products").header("X-API-Key", key).body(axum::body::Body::empty()).unwrap();
    honig::block_on(authenticated().oneshot(req)).unwrap()
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

/// The `Debug` of a client, as a log line shows it, holds its transport and none of its credentials.
#[test]
fn a_client_debug_hides_its_credentials() {
    let client = api::Client::new("no transport").with_api_key("key-secret");
    let shown = format!("{client:?}");
    assert!(!shown.contains("key-secret") && shown.contains("no transport"), "{shown}");
}
