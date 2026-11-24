//! Hand-written checks for the `validation` fixture. They run against the module that `tests/regen.rs` keeps fresh.

use assert_matches::assert_matches;
use std::future::Future;

use e2e::v32_validation::api::{self, operations as ops, types};
use serde_json::json;

/// A field that the handler adds to the wide event.
const TENANT: api::EventKey = api::EventKey::new("tenant");

struct Users;

fn shipment(body: &serde_json::Value) -> api::Response {
    let mut req = api::runtime::new_request("POST", "/shipments".to_owned(), vec![]).unwrap();
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() = honig::Body::from(serde_json::to_vec(body).unwrap());
    api::block_on(api::Server::new(Users).into_service().handle(req))
}

/// Each entry of the RFC 9457 `errors` member of a 400 response: its `pointer` and `detail`.
fn violations(r: &api::Response) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap_or_default();
    v["errors"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| format!("{} {}", x["pointer"].as_str().unwrap_or(""), x["detail"].as_str().unwrap_or("")))
        .collect()
}

#[test]
fn untyped_keywords_pass_when_satisfied() {
    let ok = shipment(
        &json!({"kind":"domestic","parcels":[{"fragile":true},{"fragile":false}],"labels":{"xa":"ab","team":"core"}}),
    );
    assert_eq!(ok.status().as_u16(), 201, "{:?}", violations(&ok));
}

/// Each keyword is reported on its own: a request is refused at its first violation.
#[test]
fn untyped_keywords_report_each_failure() {
    let ok = || json!({"kind":"domestic","parcels":[{"fragile":true}]});
    let with = |key: &str, value: serde_json::Value| {
        let mut body = ok();
        body[key] = value;
        body
    };
    for (body, expected) in [
        (with("insured", json!(true)), "`insurer` is required when `insured` is present"), // dependentRequired
        (with("parcels", json!([{"fragile": false}])), "must contain at least 1 matching item(s), found 0"), // contains
        (with("labels", json!({"Bad": "x"})), "does not match `^[a-z]+$`"),                // propertyNames
        (with("labels", json!({"xlong": "abcd"})), "at most 3 characters allowed, found 4"), // patternProperties
    ] {
        let r = shipment(&body);
        assert_eq!(r.status().as_u16(), 400, "{body}");
        let v = violations(&r).join("\n");
        assert!(v.contains(expected), "missing `{expected}` in:\n{v}");
    }
}

/// `if kind == international then required: [customs]` is typed. `customs` is a
/// `String` in the `International` variant. So a body without it does not decode.
#[test]
fn if_then_requirement_is_a_decode_error() {
    let r = shipment(&json!({"kind": "international", "parcels": [{"fragile": true}]}));
    assert_eq!(r.status().as_u16(), 400);
    assert!(
        String::from_utf8_lossy(r.body().bytes()).contains("customs"),
        "{}",
        String::from_utf8_lossy(r.body().bytes())
    );
}

/// A request is refused at its first violation: each value here breaks one keyword.
#[test]
fn not_and_max_contains_reject() {
    let r = shipment(&json!({"kind":"domestic","customs":"none","insurer":"acme","parcels":[{"fragile":true}]}));
    let v = violations(&r).join("\n");
    assert!(v.contains("# matches `not`"), "{v}");
    let r = shipment(&json!({"kind":"domestic","parcels":[{"fragile":true},{"fragile":true},{"fragile":true}]}));
    let v = violations(&r).join("\n");
    assert!(v.contains("contains 3 matching items, more than allowed"), "{v}");
}

impl api::Api for Users {
    fn error_response(rejection: api::Rejection) -> api::Response {
        rejection.response()
    }

    fn login(
        &self,
        p: ops::login::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::login::Response, api::ApiError>> + Send {
        // The request type has the password and no token; the response type the reverse.
        let types::LoginRequest { user, password: _ } = p.body;
        let reply = types::Login { user, token: "t0k".to_owned() };
        std::future::ready(Ok(ops::login::Response::Created(reply)))
    }
    fn create_shipment(
        &self,
        _: ops::create_shipment::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::create_shipment::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::create_shipment::Response::Created))
    }
    fn create_user(
        &self,
        p: ops::create_user::Params,
        _ctx: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::create_user::Response, api::ApiError>> + Send {
        std::future::ready(if p.body.name.as_str() == "Taken" {
            let detail = "the name is taken".to_owned();
            let body = types::Problem { code: types::ProblemCode::Taken, status: 409, detail: detail.clone() };
            Err(api::ApiError::new(api::http::StatusCode::CONFLICT, detail)
                .with_body("application/problem+json", &body))
        } else {
            Ok(ops::create_user::Response::Created(Box::new(p.body)))
        })
    }
    fn add_member(
        &self,
        _: ops::add_member::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::add_member::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::add_member::Response::Created))
    }
    fn add_badge(
        &self,
        _: ops::add_badge::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::add_badge::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::add_badge::Response::Created))
    }
    fn add_closed(
        &self,
        _: ops::add_closed::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::add_closed::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::add_closed::Response::Created))
    }
}

fn post(path: &str, media: &str, body: Vec<u8>) -> api::Response {
    let mut req = api::runtime::new_request("POST", path.to_owned(), vec![]).unwrap();
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::try_from(media.to_owned()).unwrap());
    *req.body_mut() = honig::Body::from(body);
    api::block_on(api::Server::new(Users).into_service().handle(req))
}

