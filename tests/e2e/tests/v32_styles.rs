//! OpenAPI 3.2 section 4.12.6 "Style Examples" row by row. The generated client
//! writes exactly the serialization that the spec shows. The generated server
//! decodes that exact text back to the same value.
//!
//! `color` is `"blue"`, `["blue", "black", "brown"]` or `{R: 100, G: 200, B: 150}`.

mod support;

use std::cell::RefCell;
use std::future::Future;

use e2e::v32_styles::api::{self, Client, operations as ops};

/// A request as it went over the wire: path and query, `color` header and `Cookie` header.
type Wire = (String, Option<String>, Option<String>);

thread_local! {
    /// What the server decoded last as `Debug` text.
    static SEEN: RefCell<Option<String>> = const { RefCell::new(None) };
    /// What the client sent last.
    static WIRE: RefCell<Option<Wire>> = const { RefCell::new(None) };
}

struct Server;

macro_rules! server {
    ($($op:ident),* $(,)?) => {
        impl api::Api for Server {
            type Principal = ();

            $(
                fn $op(&self, p: ops::$op::Params, _: &api::Context<'_>) -> impl Future<Output = Result<ops::$op::Response, api::ApiError>> + Send {
                    SEEN.with(|s| *s.borrow_mut() = Some(format!("{:?}", p.color)));
                    std::future::ready(Ok(ops::$op::Response::NoContent))
                }
            )*
        }
    };
}

server!(
    matrix_fs, matrix_fa, matrix_fo, matrix_ta, matrix_to, label_fs, label_fa, label_fo, label_ta, label_to, simple_fa,
    simple_fo, simple_ta, simple_to, form_fa, form_fo, form_ta, form_to, space_fa, space_fo, pipe_fa, pipe_fo, deep_o,
    header_fa, header_fo, header_ta, header_to, cookie_ta, cookie_to,
);

fn client() -> Client<impl api::Transport> {
    Client::new(support::loopback(|req: api::Request| {
        let wire = (
            req.uri().to_string(),
            req.headers().get("color").and_then(|v| v.to_str().ok()).map(String::from),
            req.headers().get("cookie").and_then(|v| v.to_str().ok()).map(String::from),
        );
        WIRE.with(|w| *w.borrow_mut() = Some(wire));
        api::Server::new(Server).into_service().handle(req)
    }))
}

fn arr() -> Vec<String> {
    ["blue", "black", "brown"].map(String::from).to_vec()
}

fn obj<T: serde::de::DeserializeOwned>() -> T {
    serde_json::from_value(serde_json::json!({"R": 100, "G": 200, "B": 150})).expect("R, G, B")
}

/// Where the serialization goes.
#[derive(Clone, Copy)]
enum At {
    /// The whole path and query string.
    Target,
    /// The `color` header's value.
    Header,
    /// The `Cookie` header's value.
    Cookie,
}

/// Send `params` with the generated client: the wire text must be `expect`,
/// and the server must decode it to `params.color`. Then send `expect` as is:
/// the server must decode it to the same value.
macro_rules! row {
    ($name:ident, $op:ident, $value:expr, $at:expr, $path:literal, $expect:literal) => {
        #[test]
        fn $name() {
            let params = ops::$op::Params { color: $value };
            let want = format!("{:?}", params.color);
            let resp = api::block_on(client().$op(params));
            assert!(resp.is_ok(), "client call: {resp:?}");
            let (target, header, cookie) = WIRE.with(|w| w.borrow_mut().take()).expect("sent");
            let sent = match $at {
                At::Target => target.clone(),
                At::Header => header.unwrap_or_default(),
                At::Cookie => cookie.unwrap_or_default(),
            };
            assert_eq!(sent, $expect, "client serialization ({target})");
            assert_eq!(SEEN.with(|s| s.borrow_mut().take()).as_deref(), Some(want.as_str()), "server decoded the client's");

            // The text of the spec sent as it is.
            let mut req = match $at {
                At::Target => api::runtime::new_request("GET", $expect.to_owned(), vec![]).unwrap(),
                At::Header | At::Cookie => api::runtime::new_request("GET", $path.to_owned(), vec![]).unwrap(),
            };
            match $at {
                At::Header => {
                    req.headers_mut().append("color", honig::http::HeaderValue::try_from($expect.to_owned()).unwrap());
                }
                At::Cookie => {
                    req.headers_mut().append("cookie", honig::http::HeaderValue::try_from($expect.to_owned()).unwrap());
                }
                At::Target => {}
            }
            let status = api::block_on(api::Server::new(Server).into_service().handle(req)).status().as_u16();
            assert_eq!(status, 204, "server rejected the spec's serialization `{}`", $expect);
            assert_eq!(SEEN.with(|s| s.borrow_mut().take()).as_deref(), Some(want.as_str()), "server decoded the spec's");
        }
    };
}

