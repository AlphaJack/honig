//! `if`/`then` on a property's constant becomes a native enum.

use assert_matches::assert_matches;
use std::future::Future;

use e2e::v32_conditional::api::{self, operations as ops, types};
use serde_json::json;

#[test]
fn if_then_decodes_into_variants() {
    let email: types::IfThen = serde_json::from_value(json!({"type":"email","email":"a@example.com"})).unwrap();
    assert_matches!(&email, types::IfThen::Email(e) if e.email.as_str() == "a@example.com");
    let sms: types::IfThen = serde_json::from_value(json!({"type":"sms","phone":"+49"})).unwrap();
    assert_matches!(sms, types::IfThen::Sms(_));
    // Values that no `if` tests fall back to a typed `Otherwise` and not an untyped `Other`.
    let fax: types::IfThen = serde_json::from_value(json!({"type":"fax"})).unwrap();
    assert_matches!(&fax, types::IfThen::Otherwise(o) if o.type_.as_deref() == Some("fax"));
    assert_eq!(serde_json::to_value(&email).unwrap(), serde_json::json!({"type": "email", "email": "a@example.com"}));
}

#[test]
fn all_of_with_a_one_of_member_is_one_of_its_branches() {
    let pickup: types::Delivery = serde_json::from_value(json!({"pickup": true, "orderId": "o-1"})).unwrap();
    assert_matches!(&*pickup, types::DeliveryVariants::Pickup(d) if d.pickup && d.order_id == "o-1");
    let shipped: types::Delivery = serde_json::from_value(json!({"street": "Main St 1", "orderId": "o-2"})).unwrap();
    assert_matches!(&*shipped, types::DeliveryVariants::Address(d) if d.street == "Main St 1" && d.order_id == "o-2");
    assert_eq!(serde_json::to_value(&shipped).unwrap(), json!({"street": "Main St 1", "orderId": "o-2"}));
}

struct Accept;

impl api::Api for Accept {
    type Principal = ();

    fn a(
        &self,
        _: ops::a::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::a::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::a::Response::NoContent))
    }
    fn b(
        &self,
        _: ops::b::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::b::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::b::Response::NoContent))
    }
    fn c(
        &self,
        _: ops::c::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::c::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::c::Response::NoContent))
    }
}

fn post_b(body: &serde_json::Value) -> api::Response {
    let mut req = api::runtime::new_request("POST", "/b".to_owned(), vec![]).unwrap();
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() = honig::Body::from(serde_json::to_vec(body).unwrap());
    api::block_on(api::Server::new(Accept).into_service().handle(req))
}

#[test]
fn a_missing_conditional_field_is_rejected() {
    assert_eq!(post_b(&json!({"type":"sms","phone":"+49"})).status().as_u16(), 204);
    let r = post_b(&json!({"type":"sms"}));
    assert_eq!(r.status().as_u16(), 400, "{}", String::from_utf8_lossy(r.body().bytes()));
}

#[test]
fn else_is_the_typed_fallback() {
    let us: types::Shipping = serde_json::from_value(json!({"country": "US", "zip": "94110"})).unwrap();
    assert_matches!(&us, types::Shipping::Us(s) if s.zip == "94110");
    let de: types::Shipping = serde_json::from_value(json!({"country": "DE", "postcode": "10115"})).unwrap();
    // `else: {required: [postcode]}` makes it a `String` and not an `Option`.
    assert_matches!(&de, types::Shipping::Otherwise(s) if s.postcode == "10115");
    assert!(serde_json::from_value::<types::Shipping>(json!({"country": "DE"})).is_err(), "postcode is required");
}

#[test]
fn one_variant_covers_several_tested_values() {
    for via in ["email", "mail"] {
        let c: types::Contact = serde_json::from_value(json!({"via": via, "address": "x"})).unwrap();
        assert_matches!(c, types::Contact::EmailOrMail(_), "{via}");
    }
    let sms: types::Contact = serde_json::from_value(json!({"via": "sms", "phone": "+49"})).unwrap();
    assert_matches!(sms, types::Contact::Sms(_));
}