/// A body declared with several media types is checked whichever one arrives.
#[test]
fn every_media_type_of_a_body_is_validated() {
    let ok = post("/members", "application/json", serde_json::to_vec(&json!({"name":"Ann","role":"admin"})).unwrap());
    assert_eq!(ok.status().as_u16(), 201, "{}", String::from_utf8_lossy(ok.body().bytes()));
    let json = post(
        "/members",
        "application/json",
        serde_json::to_vec(&json!({"name":"Ann","role":"admin","age":150})).unwrap(),
    );
    assert_eq!(json.status().as_u16(), 400, "{}", String::from_utf8_lossy(json.body().bytes()));
    assert!(violations(&json).join("\n").contains("#/age"), "{:?}", violations(&json));
    let form = post("/members", "application/x-www-form-urlencoded", b"name=Ann&role=admin&age=150".to_vec());
    assert_eq!(form.status().as_u16(), 400, "{}", String::from_utf8_lossy(form.body().bytes()));
    assert!(violations(&form).join("\n").contains("#/age"), "{:?}", violations(&form));
}

/// Untyped keywords of a several-media-types body are checked too. The
/// request leaves out the required `readOnly` id.
#[test]
fn untyped_keywords_of_a_body_enum_are_checked_without_read_only_fields() {
    let body = |v: serde_json::Value| serde_json::to_vec(&v).unwrap();
    let ok = post("/badges", "application/json", body(json!({"label":"gold","holders":["owner","ann"]})));
    assert_eq!(ok.status().as_u16(), 201, "{:?}", violations(&ok));
    let bad = post("/badges", "application/json", body(json!({"label":"gold","holders":["ann"]})));
    assert_eq!(bad.status().as_u16(), 400, "{}", String::from_utf8_lossy(bad.body().bytes()));
    let v = violations(&bad).join("\n");
    assert!(v.contains("#/holders must contain at least 1"), "{v}");
    assert!(!v.contains("#/id"), "a request has no readOnly id: {v}");
}

fn send(query: &str, team: &str, body: &serde_json::Value) -> api::Response {
    let mut req = api::runtime::new_request("POST", "/users".to_owned(), vec![query.to_owned()]).unwrap();
    req.headers_mut().append("x-team", honig::http::HeaderValue::try_from(team.to_owned()).unwrap());
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() = honig::Body::from(serde_json::to_vec(body).unwrap());
    api::block_on(api::Server::new(Users).into_service().handle(req))
}

#[test]
fn valid_requests_reach_the_handler() {
    let r = send("limit=5", "core", &json!({"name":"Ann","role":"admin","age":30,"score":4.5,"tags":["a","b"]}));
    assert_eq!(r.status().as_u16(), 201, "{}", String::from_utf8_lossy(r.body().bytes()));
}

/// Decoding stops at the first value that breaks its schema. The request is refused for that value, at its path.
#[test]
fn the_first_violation_is_reported_with_its_path() {
    let first = |query: &str, body: &serde_json::Value| -> serde_json::Value {
        let r = send(query, "core", body);
        assert_eq!(r.status().as_u16(), 400);
        let v: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
        v["errors"].clone()
    };
    let ok = json!({"name": "Ann", "role": "admin"});
    assert_eq!(first("limit=0", &ok), json!([{"detail": "0 is less than the minimum 1", "pointer": "#/query/limit"}]));
    let old = json!({"name": "Ann", "role": "admin", "age": 150, "score": 1.2});
    assert_eq!(first("limit=5", &old)[0]["pointer"], "#/age");
}

/// An enum is closed: a value that it does not list fails to decode. The request is refused for that value alone.
#[test]
fn an_unlisted_enum_value_is_a_bad_request() {
    let r = send("limit=5", "core", &json!({"name":"Ann","role":"root","age":150}));
    assert_eq!(r.status().as_u16(), 400);
    let v: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
    assert_eq!(v["errors"], json!([{"detail": "`root` is not one of `admin`, `user`", "pointer": "#/role"}]));
}

/// A string with a `pattern` is a type that only `TryFrom` (and decoding) builds.
/// A bad value is refused where it is made. A request that carries one is a
/// bad request and not a list of violations.
#[test]
fn pattern_fields_are_checked_types() {
    assert_eq!(types::UserName::try_from("Ann").map(types::UserName::into_inner), Ok("Ann".to_owned()));
    assert!(types::UserName::try_from("ann").is_err(), "must start with an uppercase letter");
    assert!(types::UserName::try_from("A").is_err(), "at least 2 characters");
    assert!(types::UserName::try_from("Abcdefghijk").is_err(), "at most 10 characters");
    let name = types::UserName::try_from("Ann").unwrap();
    assert_eq!(name.as_str(), "Ann", "derefs to `str`");
    let r = send("limit=5", "core", &json!({"name": "ann", "role": "admin"}));
    assert_eq!(r.status().as_u16(), 400);
    assert!(
        String::from_utf8_lossy(r.body().bytes()).contains("does not match"),
        "{}",
        String::from_utf8_lossy(r.body().bytes())
    );
    let r = send("limit=5", "Core1", &json!({"name": "Ann", "role": "admin"}));
    assert_eq!(r.status().as_u16(), 400, "the X-Team header has a pattern too");
}

