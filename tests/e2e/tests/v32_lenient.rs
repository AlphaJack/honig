//! A lenient client (`client = { strict = false }`) reads a response that holds what the server added: an enum value
//! that the description does not list and a property of a closed object. Its requests stay strict.

use e2e::v32_lenient::api::{self, operations as ops, types};

/// A client whose server answers every request with `body`.
fn client_answering(body: &'static str) -> api::Client<impl api::Transport> {
    api::Client::new(tower::service_fn(move |_: api::Request| async move {
        let mut r = api::runtime::new_response(200);
        r.headers_mut().insert("content-type", honig::http::HeaderValue::from_static("application/json"));
        *r.body_mut() = honig::Body::from(body.as_bytes().to_vec());
        Ok::<_, std::convert::Infallible>(r)
    }))
}

#[test]
fn a_response_reads_an_added_enum_value_and_property() {
    let client = client_answering(r#"{"id":"o1","status":"archived","priority":9,"added":true}"#);
    let response = api::block_on(client.get_order(ops::get_order::Params::new("o1".to_owned()))).expect("it reads");
    let ops::get_order::Response::Ok(order) = response;
    assert_eq!(order.status, types::Status::Other("archived".to_owned()));
    assert_eq!(order.priority, Some(types::OrderPriority::Other(9)));
    assert_eq!(order.status.as_str(), "archived", "it goes back as it came");
}

#[test]
fn a_request_stays_strict() {
    // The request copy is closed: it has no `Other`, and an undeclared property does not decode.
    let sent: Result<types::OrderRequest, _> = serde_json::from_str(r#"{"id":"o1","status":"archived"}"#);
    assert!(sent.is_err(), "{sent:?}");
    let added: Result<types::OrderRequest, _> = serde_json::from_str(r#"{"id":"o1","status":"open","added":1}"#);
    assert!(added.is_err(), "{added:?}");
}

#[test]
fn a_response_reads_an_added_member_of_a_tagged_union() {
    let client = client_answering(r#"{"id":"o1","status":"open","delivery":{"kind":"drone","pad":"roof"}}"#);
    let response = api::block_on(client.get_order(ops::get_order::Params::new("o1".to_owned()))).expect("it reads");
    let ops::get_order::Response::Ok(order) = response;
    let delivery = order.delivery.expect("it has a delivery");
    let added = serde_json::json!({"kind": "drone", "pad": "roof"});
    assert_eq!(*delivery, types::DeliveryVariants::Other(added.clone()));
    assert_eq!(serde_json::to_value(&delivery).unwrap(), added, "it goes back as it came");
}

#[test]
fn a_response_without_a_tag_still_fails() {
    let read: Result<types::Order, _> =
        serde_json::from_str(r#"{"id":"o1","status":"open","delivery":{"pad":"roof"}}"#);
    assert!(read.is_err(), "{read:?}");
}

#[test]
fn a_request_refuses_an_added_member_of_a_tagged_union() {
    let sent: Result<types::OrderRequest, _> =
        serde_json::from_str(r#"{"id":"o1","status":"open","delivery":{"kind":"drone","pad":"roof"}}"#);
    assert!(sent.is_err(), "{sent:?}");
}
