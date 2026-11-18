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
    // `oneOf` takes exactly one branch: `Vehicle` is checked, and reads as its variants.
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
    let mut customer = types::Customer::new("Ann".try_into().expect("a valid name"));
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
    // An enum default too (Matrix's `kind=user` on `/register`).
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
    assert_eq!(v["violations"][0]["path"], "/body/status", "{v}");
}
