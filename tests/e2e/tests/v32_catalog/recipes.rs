//! Recipes: whole setups that combine the pieces of the other pages.

use std::time::Duration;

use e2e::v32_catalog::api;
use tower::ServiceExt;

use super::Catalog;

// ANCHOR: spa
/// A web app and its API on one port. The API lives under its base path `/v1`, so an unknown `/v1/...` path gets a
/// `404` in the error schema. Every other path serves the app: a file of `dist` or else its `index.html`, so that the
/// app's own router handles the path. `dist` is compiled into the binary, so the server ships as one file.
#[derive(rust_embed::Embed)]
#[folder = "tests/v32_catalog/dist"]
struct Dist;

async fn app(uri: axum::http::Uri) -> axum::response::Response {
    use axum::response::IntoResponse;
    let path = uri.path().trim_start_matches('/');
    match Dist::get(path).or_else(|| Dist::get("index.html")) {
        Some(file) => {
            ([(axum::http::header::CONTENT_TYPE, file.metadata.mimetype().to_owned())], file.data).into_response()
        }
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

fn web_app() -> axum::Router {
    let api = api::Server::new(Catalog::default()).authenticate(super::check_key).into_service();
    axum::Router::new().nest_service("/v1", api).fallback(app)
}
// ANCHOR_END: spa

// ANCHOR: claims
/// What an auth layer that you already run puts in the extensions, such as the claims of a JWT. Its fields are its
/// own, not your principal.
#[derive(Clone)]
struct Claims {
    sub: String,
}

/// A stand-in for that layer: it trusts `Authorization: Bearer <sub>`. A real one checks a signature.
fn claims_layer(mut req: axum::extract::Request) -> axum::extract::Request {
    let sub = req.headers().get("authorization").and_then(|v| v.to_str().ok()?.strip_prefix("Bearer "));
    if let Some(sub) = sub.map(str::to_owned) {
        req.extensions_mut().insert(Claims { sub });
    }
    req
}

/// `authenticate` maps the claims to the principal of the `Api`. A request without claims is anonymous.
fn with_claims() -> axum::Router {
    api::Server::new(super::auth::Server)
        .authenticate(|req: &api::Request| {
            let who = req.extensions().get::<Claims>().map(|claims| claims.sub.clone());
            std::future::ready(Ok(who))
        })
        .into_axum()
        .layer(tower::util::MapRequestLayer::new(claims_layer))
}
// ANCHOR_END: claims

// ANCHOR: outer-errors
/// Layers outside the server that fail with an error, such as a timeout or a full queue. axum turns such an error
/// into a response with `HandleErrorLayer`. `errors()` renders it in the error schema of the description, so a
/// client decodes it as its typed error.
fn guarded(max_in_flight: usize) -> axum::Router {
    let server = api::Server::new(Catalog::default()).authenticate(super::check_key);
    let errors = server.errors();
    server.into_axum().layer(
        tower::ServiceBuilder::new()
            .layer(axum::error_handling::HandleErrorLayer::new(move |e: tower::BoxError| {
                let status = if e.is::<tower::timeout::error::Elapsed>() {
                    api::http::StatusCode::GATEWAY_TIMEOUT
                } else {
                    api::http::StatusCode::SERVICE_UNAVAILABLE
                };
                std::future::ready(errors.response(status, e.to_string()))
            }))
            .load_shed()
            .concurrency_limit(max_in_flight)
            .timeout(Duration::from_secs(30)),
    )
}
// ANCHOR_END: outer-errors

// ANCHOR: stack
/// Every kind of layer in one server. A request passes them top to bottom.
fn full_stack() -> axum::Router {
    api::Server::new(super::auth::Server)
        // Inside the server, after routing and the check of the credentials that the description asks for. Their
        // errors go out in the error schema.
        .operation_layer(tower::timeout::TimeoutLayer::new(Duration::from_secs(10)))
        .authenticate(|req: &api::Request| {
            std::future::ready(Ok(req.extensions().get::<Claims>().map(|claims| claims.sub.clone())))
        })
        // After `authenticate`: this layer sees the principal.
        .operation_layer(tower::filter::AsyncFilterLayer::new(|req: api::Request| {
            let blocked = req.extensions().get::<String>().is_some_and(|who| who == "mallory");
            let refusal = api::ApiError::new(api::http::StatusCode::FORBIDDEN, "blocked");
            std::future::ready(if blocked { Err(refusal) } else { Ok(req) })
        }))
        .into_axum()
        // Outside the server, on every request before routing. The last one added runs first.
        .layer(tower::util::MapRequestLayer::new(claims_layer))
        .layer(tower_http::cors::CorsLayer::permissive())
}
// ANCHOR_END: stack

async fn get(router: axum::Router, path: &str, bearer: Option<&str>) -> axum::http::Response<axum::body::Body> {
    let mut req =
        axum::http::Request::get(path).header("X-API-Key", "secret").header("Origin", "https://shop.example.com");
    if let Some(sub) = bearer {
        req = req.header("Authorization", format!("Bearer {sub}"));
    }
    router.oneshot(req.body(axum::body::Body::empty()).unwrap()).await.unwrap()
}

async fn text(resp: axum::http::Response<axum::body::Body>) -> String {
    String::from_utf8(axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}

#[tokio::test]
async fn the_web_app_and_its_api_share_one_port() {
    let products = get(web_app(), "/v1/products", None).await;
    assert_eq!(products.status(), 200);
    assert_eq!(products.headers()["content-type"], "application/json");
    let unknown = get(web_app(), "/v1/nothing-here", None).await;
    assert_eq!(unknown.status(), 404, "an unknown API path is the API's 404, not the app");
    assert!(text(unknown).await.contains("\"code\":404"));
    let script = get(web_app(), "/assets/app.js", None).await;
    assert_eq!(text(script).await, "console.log(\"shop\");\n");
    let deep_link = get(web_app(), "/products/7", None).await;
    assert_eq!(deep_link.status(), 200);
    assert!(text(deep_link).await.contains("<title>Shop</title>"), "the app's router handles the path");
}

#[tokio::test]
async fn claims_become_the_principal() {
    let ann = get(with_claims(), "/v1/products", Some("ann")).await;
    assert_eq!(ann.status(), 200);
    assert_eq!(ann.headers()["x-next"], "ann");
    assert_eq!(get(with_claims(), "/v1/products", None).await.status(), 401, "no claims, nobody");
}

#[tokio::test]
async fn errors_of_outer_layers_go_out_in_the_error_schema() {
    assert_eq!(get(guarded(64), "/v1/products", None).await.status(), 200);
    let full = get(guarded(0), "/v1/products", None).await;
    assert_eq!(full.status(), 503);
    let body = text(full).await;
    assert!(body.contains("\"code\":503") && body.contains("overloaded"), "{body}");
}

#[tokio::test]
async fn the_full_stack_runs_in_order() {
    let ann = get(full_stack(), "/v1/products", Some("ann")).await;
    assert_eq!(ann.status(), 200);
    assert_eq!(ann.headers()["access-control-allow-origin"], "*");
    assert_eq!(ann.headers()["x-next"], "ann");
    let mallory = get(full_stack(), "/v1/products", Some("mallory")).await;
    assert_eq!(mallory.status(), 403, "the filter after `authenticate` sees the principal");
    assert!(text(mallory).await.contains("\"code\":403"));
}
