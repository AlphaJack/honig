//! The most specific matching path template decides. It gives its operation for the
//! method or else 405 that lists only its own methods.

use std::future::Future;

use e2e::v32_routing::api::{self, operations as ops};

fn ok<T: Send>(r: T) -> impl Future<Output = Result<T, api::ApiError>> + Send {
    std::future::ready(Ok(r))
}

/// `heavy`: `search_orders` keeps a large buffer across an `await` as the state of a real handler can.
struct Server {
    heavy: bool,
}

impl api::Api for Server {
    type Principal = ();

    fn search_orders(
        &self,
        _: ops::search_orders::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::search_orders::Response, api::ApiError>> + Send {
        let heavy = self.heavy;
        async move {
            if heavy {
                let buffer = [7u8; 64 * 1024];
                std::future::ready(()).await;
                assert_eq!(buffer[buffer.len() - 1], 7);
            }
            Ok(ops::search_orders::Response::NoContent)
        }
    }
    fn update_order(
        &self,
        _: ops::update_order::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::update_order::Response, api::ApiError>> + Send {
        ok(ops::update_order::Response::NoContent)
    }
    fn status_at(
        &self,
        _: ops::status_at::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::status_at::Response, api::ApiError>> + Send {
        ok(ops::status_at::Response::NoContent)
    }
    fn status_by_key(
        &self,
        p: ops::status_by_key::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::status_by_key::Response, api::ApiError>> + Send {
        ok(ops::status_by_key::Response::Ok(p.key))
    }
    fn latest_meta(
        &self,
        _: ops::latest_meta::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::latest_meta::Response, api::ApiError>> + Send {
        ok(ops::latest_meta::Response::NoContent)
    }
    fn file_raw(
        &self,
        p: ops::file_raw::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::file_raw::Response, api::ApiError>> + Send {
        ok(ops::file_raw::Response::Ok(p.dir))
    }
    fn file_ext(
        &self,
        p: ops::file_ext::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::file_ext::Response, api::ApiError>> + Send {
        ok(ops::file_ext::Response::Ok(format!("{}|{}", p.name, p.ext)))
    }
    fn mix_literal_tail(
        &self,
        _: ops::mix_literal_tail::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::mix_literal_tail::Response, api::ApiError>> + Send {
        ok(ops::mix_literal_tail::Response::NoContent)
    }
    fn mix_mixed_tail(
        &self,
        p: ops::mix_mixed_tail::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::mix_mixed_tail::Response, api::ApiError>> + Send {
        ok(ops::mix_mixed_tail::Response::Ok(format!("{}|{}|{}", p.a, p.b, p.c)))
    }
    fn mix_var_tail(
        &self,
        _: ops::mix_var_tail::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::mix_var_tail::Response, api::ApiError>> + Send {
        ok(ops::mix_var_tail::Response::Accepted)
    }
}

fn send(method: &'static str, path: &str) -> api::Response {
    api::block_on(
        api::Server::new(Server { heavy: false })
            .into_service()
            .handle(api::runtime::new_request(method, path.to_owned(), vec![]).unwrap()),
    )
}

/// Each operation's future is boxed: the future `handle` returns stays small however large a handler's is (64 KiB
/// here).
#[test]
fn a_large_handler_future_stays_off_the_stack() {
    let server = api::Server::new(Server { heavy: true }).into_service();
    let answer = server.handle(api::runtime::new_request("GET", "/orders/search".to_owned(), vec![]).unwrap());
    let size = std::mem::size_of_val(&answer);
    assert!(size < 16 * 1024, "the request's future is {size} bytes");
    assert_eq!(api::block_on(answer).status().as_u16(), 204);
}

#[test]
fn a_literal_template_shadows_a_variable_one_for_other_methods() {
    let r = send("POST", "/orders/search");
    assert_eq!(r.status().as_u16(), 405, "not `update_order` with id `search`");
    assert_eq!(r.headers().get("allow").and_then(|v| v.to_str().ok()), Some("GET, HEAD, OPTIONS"));
}

#[test]
fn a_405_lists_only_the_matching_template_methods() {
    let r = send("POST", "/status/@");
    assert_eq!(r.status().as_u16(), 405);
    assert_eq!(r.headers().get("allow").and_then(|v| v.to_str().ok()), Some("GET, HEAD, OPTIONS"), "not `GET, GET`");
}

/// RFC 9110 section 9.3.7: an `OPTIONS` of a path that declares none gets `204` with the methods of the path. One
/// of no path is still `404`.
#[test]
fn an_options_lists_the_methods_of_the_path() {
    let r = send("OPTIONS", "/orders/search");
    assert_eq!((r.status().as_u16(), r.body().len()), (204, 0));
    assert_eq!(r.headers().get("allow").and_then(|v| v.to_str().ok()), Some("GET, HEAD, OPTIONS"));
    assert_eq!(send("OPTIONS", "/nowhere/at/all").status().as_u16(), 404);
}

/// RFC 9110 section 9.1: a server supports `HEAD` wherever it supports `GET`. A `HEAD` reaches the `GET` operation.
/// Its answer has the headers without the body, and a body in memory keeps its length as `Content-Length`.
#[test]
fn a_head_reaches_the_get_operation_and_gets_no_body() {
    assert_eq!(send("HEAD", "/orders/search").status().as_u16(), 204);
    let get = send("GET", "/nowhere");
    let head = send("HEAD", "/nowhere");
    assert_eq!((get.status().as_u16(), head.status().as_u16()), (404, 404));
    assert_eq!(head.body().len(), 0);
    assert_eq!(
        head.headers().get("content-length").and_then(|v| v.to_str().ok()),
        Some(get.body().len().to_string().as_str())
    );
    let refused = send("HEAD", "/orders/x");
    assert_eq!(refused.status().as_u16(), 405, "a path without `GET` has no `HEAD`");
    assert_eq!(
        (refused.headers().get("allow").and_then(|v| v.to_str().ok()), refused.body().len()),
        (Some("POST, OPTIONS"), 0)
    );
}

#[test]
fn encoded_unreserved_characters_match_literals() {
    assert_eq!(send("GET", "/orders/%73earch").status().as_u16(), 204, "`%73` is `s`");
    assert_eq!(send("POST", "/orders/%73earch").status().as_u16(), 405);
    // `@` is reserved. `%40` is data. So it reaches the variable decoded.
    let r = send("GET", "/status/%40");
    assert_eq!((r.status().as_u16(), r.body().bytes()), (200, &b"\"@\""[..]));
}

#[test]
fn each_template_still_reaches_its_operation() {
    assert_eq!(send("GET", "/orders/search").status().as_u16(), 204);
    assert_eq!(send("POST", "/orders/o1").status().as_u16(), 204);
    assert_eq!(send("GET", "/status/@").status().as_u16(), 204);
    let r = send("GET", "/status/k1");
    assert_eq!((r.status().as_u16(), r.body().bytes()), (200, &b"\"k1\""[..]));
    assert_eq!(send("GET", "/nowhere").status().as_u16(), 404);
}

/// 3.2 section 4.4 / section 4.8: a request URL is the server URL followed by the path,
/// so the server's base path comes first on the wire.
#[test]
fn requests_under_a_server_base_path_are_routed() {
    assert_eq!(send("GET", "/api/v1/orders/search").status().as_u16(), 204);
    assert_eq!(send("GET", "/api/v2/orders/search").status().as_u16(), 204, "every enum value of the variable");
    assert_eq!(send("GET", "/api/v3/orders/search").status().as_u16(), 404, "not a value of the variable");
    // `/orders` is also a base path. Yet `/orders/search` is a spec path itself.
    assert_eq!(send("GET", "/orders/search").status().as_u16(), 204);
    assert_eq!(send("GET", "/orders/orders/search").status().as_u16(), 204, "base `/orders` + `/orders/search`");
    assert_eq!(send("GET", "/apix/v1/orders/search").status().as_u16(), 404, "a prefix must end at a segment");
}

#[test]
fn a_literal_dead_end_falls_back_to_the_variable_beside_it() {
    assert_eq!(send("GET", "/files/latest/meta").status().as_u16(), 204);
    let r = send("GET", "/files/latest/raw");
    assert_eq!(
        (r.status().as_u16(), r.body().bytes()),
        (200, &b"\"latest\""[..]),
        "`latest` has no `raw`: `{{dir}}` matches it"
    );
}

#[test]
fn a_mixed_segment_captures_each_variable() {
    let r = send("GET", "/files/report.json");
    assert_eq!((r.status().as_u16(), r.body().bytes()), (200, &b"\"report|json\""[..]));
    // The first variable is the longest that still lets the rest match.
    let r = send("GET", "/files/a.b.json");
    assert_eq!((r.status().as_u16(), r.body().bytes()), (200, &b"\"a.b|json\""[..]));
    assert_eq!(send("GET", "/files/report").status().as_u16(), 404, "no `.`: neither template matches");
}

#[test]
fn a_variable_never_matches_an_empty_segment() {
    assert_eq!(send("GET", "/status/").status().as_u16(), 404);
    assert_eq!(send("GET", "/files//raw").status().as_u16(), 404);
    assert_eq!(send("GET", "/status/k1/").status().as_u16(), 404, "a trailing `/` is another path");
}

#[test]
fn among_mixed_siblings_the_most_specific_template_wins() {
    // `/m/{a}.{b}/y-{c}` (mixed, mixed) beats `/m/{a}.json/{c}` (mixed, variable)
    // although `{a}.json` comes first among the siblings.
    let r = send("GET", "/m/f.json/y-1");
    assert_eq!((r.status().as_u16(), r.body().bytes()), (200, &b"\"f|json|1\""[..]));
    assert_eq!(send("GET", "/m/f.json/x").status().as_u16(), 204);
    assert_eq!(send("GET", "/m/f.json/z").status().as_u16(), 202);
}

/// The axum router registers a route per template. So it merges next to an app with routes and a fallback of its
/// own. `handle` still picks the most specific template behind each route.
#[cfg(feature = "axum")]
mod axum_routes {

    use tower::ServiceExt;

    use super::{Server, api};

    fn app() -> axum::Router {
        axum::Router::new()
            .route("/health", axum::routing::get(|| async { "up" }))
            .fallback(|| async { axum::http::StatusCode::IM_A_TEAPOT })
            .merge(api::Server::new(Server { heavy: false }).into_axum())
    }

    fn status(method: &str, path: &str) -> u16 {
        let req = axum::http::Request::builder().method(method).uri(path).body(axum::body::Body::empty()).unwrap();
        api::block_on(app().oneshot(req)).unwrap().status().as_u16()
    }

    #[test]
    fn the_api_merges_beside_routes_and_a_fallback() {
        assert_eq!(status("GET", "/health"), 200);
        assert_eq!(status("GET", "/orders/search"), 204);
        assert_eq!(status("GET", "/nowhere"), 418, "undeclared paths reach the app's fallback");
        assert_eq!(status("DELETE", "/orders/search"), 405, "a declared path answers its own 405");
    }

    #[test]
    fn base_paths_mixed_segments_and_dead_ends_route() {
        assert_eq!(status("GET", "/api/v2/orders/search"), 204);
        assert_eq!(status("GET", "/files/notes.txt"), 200);
        assert_eq!(status("GET", "/files/latest/raw"), 200, "the literal `latest` dead-ends: `{{dir}}` matches it");
        assert_eq!(status("GET", "/files/latest/meta"), 204);
    }
}

/// Generic tower layers fit between routing and the operations: their `BoxError` converts into an `ApiError`.
#[test]
fn generic_tower_layers_fit_between_routing_and_the_operations() {
    let service = api::Server::new(Server { heavy: false })
        .operation_layer(tower::load_shed::LoadShedLayer::new())
        .operation_layer(tower::limit::ConcurrencyLimitLayer::new(1))
        .into_service();
    let r =
        api::block_on(service.handle(api::runtime::new_request("GET", "/orders/search".to_owned(), vec![]).unwrap()));
    assert_eq!(r.status().as_u16(), 204);
}