row!(matrix_false_string, matrix_fs, "blue".to_owned(), At::Target, "", "/matrix-f-s/;color=blue");
row!(matrix_false_array, matrix_fa, arr(), At::Target, "", "/matrix-f-a/;color=blue,black,brown");
row!(matrix_false_object, matrix_fo, obj(), At::Target, "", "/matrix-f-o/;color=R,100,G,200,B,150");
row!(matrix_true_array, matrix_ta, arr(), At::Target, "", "/matrix-t-a/;color=blue;color=black;color=brown");
row!(matrix_true_object, matrix_to, obj(), At::Target, "", "/matrix-t-o/;R=100;G=200;B=150");
row!(label_false_string, label_fs, "blue".to_owned(), At::Target, "", "/label-f-s/.blue");
row!(label_false_array, label_fa, arr(), At::Target, "", "/label-f-a/.blue,black,brown");
row!(label_false_object, label_fo, obj(), At::Target, "", "/label-f-o/.R,100,G,200,B,150");
row!(label_true_array, label_ta, arr(), At::Target, "", "/label-t-a/.blue.black.brown");
row!(label_true_object, label_to, obj(), At::Target, "", "/label-t-o/.R=100.G=200.B=150");
row!(simple_false_array, simple_fa, arr(), At::Target, "", "/simple-f-a/blue,black,brown");
row!(simple_false_object, simple_fo, obj(), At::Target, "", "/simple-f-o/R,100,G,200,B,150");
row!(simple_true_array, simple_ta, arr(), At::Target, "", "/simple-t-a/blue,black,brown");
row!(simple_true_object, simple_to, obj(), At::Target, "", "/simple-t-o/R=100,G=200,B=150");

row!(form_false_array, form_fa, arr(), At::Target, "", "/form-f-a?color=blue,black,brown");
row!(form_false_object, form_fo, obj(), At::Target, "", "/form-f-o?color=R,100,G,200,B,150");
row!(form_true_array, form_ta, arr(), At::Target, "", "/form-t-a?color=blue&color=black&color=brown");
row!(form_true_object, form_to, obj(), At::Target, "", "/form-t-o?R=100&G=200&B=150");
row!(space_false_array, space_fa, arr(), At::Target, "", "/space-f-a?color=blue%20black%20brown");
row!(space_false_object, space_fo, obj(), At::Target, "", "/space-f-o?color=R%20100%20G%20200%20B%20150");
row!(pipe_false_array, pipe_fa, arr(), At::Target, "", "/pipe-f-a?color=blue%7Cblack%7Cbrown");
row!(pipe_false_object, pipe_fo, obj(), At::Target, "", "/pipe-f-o?color=R%7C100%7CG%7C200%7CB%7C150");
row!(deep_object, deep_o, obj(), At::Target, "", "/deep-o?color%5BR%5D=100&color%5BG%5D=200&color%5BB%5D=150");

row!(header_false_array, header_fa, arr(), At::Header, "/header-f-a", "blue,black,brown");
row!(header_false_object, header_fo, obj(), At::Header, "/header-f-o", "R,100,G,200,B,150");
row!(header_true_array, header_ta, arr(), At::Header, "/header-t-a", "blue,black,brown");
row!(header_true_object, header_to, obj(), At::Header, "/header-t-o", "R=100,G=200,B=150");

row!(cookie_true_array, cookie_ta, arr(), At::Cookie, "/cookie-t-a", "color=blue; color=black; color=brown");
row!(cookie_true_object, cookie_to, obj(), At::Cookie, "/cookie-t-o", "R=100; G=200; B=150");