/// Records every wide event; the handler adds its own field.
#[derive(Default)]
struct Recording(std::sync::Mutex<Vec<serde_json::Map<String, serde_json::Value>>>);

impl api::Api for Recording {
    fn error_response(rejection: api::Rejection) -> api::Response {
        rejection.response()
    }

    fn login(
        &self,
        p: ops::login::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::login::Response, api::ApiError>> + Send {
        let reply = types::Login { user: p.body.user, token: String::new() };
        std::future::ready(Ok(ops::login::Response::Created(reply)))
    }
    fn create_shipment(
        &self,
        _: ops::create_shipment::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::create_shipment::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::create_shipment::Response::Created))
    }
    fn create_user(
        &self,
        p: ops::create_user::Params,
        ctx: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::create_user::Response, api::ApiError>> + Send {
        ctx.event.set(TENANT, "acme");
        std::future::ready(Ok(ops::create_user::Response::Created(Box::new(p.body))))
    }
    fn add_member(
        &self,
        _: ops::add_member::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::add_member::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::add_member::Response::Created))
    }
    fn add_badge(
        &self,
        _: ops::add_badge::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::add_badge::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::add_badge::Response::Created))
    }
    fn add_closed(
        &self,
        _: ops::add_closed::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::add_closed::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::add_closed::Response::Created))
    }
}

#[test]
fn one_wide_event_per_request() {
    let api = std::sync::Arc::new(Recording::default());
    let sink = std::sync::Arc::clone(&api);
    let service = api::Server::new(std::sync::Arc::clone(&api))
        .on_event(move |event| sink.0.lock().unwrap().push(event.fields()))
        .into_service();
    for body in
        [json!({"name": "Ann", "role": "user"}), json!({"name": "Ann", "role": "user", "age": 150, "score": 1.2})]
    {
        let mut req = api::runtime::new_request("POST", "/users".to_owned(), vec![]).unwrap();
        req.headers_mut()
            .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
        *req.body_mut() = honig::Body::from(serde_json::to_vec(&body).unwrap());
        api::block_on(service.handle(req));
    }
    let events = api.0.lock().unwrap();
    assert_eq!(events.len(), 2);
    let ok = &events[0];
    assert_eq!(ok["operation"], "create_user");
    assert_eq!(ok["route"], "/users");
    assert_eq!(ok["status"], 201);
    assert_eq!(ok["tenant"], "acme");
    assert!(ok["duration_us"].is_u64() && ok["request_bytes"].as_u64() > Some(0));
    let bad = &events[1];
    assert_eq!(bad["status"], 400);
    assert_eq!(bad["error"], "invalid");
    assert_eq!(bad["violations"], 1);
    assert!(bad.get("tenant").is_none(), "the handler must not run");
}

#[test]
fn pattern_properties_type_their_values() {
    let s: types::Shipment =
        serde_json::from_value(json!({"kind":"domestic","parcels":[{"fragile":true}],"scores":{"speed":3}})).unwrap();
    // `if kind == international then customs is required`: a domestic shipment is the `Otherwise` variant.
    let types::ShipmentVariants::Otherwise(s) = s.into_inner() else { panic!("a domestic shipment") };
    let scores = s.scores.as_ref().expect("present");
    assert_eq!(scores["speed"], 3);
    let bad = shipment(&json!({"kind":"domestic","parcels":[{"fragile":true}],"scores":{"Speed":-1}}));
    assert_eq!(bad.status().as_u16(), 400, "key pattern and value minimum are both checked");
}

#[test]
fn an_object_that_allows_no_property_accepts_only_the_empty_object() {
    assert!(serde_json::from_value::<types::NoOptions>(json!({})).is_ok());
    assert!(serde_json::from_value::<types::NoOptions>(json!({"x": 1})).is_err());
    assert!(serde_json::from_value::<types::NoOptions>(json!([])).is_err());
}

#[test]
fn a_schema_without_type_still_checks_and_a_bad_default_is_not_filled_in() {
    let base = json!({"kind":"domestic","parcels":[{"fragile":true}]});
    assert_eq!(shipment(&base).status().as_u16(), 201, "no `label`: its bad default is not filled in");
    let mut low = base.clone();
    low["rating"] = json!(0);
    assert_eq!(shipment(&low).status().as_u16(), 400, "`minimum: 1` applies to a number without `type`");
    low["rating"] = json!("any text");
    assert_eq!(shipment(&low).status().as_u16(), 201, "`minimum` says nothing about a string");
}

#[test]
fn properties_without_type_hold_for_an_object_and_any_other_value_passes() {
    let with = |meta: serde_json::Value| {
        let body = json!({"kind": "domestic", "parcels": [{"fragile": true}], "meta": meta});
        shipment(&body).status().as_u16()
    };
    assert_eq!(with(json!({"count": 2})), 201);
    assert_eq!(with(json!("free text")), 201, "not an object: `properties` says nothing about it");
    assert_eq!(with(json!({"count": "two"})), 400, "an object gets the properties");
}

#[test]
fn the_bounds_of_a_union_branch_are_checked() {
    let user = |extra: serde_json::Value| {
        let mut body = json!({"name": "Ann", "role": "user"});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        violations(&send("limit=5", "core", &body))
    };
    assert!(user(json!({"loyalty": 4, "note": "fine"})).is_empty());
    assert!(user(json!({"loyalty": "VIP", "note": null})).is_empty());
    assert_eq!(user(json!({"loyalty": 11})), ["#/loyalty 11 is greater than the maximum 10"]);
    assert_eq!(user(json!({"loyalty": "GOLD"})), ["#/loyalty at most 3 characters allowed, found 4"]);
    assert_eq!(user(json!({"note": "too long"})), ["#/note at most 5 characters allowed, found 8"]);
}

