//! Spec rules the roundtrip tests cannot see: client and server would agree
//! with each other while both differ from the spec. Each test cites its rule.

mod support;

use std::future::Future;

use e2e::v32_conformance::api::{self, Client, operations as ops};

struct Server;

fn ok<T: Send>(r: T) -> impl Future<Output = Result<T, api::ApiError>> + Send {
    std::future::ready(Ok(r))
}

impl api::Api for Server {
    type Bearer = ();
    type CookieKey = ();
    fn error_response(rejection: api::Rejection) -> api::Response {
        rejection.response()
    }

    async fn verify_bearer(&self, _: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }
    async fn verify_cookie_key(&self, _: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok(Some(()))
    }

    fn open(
        &self,
        _: ops::open::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::open::Response, api::ApiError>> + Send {
        ok(ops::open::Response::NoContent)
    }
    fn inherits(
        &self,
        _: ops::inherits::Params,
        _: &api::Context<'_, ops::inherits::Security<Self>>,
    ) -> impl Future<Output = Result<ops::inherits::Response, api::ApiError>> + Send {
        ok(ops::inherits::Response::NoContent)
    }
    fn get_item(
        &self,
        _: ops::get_item::Params,
        _: &api::Context<'_, ops::get_item::Security<Self>>,
    ) -> impl Future<Output = Result<ops::get_item::Response, api::ApiError>> + Send {
        ok(ops::get_item::Response::Success { status: ops::get_item::SuccessStatus::Created, body: "x".into() })
    }
    fn post_closed(
        &self,
        _: ops::post_closed::Params,
        _: &api::Context<'_, ops::post_closed::Security<Self>>,
    ) -> impl Future<Output = Result<ops::post_closed::Response, api::ApiError>> + Send {
        ok(ops::post_closed::Response::NoContent)
    }
    fn optional_body(
        &self,
        _: ops::optional_body::Params,
        _: &api::Context<'_, ops::optional_body::Security<Self>>,
    ) -> impl Future<Output = Result<ops::optional_body::Response, api::ApiError>> + Send {
        ok(ops::optional_body::Response::NoContent)
    }
    fn post_product(
        &self,
        _: ops::post_product::Params,
        _: &api::Context<'_, ops::post_product::Security<Self>>,
    ) -> impl Future<Output = Result<ops::post_product::Response, api::ApiError>> + Send {
        ok(ops::post_product::Response::NoContent)
    }
    fn upload(
        &self,
        _: ops::upload::Params,
        _: &api::Context<'_, ops::upload::Security<Self>>,
    ) -> impl Future<Output = Result<ops::upload::Response, api::ApiError>> + Send {
        ok(ops::upload::Response::NoContent)
    }
    fn filter(
        &self,
        p: ops::filter::Params,
        _: &api::Context<'_, ops::filter::Security<Self>>,
    ) -> impl Future<Output = Result<ops::filter::Response, api::ApiError>> + Send {
        assert_eq!(p.page, Some(2));
        assert_eq!(p.where_.map(|w| w.a), Some(Some("x".to_owned())));
        ok(ops::filter::Response::NoContent)
    }
    fn cookies(
        &self,
        _: ops::cookies::Params,
        _: &api::Context<'_, ops::cookies::Security<Self>>,
    ) -> impl Future<Output = Result<ops::cookies::Response, api::ApiError>> + Send {
        let cookies = vec!["a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT".to_owned(), "b=2".to_owned()];
        ok(ops::cookies::Response::NoContent { set_cookie: Some(cookies) })
    }
}

fn send(method: &'static str, path: &str, headers: &[(&str, &str)], body: Option<&str>) -> api::Response {
    let mut r = honig::__private::new_request(method, path.to_owned(), vec![]).unwrap();
    r.headers_mut().append("authorization", honig::http::HeaderValue::try_from("Bearer t".to_owned()).unwrap());
    for (k, v) in headers {
        r.headers_mut().append(
            honig::http::HeaderName::try_from(*k).unwrap(),
            honig::http::HeaderValue::try_from((*v).to_owned()).unwrap(),
        );
    }
    if let Some(b) = body {
        r.headers_mut()
            .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
        *r.body_mut() = honig::Body::from(b.as_bytes().to_vec());
    }
    honig::block_on(api::Server::new(Server).into_service().handle(r))
}

/// JSON Schema 2020-12 section 10.3.2.3: with `additionalProperties: false` an
/// undeclared property fails validation.
#[test]
fn additional_properties_false_rejects_an_undeclared_property() {
    assert_eq!(send("POST", "/closed", &[], Some(r#"{"a":"x"}"#)).status().as_u16(), 204);
    let r = send("POST", "/closed", &[], Some(r#"{"a":"x","b":1}"#));
    assert_eq!(r.status().as_u16(), 400, "{}", String::from_utf8_lossy(r.body().bytes()));
    let body: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
    assert_eq!(
        body["errors"],
        serde_json::json!([{"detail": "is not allowed (additionalProperties)", "pointer": "#/b"}])
    );
}

/// An exploded form object (`?a=x`) shares the query with the other parameters
/// of the operation (`page`). They are not its properties even when it is
/// closed (`additionalProperties: false`).
#[test]
fn an_exploded_object_leaves_the_other_query_parameters_alone() {
    assert_eq!(send("GET", "/filter?a=x&page=2", &[], None).status().as_u16(), 204);
}

/// 3.2 Request Body Object `required` (default `false`).
#[test]
fn a_required_body_is_required_and_an_optional_one_is_not() {
    assert_eq!(send("POST", "/closed", &[], None).status().as_u16(), 400);
    assert_eq!(send("POST", "/optional-body", &[], None).status().as_u16(), 204);
}

/// 3.2 Discriminator Object: the value refers to a schema (implicit mapping). An
/// unknown one without `defaultMapping` matches nothing.
#[test]
fn a_discriminator_value_selects_a_schema_by_name() {
    assert_eq!(send("POST", "/products", &[], Some(r#"{"kind":"Gadget","purrs":true}"#)).status().as_u16(), 204);
    assert_eq!(send("POST", "/products", &[], Some(r#"{"kind":"Fish"}"#)).status().as_u16(), 400);
}

/// 3.2 Responses Object: `2XX` covers any 2xx status.
#[test]
fn a_status_range_answers_with_its_actual_status() {
    assert_eq!(send("GET", "/items/1", &[], None).status().as_u16(), 201);
}

/// 3.2 Security Requirement Object: `security: []` removes the top-level
/// requirement; an operation without `security` inherits it.
#[test]
fn an_empty_security_list_overrides_the_top_level_one() {
    let mut r = honig::__private::new_request("GET", "/public".to_owned(), vec![]).unwrap();
    assert_eq!(honig::block_on(api::Server::new(Server).into_service().handle(r)).status().as_u16(), 204);
    r = honig::__private::new_request("GET", "/inherits".to_owned(), vec![]).unwrap();
    assert_eq!(honig::block_on(api::Server::new(Server).into_service().handle(r)).status().as_u16(), 401);
}

/// 3.2 Header Object, "Representing the `Set-Cookie` Header": one value per
/// line because the `Expires` of a cookie contains a comma.
#[test]
fn set_cookie_values_go_on_separate_lines_and_come_back_whole() {
    let r = send("GET", "/cookies", &[("Cookie", "SID=s")], None);
    let lines: Vec<&str> = r.headers().get_all("set-cookie").iter().map(|v| v.to_str().unwrap()).collect();
    assert_eq!(lines, ["a=1; Expires=Wed, 21 Oct 2015 07:28:00 GMT", "b=2"]);
    let client =
        Client::new(support::loopback(|req| api::Server::new(Server).into_service().handle(req))).with_cookie_key("s");
    let resp = honig::block_on(client.cookies(ops::cookies::Params::default())).expect("204");
    let ops::cookies::Response::NoContent { set_cookie } = resp;
    assert_eq!(set_cookie.expect("cookies").len(), 2, "the comma inside `Expires` does not split a cookie");
}

/// RFC 9110 section 5.3 (3.2 Header Object): `X: a` + `X: b` equals `X: a,b`.
#[test]
fn a_header_sent_on_two_lines_reads_as_one_list() {
    // `limit` is a query parameter; `Authorization` lines must not merge into
    // anything else either: the request still authenticates.
    let mut r = honig::__private::new_request("GET", "/items/1".to_owned(), vec![]).unwrap();
    r.headers_mut().append("authorization", honig::http::HeaderValue::try_from("Bearer t".to_owned()).unwrap());
    assert_eq!(honig::block_on(api::Server::new(Server).into_service().handle(r)).status().as_u16(), 201);
    let mut headers = honig::http::HeaderMap::new();
    for (k, v) in [("X", "a"), ("x", "b"), ("Y", "c")] {
        headers.append(honig::http::HeaderName::try_from(k).unwrap(), honig::http::HeaderValue::from_static(v));
    }
    assert_eq!(honig::__private::joined_header(&headers, "X").as_deref(), Some("a,b"));
    assert_eq!(honig::__private::joined_header(&headers, "Z"), None);
}

/// 3.2 Encoding Object defaults: `integer`, `number` and `boolean` parts are
/// `text/plain`. Objects are `application/json`. Parts follow the order of the schema.
#[test]
fn multipart_parts_get_the_default_content_types_in_schema_order() {
    use std::sync::Mutex;
    static BODY: Mutex<String> = Mutex::new(String::new());
    let client = Client::new(support::loopback(|req: api::Request| {
        *BODY.lock().unwrap() = String::from_utf8_lossy(req.body().bytes()).into_owned();
        api::Server::new(Server).into_service().handle(req)
    }))
    .with_bearer("t");
    let body = serde_json::from_value(serde_json::json!({"name": "n", "count": 3, "meta": {"x": 1}})).expect("body");
    honig::block_on(client.upload(ops::upload::Params { body })).expect("204");
    let sent = BODY.lock().unwrap().clone();
    let part = |name: &str| {
        let at = sent.find(&format!("name=\"{name}\"")).unwrap_or_else(|| panic!("no {name} part in {sent}"));
        let ct = sent[at..].lines().nth(1).unwrap_or("").to_owned();
        (at, ct)
    };
    let (name, count, meta) = (part("name"), part("count"), part("meta"));
    assert_eq!(name.1, "Content-Type: text/plain");
    assert_eq!(count.1, "Content-Type: text/plain", "an integer part");
    assert_eq!(meta.1, "Content-Type: application/json");
    assert!(name.0 < count.0 && count.0 < meta.0, "schema order: {sent}");
}

/// 3.2 Responses Object: the client decodes a `2XX` range answer with its actual status. It leaves other
/// statuses to `default`.
#[test]
fn the_client_reads_a_status_range_with_its_actual_status() {
    let service = api::Server::new(Server).into_service();
    let client = Client::new(service).with_bearer("t");
    let got = honig::block_on(client.get_item(ops::get_item::Params::new("1".to_owned())));
    assert_eq!(
        got.ok(),
        Some(ops::get_item::Response::Success { status: ops::get_item::SuccessStatus::Created, body: "x".to_owned() })
    );
    let missing = Client::new(support::loopback(|_| {
        let mut r = honig::__private::new_response(404);
        r.headers_mut()
            .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
        *r.body_mut() = honig::Body::from(b"{}".to_vec());
        std::future::ready(r)
    }))
    .with_bearer("t");
    let got = honig::block_on(missing.get_item(ops::get_item::Params::new("1".to_owned())));
    assert_eq!(
        got.ok(),
        Some(ops::get_item::Response::Default {
            status: ops::get_item::DefaultStatus::NotFound,
            body: std::collections::BTreeMap::new()
        })
    );
}
