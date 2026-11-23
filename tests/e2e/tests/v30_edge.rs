//! Hand-written checks for the `edge` fixture.

mod support;

use assert_matches::assert_matches;
use std::future::Future;
use std::sync::Mutex;

use e2e::v30_edge::api::{self, operations as ops, types};
use serde_json::json;

#[test]
fn link_request_body_is_typed_after_the_target() {
    let body = serde_json::json!({
        "status": "confirmed",
        "total": {"amountCents": 9007199254740993_i64, "currency": "EUR"},
        "vehicle": {"kind": "van", "cargoLitres": 900}
    });
    let request = ops::get_order::Params { order_id: "00000000-0000-0000-0000-000000000000".parse().unwrap() };
    let again = ops::get_order::links::Reorder::new(&request, &body);
    // The target's request body type: `readOnly` `id` and `createdAt` are not part of it.
    let order: types::OrderRequest = again.body.expect("the response body becomes the new request body");
    assert_eq!(order.total.amount_cents, 9_007_199_254_740_993, "int64 beyond 2^53 survives");
    // `oneOf` accepts exactly one branch: `Vehicle` is checked, and derefs to its variants.
    assert_matches!(*order.vehicle, types::VehicleVariants::Van(_));
}

#[test]
fn open_objects_keep_unknown_keys() {
    let json = json!({"name": "Ann", "email": null, "vip": true, "tier": {"level": 3}});
    let customer: types::Customer = serde_json::from_value(json).expect("decodes");
    assert_eq!(customer.additional_properties.get("vip"), Some(&serde_json::json!(true)));
    let back = serde_json::to_value(&customer).expect("encodes");
    assert_eq!(back["tier"]["level"], 3, "a proxy roundtrip keeps what it does not know");
}