fn login(body: &serde_json::Value) -> api::Response {
    let mut req = api::runtime::new_request("POST", "/logins".to_owned(), vec![]).unwrap();
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() = honig::Body::from(serde_json::to_vec(body).unwrap());
    api::block_on(api::Server::new(Users).into_service().handle(req))
}

#[test]
fn required_write_only_property_is_required_in_requests() {
    // `password` is a plain `String` in `LoginRequest`: a body without it does not decode.
    let r = login(&json!({"user": "ann"}));
    assert_eq!(r.status().as_u16(), 400, "{}", String::from_utf8_lossy(r.body().bytes()));
    assert!(
        String::from_utf8_lossy(r.body().bytes()).contains("password"),
        "{}",
        String::from_utf8_lossy(r.body().bytes())
    );
}

#[test]
fn write_only_constraints_still_apply_and_read_only_may_be_omitted() {
    assert_eq!(login(&json!({"user": "ann", "password": "short"})).status().as_u16(), 400);
    let r = login(&json!({"user": "ann", "password": "long enough"}));
    assert_eq!(r.status().as_u16(), 201, "{}", String::from_utf8_lossy(r.body().bytes()));
    let back: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
    assert_eq!(back, json!({"user": "ann", "token": "t0k"}));
}

#[test]
fn a_lone_if_then_types_its_requirement() {
    // `customs` is a `String` in the `International` variant: it cannot be left out.
    let s: types::Shipment =
        serde_json::from_value(json!({"kind": "international", "customs": "cn22", "parcels": [{"fragile": true}]}))
            .unwrap();
    assert_matches!(&*s, types::ShipmentVariants::International(i) if i.customs == "cn22");
}

fn create_user(body: &serde_json::Value) -> api::Response {
    let mut req = api::runtime::new_request("POST", "/users".to_owned(), vec![]).unwrap();
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() = honig::Body::from(serde_json::to_vec(body).unwrap());
    api::block_on(api::Server::new(Users).into_service().handle(req))
}

/// The own error type of the handler goes out as it is at its status.
#[test]
fn a_typed_error_is_sent_as_it_is() {
    let r = create_user(&json!({"name": "Taken", "role": "user"}));
    assert_eq!(
        (r.status().as_u16(), r.headers().get("content-type").map_or("", |v| v.to_str().unwrap_or(""))),
        (409, "application/problem+json")
    );
    let body: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
    assert_eq!(body, json!({"code": "TAKEN", "status": 409, "detail": "the name is taken"}));
}

/// `Problem` requires a `code` from a closed list and accepts no other property. The runtime cannot fill it for a
/// request that it refuses itself. So it answers RFC 9457 and not a body that breaks the schema. Nor does it borrow
/// `Refusal`, which `createShipment` declares and the runtime can fill.
#[test]
fn an_unfillable_error_schema_falls_back_to_rfc_9457() {
    let r = create_user(&json!({"name": "Ann", "role": "user", "age": 200}));
    assert_eq!(
        (r.status().as_u16(), r.headers().get("content-type").map_or("", |v| v.to_str().unwrap_or(""))),
        (400, "application/problem+json")
    );
    let body: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
    assert_eq!((body["title"].as_str(), body.get("code")), (Some("Bad Request"), None), "{body}");
    assert_eq!(violations(&r), ["#/age 200 must be less than 150"], "{body}");
}

/// `email` and `idn-hostname` are types that only a valid value can be (`api::EmailStr` and `api::IdnHostnameStr`). A
/// request that contains another value is refused while decoding.
#[test]
fn mail_addresses_and_host_names_are_types() {
    let ok = json!({"name": "Ann", "role": "user", "email": "ann@example.com", "site": "bücher.example"});
    assert_eq!(send("limit=5", "core", &ok).status().as_u16(), 201);
    for (field, bad) in [("email", "x"), ("site", "-bad-.example")] {
        let mut body = ok.clone();
        body[field] = json!(bad);
        let r = send("limit=5", "core", &body);
        assert_eq!(r.status().as_u16(), 400);
        let text = String::from_utf8_lossy(r.body().bytes());
        assert!(text.contains(&format!("`{bad}` is not a valid")), "{text}");
    }
    assert!(api::EmailStr::try_from("x").is_err(), "nor can a handler build one");
}

/// `multipleOf` on a number divides and allows 1e-9: decimal multiples a binary float cannot represent exactly pass.
#[test]
fn a_decimal_multiple_passes_although_its_float_is_inexact() {
    let price = |p: f64| serde_json::from_value::<types::Price>(json!({"amount": p})).map_err(|e| e.to_string());
    assert!(price(0.07).is_ok(), "0.07 / 0.01 is 7.000000000000001");
    assert!(price(19.99).is_ok());
    assert_eq!(price(0.015).err(), Some("is not a multiple of 0.01".to_owned()));
}

/// A refusal of the server goes out in the schema that its operation declares, its `const` at its value.
#[test]
fn a_refusal_fills_the_declared_schema() {
    let r = shipment(&json!({"kind": "domestic", "parcels": "none"}));
    assert_eq!(
        (r.status().as_u16(), r.headers().get("content-type").map_or("", |v| v.to_str().unwrap_or(""))),
        (400, "application/problem+json")
    );
    let body: serde_json::Value = serde_json::from_slice(r.body().bytes()).unwrap();
    // The body decodes as its type: the wrong type is a violation of the whole body.
    let detail = "/body: invalid type: string \"none\", expected a sequence";
    let errors = json!([{"detail": "invalid type: string \"none\", expected a sequence", "pointer": "#"}]);
    assert_eq!(body, json!({"detail": detail, "object": "error", "errors": errors}));
}

/// JSON Schema 2020-12 section 6.1.1: `type: null` allows only `null`. Absent, it is `None`. `null` is `Some(())`,
/// which writes `null` again. Any other value is refused.
#[test]
fn a_null_only_property_takes_only_null() {
    let decode = |body: serde_json::Value| {
        serde_json::from_value::<types::Price>(body).map(|p| p.retired).map_err(|e| e.to_string())
    };
    assert_eq!(decode(json!({})), Ok(None));
    assert_eq!(decode(json!({"retired": null})), Ok(Some(())));
    assert_eq!(decode(json!({"retired": "abc"})), Err("invalid type: string \"abc\", expected unit".to_owned()));
    let sent = types::Price { retired: Some(()), ..types::Price::default() };
    assert_eq!(serde_json::to_value(&sent).unwrap(), json!({"retired": null}));
}

/// An `allOf` item whose members both declare `lines` enforces both: the field is the `allOf` of the two schemas.
/// Keywords deep inside keep their values: `type: [string, null]` still accepts a string and `null`, `maxLength` still
/// applies, and so does the `maxItems` of the second member.
#[test]
fn a_whole_checked_schema_keeps_its_deep_keywords() {
    let check = |lines: serde_json::Value| {
        serde_json::from_value::<types::Ledger>(json!({"entries": [{"id": 1, "lines": lines}]}))
            .map(drop)
            .map_err(|e| e.to_string())
    };
    assert_eq!(check(json!([{"note": "ok"}, {"note": null}])), Ok(()));
    let long = check(json!([{"note": "long"}])).expect_err("past maxLength");
    assert!(long.contains("at most 3 characters allowed, found 4"), "{long}");
    let many = check(json!([{}, {}, {}])).expect_err("past maxItems");
    assert!(many.contains("at most 2 items allowed, found 3"), "{many}");
}

/// `^(a+)+$` needs exponential time on a long value that almost matches. `handle` is a pattern type: past `maxLength`
/// it refuses the value before its pattern runs. Run, 30,001 characters would not finish.
#[test]
fn a_pattern_does_not_run_past_max_length() {
    let handle = format!("{}b", "a".repeat(30_000));
    let err =
        serde_json::from_value::<types::User>(json!({"name": "Alice", "role": "admin", "handle": handle})).unwrap_err();
    assert_eq!(err.to_string(), "at most 20 characters allowed, found 30001");
}