/// An extra entry built by hand with a declared name goes out once, at the declared field's value. The object then
/// decodes again: its decoder refuses a name written twice.
#[test]
fn an_extra_entry_named_like_a_field_is_written_once() {
    let mut customer = types::Customer::builder().name("Ann").build().expect("a valid name");
    customer.additional_properties.insert("name".to_owned(), json!("Bob"));
    customer.additional_properties.insert("vip".to_owned(), json!(true));
    let text = serde_json::to_string(&customer).expect("encodes");
    assert_eq!(text, r#"{"name":"Ann","vip":true}"#);
    let back: types::Customer = serde_json::from_str(&text).expect("decodes");
    assert_eq!(back.name, "Ann");
}

#[test]
fn spec_defaults_fill_absent_values() {
    let p: types::Problem = serde_json::from_value(json!({"title":"Nope","status":404})).expect("decodes");
    assert_eq!(p.type_, "about:blank");
    assert_eq!(ops::list_orders::Params::default().limit, 20);
}

#[derive(Default)]
struct Seen(Mutex<Option<i32>>);

impl api::Api for Seen {
    type Principal = ();

    async fn verify_bearer(&self, _: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }
    async fn verify_api_key(&self, _: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }
    async fn verify_tenant(&self, _: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }

    fn list_orders(
        &self,
        p: ops::list_orders::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::list_orders::Response, api::ApiError>> + Send {
        if let Ok(mut s) = self.0.lock() {
            *s = Some(p.limit.get());
        }
        std::future::ready(Err(api::ApiError::new(api::http::StatusCode::IM_A_TEAPOT, "seen")))
    }
    fn create_order(
        &self,
        _: ops::create_order::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::create_order::Response, api::ApiError>> + Send {
        std::future::ready(Err(api::ApiError::new(api::http::StatusCode::IM_A_TEAPOT, "seen")))
    }
    fn get_order(
        &self,
        _: ops::get_order::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::get_order::Response, api::ApiError>> + Send {
        std::future::ready(Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not used by this test")))
    }
    fn upload_document(
        &self,
        _: ops::upload_document::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::upload_document::Response, api::ApiError>> + Send {
        std::future::ready(Err(api::ApiError::new(api::http::StatusCode::NOT_IMPLEMENTED, "not used by this test")))
    }
    fn cookie_form_fa(
        &self,
        p: ops::cookie_form_fa::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::cookie_form_fa::Response, api::ApiError>> + Send {
        COOKIE.with(|c| *c.borrow_mut() = Some(format!("{:?}", p.color)));
        std::future::ready(Ok(ops::cookie_form_fa::Response::NoContent))
    }
    fn cookie_form_fo(
        &self,
        p: ops::cookie_form_fo::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::cookie_form_fo::Response, api::ApiError>> + Send {
        COOKIE.with(|c| *c.borrow_mut() = Some(format!("{:?}", p.color)));
        std::future::ready(Ok(ops::cookie_form_fo::Response::NoContent))
    }
}

thread_local! {
    /// The `color` cookie that the server decoded last, as `Debug` text.
    static COOKIE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// A client whose `Cookie` header goes to the returned slot. Its server records what it decoded in `COOKIE`.
fn cookie_client() -> (api::Client<impl api::Transport>, std::sync::Arc<Mutex<Option<String>>>) {
    let sent = std::sync::Arc::new(Mutex::new(None::<String>));
    let log = std::sync::Arc::clone(&sent);
    let client = api::Client::new(support::loopback(move |req: api::Request| {
        *log.lock().unwrap() = req.headers().get("cookie").and_then(|v| v.to_str().ok()).map(String::from);
        api::Server::new(Seen::default()).into_service().handle(req)
    }));
    (client, sent)
}

/// 3.0 allows `explode: false` on a cookie: items are joined with commas as in the query.
#[test]
fn a_cookie_with_explode_false_joins_its_items_with_commas() {
    let (client, sent) = cookie_client();
    let colors = ["blue", "black", "brown"].map(String::from).to_vec();
    let want = format!("{colors:?}");
    api::block_on(client.cookie_form_fa(ops::cookie_form_fa::Params { color: colors })).expect("204");
    assert_eq!(sent.lock().unwrap().take().as_deref(), Some("color=blue,black,brown"));
    assert_eq!(COOKIE.with(|c| c.borrow_mut().take()), Some(want), "the server decodes what the client wrote");

    let rgb: types::CookieFormFoColor = serde_json::from_value(json!({"R": 100, "G": 200, "B": 150})).expect("R, G, B");
    let want = format!("{rgb:?}");
    api::block_on(client.cookie_form_fo(ops::cookie_form_fo::Params { color: rgb })).expect("204");
    assert_eq!(sent.lock().unwrap().take().as_deref(), Some("color=R,100,G,200,B,150"));
    assert_eq!(COOKIE.with(|c| c.borrow_mut().take()), Some(want), "the server decodes what the client wrote");
}

#[test]
fn the_server_applies_parameter_defaults() {
    let seen = std::sync::Arc::new(Seen::default());
    let mut req = api::runtime::new_request("GET", "/orders".to_owned(), vec![]).unwrap();
    req.headers_mut().append("authorization", honig::http::HeaderValue::try_from("Bearer t".to_owned()).unwrap());
    api::block_on(api::Server::new(std::sync::Arc::clone(&seen)).into_service().handle(req));
    assert_eq!(*seen.0.lock().unwrap(), Some(20));
}

#[test]
fn the_client_leaves_out_a_parameter_at_its_default() {
    let sent = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let log = std::sync::Arc::clone(&sent);
    let client = api::Client::new(support::loopback(move |req: api::Request| {
        log.lock().unwrap().push(req.uri().to_string());
        api::Server::new(Seen::default()).into_service().handle(req)
    }))
    .with_bearer("t");
    for limit in [20, 5] {
        let params = ops::list_orders::Params {
            limit: limit.try_into().expect("a valid limit"),
            ..ops::list_orders::Params::default()
        };
        drop(api::block_on(client.list_orders(params)));
    }
    // An enum default too.
    let params =
        ops::list_orders::Params { sort: types::ListOrdersSort::Oldest, ..ops::list_orders::Params::default() };
    drop(api::block_on(client.list_orders(params)));
    // `limit: 20` and `sort: newest` are the defaults: the same as leaving them out, which some servers require.
    assert_eq!(*sent.lock().unwrap(), ["/orders", "/orders?limit=5", "/orders?sort=oldest"]);
}

fn create_order(body: &serde_json::Value) -> u16 {
    let mut req = api::runtime::new_request("POST", "/orders".to_owned(), vec![]).unwrap();
    req.headers_mut().append("authorization", honig::http::HeaderValue::try_from("Bearer t".to_owned()).unwrap());
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() = honig::Body::from(serde_json::to_vec(body).unwrap());
    api::runtime::block_on(api::Server::new(Seen::default()).into_service().handle(req)).status().as_u16()
}

#[test]
fn a_number_with_a_zero_fraction_is_an_integer() {
    let order = |cents: serde_json::Value| {
        create_order(
            &json!({"status":"draft","total":{"amountCents":cents,"currency":"EUR"},"vehicle":{"kind":"car","seats":2}}),
        )
    };
    // JSON Schema 2020-12: 1.0 is an integer. 418: the handler ran.
    assert_eq!(order(json!(1.0)), 418);
    assert_eq!(order(json!(1.5)), 400);
}

#[test]
fn a_branch_that_takes_any_value_lets_any_value_through() {
    let order = json!({"status":"draft","total":{"amountCents":1,"currency":"EUR"},"vehicle":{"kind":"car","seats":2}});
    for discount in [json!(null), json!({"amountCents": 5, "currency": "EUR"}), json!("SUMMER")] {
        let mut body = order.clone();
        body["discount"] = discount.clone();
        // 418: the handler ran. 400 would be a violation.
        assert_eq!(create_order(&body), 418, "discount {discount}");
    }
}

#[test]
fn rejections_use_the_described_error_schema() {
    let seen = std::sync::Arc::new(Seen::default());
    let mut req = api::runtime::new_request("POST", "/orders".to_owned(), vec![]).unwrap();
    req.headers_mut().append("authorization", honig::http::HeaderValue::try_from("Bearer t".to_owned()).unwrap());
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() = honig::Body::from(
        serde_json::to_vec(
            &json!({"status":"bogus","total":{"amountCents":1,"currency":"EUR"},"vehicle":{"kind":"car","seats":2}}),
        )
        .unwrap(),
    );
    let r = api::block_on(api::Server::new(std::sync::Arc::clone(&seen)).into_service().handle(req));
    assert_eq!(
        (r.status().as_u16(), r.headers().get("content-type").map_or("", |v| v.to_str().unwrap_or(""))),
        (400, "application/problem+json")
    );
    // The spec's own `Problem` type decodes it: a third-party client would too.
    let p: types::Problem = serde_json::from_slice(r.body().bytes()).expect("a Problem");
    assert_eq!((p.status, p.title.as_str()), (400, "Bad Request"));
    let v: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
    assert_eq!(v["errors"][0]["pointer"], "#/status", "{v}");
}