/// A recursive union whose branches differ in a field after the recursive one. A branch that cannot fit is skipped
/// before decoding, so depth 40 decodes in steps linear in its depth, and the stack of a test thread fits it in a
/// debug build.
#[test]
fn a_recursive_union_decodes_in_time_linear_in_its_depth() {
    let mut json = String::from(r#"{"items":[],"x":1}"#);
    for _ in 0..40 {
        json = format!(r#"{{"items":[{json}],"x":1}}"#);
    }
    assert!(serde_json::from_str::<types::Tree>(&json).is_ok());
}

/// A builder accepts raw values for its fields and converts each one. `build` returns the first that did not convert,
/// named as the description spells it. The required properties are the arguments of `builder`.
#[test]
fn a_builder_converts_each_field_and_returns_the_first_error() -> Result<(), Box<dyn std::error::Error>> {
    // ANCHOR: builder
    let user = types::User::builder().name("Anna").role("admin").age(30).email("anna@example.com").build()?;
    // ANCHOR_END: builder
    assert_eq!(
        (user.name.as_str(), user.role, user.age.map(types::UserAge::get)),
        ("Anna", types::UserRole::Admin, Some(30))
    );
    let err = types::User::builder()
        .name("Anna")
        .role("admin")
        .age(150)
        .email("x")
        .build()
        .expect_err("150 is past the limit");
    assert_eq!(err.to_string(), "/age: 150 must be less than 150", "the first error");
    let err = types::User::builder().name("anna").role("admin").age(30).build().expect_err("a lowercase name");
    assert_eq!(err.violation().instance_location, "/name", "{err}");
    assert!(err.to_string().starts_with("/name: "), "{err}");
    Ok(())
}

/// A decimal is checked on its digits: `0.09999999999999999999` is below `minimum: 0.1` although its nearest `f64` is
/// `0.1`, and `100000000000000000.001` is not a multiple of `0.01` although every `f64` quotient that large is whole.
#[test]
fn decimal_keywords_are_exact() {
    let decodes = |v: &str| serde_json::from_str::<types::Exact>(v).map_err(|e| e.to_string());
    assert!(decodes(r#"{"tenth": 0.1}"#).is_ok());
    let below = decodes(r#"{"tenth": 0.09999999999999999999}"#).expect_err("below 0.1");
    assert!(below.starts_with("0.09999999999999999999 is less than the minimum 0.1"), "{below}");
    assert!(decodes(r#"{"cents": 100000000000000000.01}"#).is_ok());
    let past = decodes(r#"{"cents": 100000000000000000.001}"#).expect_err("not a multiple of 0.01");
    assert!(past.starts_with("is not a multiple of 0.01"), "{past}");
    assert!(decodes(r#"{"cents": 0.07}"#).is_ok(), "0.07 / 0.01 is not a whole f64");
}

/// Integer keywords are checked exactly: a bound that no value of the type meets refuses every value, a fractional
/// `multipleOf` accepts the integers that are multiples of it, and one past the type accepts only `0`.
#[test]
fn integer_keywords_are_exact() {
    let decodes = |v: serde_json::Value| serde_json::from_value::<types::Degenerate>(v).map_err(|e| e.to_string());
    for never in [0, 1, 255] {
        let e = decodes(json!({"never": never})).expect_err("`maximum: -1` leaves no uint8");
        let message = format!("`maximum: -1` leaves no value of this integer type, not even {never}");
        assert!(e.starts_with(&message), "{e}");
    }
    assert!(decodes(json!({"huge": i64::MAX})).is_err(), "`minimum: 1e40` leaves no int64");
    for (fives, ok) in [(0, true), (5, true), (-15, true), (1, false), (2, false), (25, true)] {
        assert_eq!(decodes(json!({"fives": fives})).is_ok(), ok, "{fives} as a multiple of 2.5");
    }
    assert!(decodes(json!({"zero": 0})).is_ok());
    assert!(decodes(json!({"zero": 200})).is_err(), "a uint8 multiple of 1000 is 0");
    for (half, ok) in [(0, false), (1, true), (3, true), (4, false)] {
        assert_eq!(decodes(json!({"half": half})).is_ok(), ok, "{half} in [0.5, 3.5)");
    }
}

/// `additionalProperties: false` applies in every media type of a body: a field that the schema does not declare is a
/// bad request as JSON, as a form and as XML.
#[test]
fn a_closed_object_refuses_unknown_fields_in_every_media_type() {
    let cases: [(&str, &str, &str); 3] = [
        ("application/json", r#"{"name":"a"}"#, r#"{"name":"a","extra":"b"}"#),
        ("application/x-www-form-urlencoded", "name=a", "name=a&extra=b"),
        ("application/xml", "<closed><name>a</name></closed>", "<closed><name>a</name><extra>b</extra></closed>"),
    ];
    for (media, ok, extra) in cases {
        let r = post("/closed", media, ok.as_bytes().to_vec());
        assert_eq!(r.status().as_u16(), 201, "{media}: {}", String::from_utf8_lossy(r.body().bytes()));
        let r = post("/closed", media, extra.as_bytes().to_vec());
        assert_eq!(
            r.status().as_u16(),
            400,
            "{media} with an unknown field: {}",
            String::from_utf8_lossy(r.body().bytes())
        );
    }
}

/// An `anyOf` decodes as its first branch that accepts the value, keywords and all. A value that only the second branch
/// accepts decodes, and one that no branch accepts does not.
#[test]
fn an_any_of_takes_the_first_branch_that_decodes() {
    for ok in [r#""""#, r#""abc""#] {
        assert!(serde_json::from_str::<types::Slug>(ok).is_ok(), "{ok}");
    }
    assert!(serde_json::from_str::<types::Slug>(r#""ABC""#).is_err());
}

/// A `oneOf` decodes every branch and accepts a value that exactly one accepts: `ab` fits both and is refused.
#[test]
fn a_one_of_takes_a_value_that_exactly_one_branch_takes() {
    for ok in [r#""AB""#, r#""abcd""#] {
        assert!(serde_json::from_str::<types::Code>(ok).is_ok(), "{ok}");
    }
    let both = serde_json::from_str::<types::Code>(r#""ab""#).unwrap_err().to_string();
    assert!(both.contains("matches 2 of `oneOf`, not exactly one"), "{both}");
    assert!(serde_json::from_str::<types::Code>(r#""ABCD""#).is_err(), "neither");
}

/// `enum: [true]` is a constant: it decodes `true` and nothing else, with no check of the whole value.
#[test]
fn a_one_value_enum_is_a_constant() {
    assert!(serde_json::from_str::<types::AlwaysTrue>("true").is_ok());
    assert!(serde_json::from_str::<types::AlwaysTrue>("false").is_err());
    const { assert!(types::AlwaysTrue::VALUE) };
}

/// A property that two `allOf` members declare must meet both: here a string of at most 5 characters, never `null`.
/// The struct stores it as one typed field, with no check of the whole value.
#[test]
fn a_property_of_two_all_of_members_meets_both() {
    let ok: types::NarrowStamp = serde_json::from_str(r#"{"at":"12:00"}"#).expect("both pass");
    assert_eq!(ok.at.as_str(), "12:00");
    // A plain struct: no checked type around its fields.
    let built = types::NarrowStamp { at: "12:00".try_into().expect("short") };
    assert_eq!(built, ok);
    for bad in [r#"{"at":null}"#, r#"{"at":"12:00:00"}"#, "{}"] {
        assert!(serde_json::from_str::<types::NarrowStamp>(bad).is_err(), "{bad}");
    }
}

/// `unevaluatedProperties` beside `anyOf` sees the properties of each branch that matches, and refuses the rest.
#[test]
fn unevaluated_properties_beside_any_of_sees_the_matching_branches() {
    let tagged = |v: serde_json::Value| serde_json::from_value::<types::Tagged>(v);
    assert!(tagged(json!({"label": "a", "colour": "red", "size": 1})).is_ok());
    assert!(tagged(json!({"label": "a", "shape": "round"})).is_err(), "a property that no branch evaluates");
}

/// `dependentRequired` over declared properties is a check of the fields: a card needs its code and its holder, which
/// may be `null` but not absent.
#[test]
fn dependent_required_is_a_check_of_the_fields() {
    let decode = |v: serde_json::Value| serde_json::from_value::<types::Payment>(v);
    assert!(decode(json!({"card": "4111", "cvc": "123", "holder": null})).is_ok());
    assert!(decode(json!({"note": "cash"})).is_ok(), "no card: nothing else is required");
    let missing = decode(json!({"card": "4111", "holder": "Ann"})).expect_err("no cvc");
    assert!(missing.to_string().contains("`cvc` is required when `card` is present"), "{missing}");
    assert!(decode(json!({"card": "4111", "cvc": "123"})).is_err(), "an absent holder is not a null one");
}

/// `propertyNames` that checks only text is a check of each name: of a map's keys, and of a struct's present
/// properties and the names it does not declare.
#[test]
fn property_names_is_a_check_of_each_name() {
    let labels = |v: serde_json::Value| serde_json::from_value::<types::Labelled>(json!({"labels": v}));
    assert!(labels(json!({"env": "prod", "team": "core"})).is_ok());
    assert!(labels(json!({"Env": "prod"})).is_err(), "not lowercase");
    assert!(labels(json!({"environment": "prod"})).is_err(), "longer than 8");
    let profile = |v: serde_json::Value| serde_json::from_value::<types::Profile>(v);
    assert!(profile(json!({"name": "Ann", "extra": 1})).is_ok());
    assert!(profile(json!({"Nick-Name": "A"})).is_err(), "a declared property whose name fails");
    assert!(profile(json!({"name": "Ann", "Other": 1})).is_err(), "a name the struct does not declare");
}

/// One `patternProperties` entry of a closed map: every name matches the pattern and every value decodes as its schema.
#[test]
fn a_closed_map_of_one_pattern_checks_names_and_values() {
    let scores = |v: serde_json::Value| serde_json::from_value::<types::Scored>(json!({"scores": v}));
    assert!(scores(json!({"alice": 3, "bob": 0})).is_ok());
    assert!(scores(json!({"Alice": 3})).is_err(), "a name the pattern does not match");
    assert!(scores(json!({"alice": -1})).is_err(), "a value below its minimum");
    assert!(scores(json!({"alice": "three"})).is_err(), "a value of another type");
}

/// Keywords without `type` apply to values of their kind: `minimum` to a number, `maxLength` to a string, nothing to a
/// boolean.
#[test]
fn keywords_without_a_type_apply_to_their_kind() {
    let loose = |v: serde_json::Value| serde_json::from_value::<types::Loose>(json!({"v": v}));
    for ok in [json!(5), json!("ab"), json!(true)] {
        assert!(loose(ok.clone()).is_ok(), "{ok}");
    }
    assert!(loose(json!(0)).is_err(), "a number below its minimum");
    assert!(loose(json!("abcd")).is_err(), "a string past its length");
}

/// Keywords of each kind beside an implied shape check the values of their kind, decoded from the text or built.
#[test]
fn keywords_beside_an_implied_shape_check_their_kind() {
    let from_text = |v: &serde_json::Value| serde_json::from_str::<types::Implied>(&v.to_string()).is_ok();
    let built = |v: &serde_json::Value| serde_json::from_value::<types::Implied>(v.clone()).is_ok();
    for ok in [json!({"name": "a"}), json!("ab"), json!(1.5), json!([1, 2]), json!(true)] {
        assert!(from_text(&ok) && built(&ok), "{ok}");
    }
    for wrong in [json!({}), json!("a"), json!(0), json!([1, 1.0])] {
        assert!(!from_text(&wrong) && !built(&wrong), "{wrong}");
    }
}

/// A `oneOf` of branches that only list `required` takes a value with exactly one of the names.
#[test]
fn exactly_one_required_name_of_a_one_of() {
    let reach = |v: serde_json::Value| serde_json::from_str::<types::Reach>(&v.to_string()).is_ok();
    assert!(reach(json!({"email": "a"})) && reach(json!({"phone": "1"})));
    assert!(!reach(json!({"email": "a", "phone": "1"})) && !reach(json!({})));
}

/// `dependentSchemas` over declared properties: when the property is present, the value decodes as the type of its
/// schema.
#[test]
fn dependent_schemas_hold_when_the_property_is_present() {
    let decode = |v: serde_json::Value| serde_json::from_value::<types::Voucher>(v);
    assert!(decode(json!({"code": "x", "amount": 50})).is_ok());
    assert!(decode(json!({"amount": 500})).is_ok(), "no code: the schema does not apply");
    let missing = decode(json!({"code": "x"})).expect_err("no amount");
    assert!(missing.to_string().contains("`code` is present"), "{missing}");
    assert!(decode(json!({"code": "x", "amount": 500})).is_err(), "an amount over 100");
}

/// `patternProperties` beside properties: each undeclared name decodes into the map of its pattern, as every pattern
/// that matches it, and `additionalProperties: false` refuses a name that none matches.
#[test]
fn settings_decode_each_name_into_its_pattern() {
    let decode = |v: serde_json::Value| serde_json::from_value::<types::Settings>(v);
    let settings = decode(json!({"name": "a", "x_a": "abc", "b_n": 2})).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(settings.pattern_1.len(), 1);
    assert_eq!(settings.pattern_2.len(), 1);
    assert_eq!(serde_json::to_value(&settings).ok(), Some(json!({"name": "a", "x_a": "abc", "b_n": 2})));
    assert!(decode(json!({"x_a": "abcd"})).is_err(), "longer than the pattern's maxLength");
    assert!(decode(json!({"b_n": -1})).is_err(), "under the pattern's minimum");
    assert!(decode(json!({"other": 1})).is_err(), "a name that no pattern matches");
    // `x_n` matches both patterns: it is a string of `^x_` and an integer of `_n$`, which no value is.
    assert!(decode(json!({"x_n": "1"})).is_err(), "a name of both patterns meets both");
}

/// `minProperties` and `maxProperties` on a struct count the declared properties sent and the names it keeps.
#[test]
fn a_pair_counts_its_properties() {
    let decode = |v: serde_json::Value| serde_json::from_value::<types::Pair>(v);
    assert!(decode(json!({"a": "x", "b": "y"})).is_ok());
    assert!(decode(json!({"a": "x", "c": "z"})).is_ok(), "a kept name counts as one");
    assert!(decode(json!({"a": "x", "b": "y", "c": "z"})).is_err(), "three is more than two");
    assert!(decode(json!({})).is_err(), "none is fewer than one");
}

/// A map component with keywords of its own is a checked type: a value built in code meets them too, not only one that
/// a field decodes.
#[test]
fn a_map_component_checks_a_value_built_in_code() {
    let labels = |name: &str| std::collections::BTreeMap::from([(name.to_owned(), "x".to_owned())]);
    assert!(types::Labels::try_from(labels("red")).is_ok());
    assert!(types::Labels::try_from(labels("Red")).is_err(), "a name outside `^[a-z]+$`");
}

/// A builder checks the rules across fields too, not only each field: `build` refuses a value that breaks
/// `dependentRequired` or `not`, although every field converts.
#[test]
fn a_builder_checks_the_rules_across_fields() {
    let parcels =
        || -> types::ShipmentInternationalParcels { serde_json::from_value(json!([{"fragile": true}])).unwrap() };
    types::ShipmentInternational::builder().customs("cn22").parcels(parcels()).build().expect("every rule passes");
    let insured_alone =
        types::ShipmentInternational::builder().customs("cn22").parcels(parcels()).insured(true).build();
    assert!(insured_alone.is_err(), "dependentRequired: insured needs insurer");
    let both = types::ShipmentInternational::builder().customs("none").parcels(parcels()).insurer("acme").build();
    assert!(both.is_err(), "not: customs `none` together with an insurer");
}

/// A bounded integer has a `const fn new` that returns `None` outside its limits: in a constant, `expect` makes a
/// literal that breaks them a build error.
#[test]
fn a_literal_of_a_bounded_integer_is_built_in_a_constant() {
    const AGE: types::UserAge = types::UserAge::new(30).expect("within the limits");
    assert_eq!(AGE.get(), 30);
    assert_eq!(types::UserAge::new(-1), None);
}

/// A `pattern` beside a format is checked on the text: a uuid that does not start with `0` is refused.
#[test]
fn a_pattern_beside_a_format_is_checked() {
    let id = |text: &str| serde_json::from_value::<types::LegacyId>(serde_json::json!(text));
    assert!(id("01234567-89ab-cdef-0123-456789abcdef").is_ok());
    assert!(id("11234567-89ab-cdef-0123-456789abcdef").is_err());
}

/// A `maxLength` on base64 bytes is checked on their text before it decodes.
#[test]
fn a_length_on_base64_bytes_is_checked_on_their_text() {
    let token = |text: &str| serde_json::from_value::<types::Token>(serde_json::json!(text));
    assert!(token("QUJDRA==").is_ok(), "8 characters");
    assert!(token("QUJDREVGR0g=").is_err(), "12 characters");
}

/// A limit beside a type of yours (`x-rust-type: i64`) is checked on the JSON value before it converts.
#[test]
fn a_limit_beside_a_type_of_yours_is_checked() {
    let wallet = |cents: i64| serde_json::from_value::<types::Wallet>(serde_json::json!({"cents": cents}));
    assert!(wallet(100).is_ok());
    assert!(wallet(101).is_err(), "maximum 100");
}

/// An `if` passes when the property that it tests is absent, so its `then` applies: `{}` lacks `x`.
#[test]
fn an_if_on_an_absent_property_applies_its_then() {
    assert!(serde_json::from_value::<types::Kinded>(json!({})).is_err());
    assert!(serde_json::from_value::<types::Kinded>(json!({"kind": "a"})).is_err());
    assert!(serde_json::from_value::<types::Kinded>(json!({"kind": "a", "x": "1"})).is_ok());
    assert!(serde_json::from_value::<types::Kinded>(json!({"kind": "b"})).is_ok());
}
