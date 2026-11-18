//! Hand-written checks for the `formats` fixture.

use assert_matches::assert_matches;

use e2e::v32_formats::api::{self, operations as ops, types};
use serde_json::json;

/// An `id` and an `at` that their formats take. Without the uuid and date libraries they are strings checked as text.
const ID: &str = "01234567-89ab-4def-8123-456789abcdef";
const AT: &str = "2024-01-31T13:45:30Z";

#[test]
fn json_inside_a_string_is_typed() {
    // `payload` carries JSON text inside a string (`contentMediaType`).
    let json = json!({"id": "x", "at": "2024-01-31T00:00:00Z", "payload": json!({"n": 7}).to_string()});
    let e: types::Event = serde_json::from_value(json).expect("decodes");
    assert_eq!(e.payload.as_ref().expect("present").0.n, 7);
    let back = serde_json::to_value(&e).expect("encodes");
    assert_eq!(back["payload"], json!(json!({"n": 7}).to_string()), "sent back as JSON text");
    let wrong = json!({"id": ID, "at": AT, "payload": json!({"n": "seven"}).to_string()});
    assert!(serde_json::from_value::<types::Event>(wrong).is_err());
}

#[test]
fn base64url_uses_the_url_safe_alphabet() {
    let e: types::Event = serde_json::from_value(json!({"id": ID, "at": AT, "token": "-_-_"})).expect("decodes");
    let token = e.token.clone().expect("present");
    assert_eq!(token.0, vec![0xfb, 0xff, 0xbf]);
    assert_eq!(serde_json::to_value(&e).expect("encodes")["token"], serde_json::json!("-_-_"));
    assert!(
        serde_json::from_value::<types::Event>(json!({"id": ID, "at": AT, "token": "+/+/"})).is_err(),
        "standard alphabet"
    );
}

/// Violations of an event as `path message`.
fn violations(port: i64) -> Vec<String> {
    let e: types::Event = serde_json::from_value(json!({"id": ID, "at": AT, "port": port})).expect("decodes");
    let mut out = vec![];
    api::Validate::validate(&e, &api::InstancePath::Root, &mut out);
    out.iter().map(|v| format!("{} {}", v.path, v.message)).collect()
}

#[test]
fn small_integer_formats_are_their_rust_types() {
    let e: types::Event =
        serde_json::from_value(json!({"id": ID, "at": AT, "level": -128, "port": 65_535})).expect("decodes");
    let (level, port): (Option<i8>, Option<u16>) = (e.level, e.port);
    assert_eq!((level, port), (Some(-128), Some(65_535)));
    for out_of_range in [json!({"id": ID, "at": AT, "level": 300}), json!({"id": ID, "at": AT, "port": 70_000})] {
        assert!(serde_json::from_value::<types::Event>(out_of_range).is_err());
    }
    // The explicit `minimum: 1024` still applies inside `uint16`.
    assert!(violations(80).iter().any(|m| m.starts_with("/port ")));
    assert_eq!(violations(1024), Vec::<String>::new());
}

/// `{type: string, format: decimal}` is a `String`. Every digit of `12345678901234567.89` survives. An `f64` would give
/// `12345678901234568`. A value that is not a decimal is refused. `decimal128` is the same.
#[test]
fn a_string_decimal_keeps_its_digits() {
    let wide =
        json!({"id": ID, "at": AT, "price": "12345678901234567.89", "total": "1.000000000000000000000000000000001"});
    let e: types::Event = serde_json::from_value(wide).expect("decodes");
    assert_eq!(e.price.as_deref(), Some("12345678901234567.89"));
    assert_eq!(e.total.as_deref(), Some("1.000000000000000000000000000000001"));
}

/// Without the uuid and date libraries `id` and `at` are strings: their text is checked as the libraries would parse
/// it.
#[test]
fn a_format_without_its_library_is_checked_as_text() {
    let e: types::Event = serde_json::from_value(json!({
        "id": "{01234567-89ab-4def-8123-456789abcdef}", "at": "2024-13-45",
        "expires": "Sun, 31 Nov 1994 08:49:37 GMT", "lasts": "P1Y2D", "link": "a b", "page": "/no-scheme",
        "rel": "#a#b", "at_path": "/~2", "up": "-1/a", "filter": "(", "search": "{x",
    }))
    .expect("decodes");
    let mut out = vec![];
    api::Validate::validate(&e, &api::InstancePath::Root, &mut out);
    let paths: Vec<&str> = out.iter().map(|v| v.path.as_str()).collect();
    assert_eq!(
        paths,
        ["/id", "/at", "/expires", "/lasts", "/link", "/page", "/rel", "/at_path", "/up", "/filter", "/search"]
    );
}

/// `{type: number, format: decimal}` keeps every digit through a JSON body. An `f64` would round
/// `12345678901234567.89` to `12345678901234568`. `minimum: 0` still applies.
#[test]
fn a_decimal_number_keeps_every_digit() {
    let wide = r#"{"id":"x","at":"t","amount":12345678901234567.89,"sum":0.1000000000000000055511151231257827}"#;
    let e: types::Event = serde_json::from_str(wide).expect("decodes");
    assert_eq!(e.amount.as_ref().expect("present").as_str(), "12345678901234567.89");
    let text = serde_json::to_string(&e).expect("encodes");
    assert!(text.contains(r#""amount":12345678901234567.89"#), "{text}");
    assert!(text.contains(r#""sum":0.1000000000000000055511151231257827"#), "{text}");
    let mut out = vec![];
    let negative: types::Event = serde_json::from_str(
        r#"{"id":"01234567-89ab-4def-8123-456789abcdef","at":"2024-06-01T12:30:00Z","amount":-1}"#,
    )
    .expect("decodes");
    api::Validate::validate(&negative, &api::InstancePath::Root, &mut out);
    assert_eq!(out.len(), 1, "`minimum: 0` still applies: {out:?}");
}

/// `char`, `uri` (with `url`) and the CIDR formats (with `ipnet`) are typed. Without chrono or jiff the local date
/// and time formats stay strings.
#[test]
fn registry_formats_take_their_library_types() {
    let wire = json!({"id": "x", "at": "t", "initial": "Z", "home": "https://example.com/a?b=c",
        "net4": "192.0.2.0/24", "net6": "2001:db8::/32", "local": "2024-01-31T13:45:30", "clock": "13:45:30"});
    let e: types::Event = serde_json::from_value(wire.clone()).expect("decodes");
    let initial: char = e.initial.expect("present");
    let home: url::Url = e.home.clone().expect("present");
    let net4: ipnet::Ipv4Net = e.net4.expect("present");
    let net6: ipnet::Ipv6Net = e.net6.expect("present");
    let local: String = e.local.clone().expect("present");
    assert_eq!((initial, home.host_str(), net4.prefix_len(), net6.prefix_len()), ('Z', Some("example.com"), 24, 32));
    assert_eq!(local, "2024-01-31T13:45:30");
    let back = serde_json::to_value(&e).expect("encodes");
    for k in ["initial", "home", "net4", "net6", "local", "clock"] {
        assert_eq!(back[k], wire[k], "{k} goes back as it came");
    }
    assert!(serde_json::from_value::<types::Event>(json!({"id": "x", "at": "t", "net4": "300.0.0.0/8"})).is_err());
}

/// A decimal inside a `oneOf` branch keeps its digits. The union decodes from the JSON text and not from a buffer
/// where numbers are `f64`.
#[test]
fn a_decimal_in_a_one_of_branch_keeps_its_digits() {
    let body = format!(r#"{{"id": "{ID}", "at": "{AT}", "charge": {{"fee": 12345678901234567.89}}}}"#);
    let e: types::Event = serde_json::from_str(&body).expect("decodes");
    let Some(types::EventCharge::Fee(charge)) = e.charge else { panic!("the fee branch: {:?}", e.charge) };
    assert_eq!(charge.fee.as_str(), "12345678901234567.89");
}

/// A decimal in a branch of a union told apart by a discriminator keeps its digits. This holds whether the
/// discriminator comes first or last. The tag is read from the JSON text. Then the one branch is parsed from it.
#[test]
fn a_decimal_in_a_tagged_branch_keeps_its_digits_wherever_the_tag_sits() {
    for payment in [
        r#"{"method": "Card", "amount": 12345678901234567.89}"#,
        r#"{"amount": 12345678901234567.89, "method": "Card"}"#,
    ] {
        let body = format!(r#"{{"id": "{ID}", "at": "{AT}", "payment": {payment}}}"#);
        let e: types::Event = serde_json::from_str(&body).expect("decodes");
        let Some(types::EventPayment::Card(card)) = e.payment else { panic!("the card branch: {:?}", e.payment) };
        assert_eq!(card.amount.as_str(), "12345678901234567.89", "{payment}");
    }
}

/// An event with a value in every format the fixture has.
fn full_event() -> types::Event {
    serde_json::from_value(json!({
        "id": ID, "at": AT, "ip4": "192.0.2.1", "ip6": "2001:db8::1", "day": "2024-01-31",
        "seen": [AT], "token": "-_8", "level": -3, "port": 8080,
        "amount": 12345678901234567.89, "sum": "0.10", "initial": "é",
        "home": "https://example.com/a", "net4": "192.0.2.0/24", "net6": "2001:db8::/32",
        "payment": {"method": "Card", "amount": 1.5},
    }))
    .expect("decodes")
}

/// Keeps the params the server decoded and answers with [`full_event`].
#[derive(Default)]
struct Events(std::sync::Mutex<Option<ops::get_event::Params>>);

impl api::Api for Events {
    type Principal = ();

    async fn get_event(
        &self,
        p: ops::get_event::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::get_event::Response, api::ApiError> {
        *self.0.lock().expect("lock") = Some(p);
        Ok(ops::get_event::Response::Ok(full_event()))
    }
}

/// Formats in the path, the query and the body go from client to server and back unchanged. A decimal keeps its
/// digits in the URL as in JSON.
#[test]
fn every_format_round_trips_through_client_and_server() {
    let events = std::sync::Arc::new(Events::default());
    let service = api::Server::new(std::sync::Arc::clone(&events)).into_service();
    let client = api::Client::new(service);
    let above: api::Decimal = "12345678901234567.89".parse().expect("a decimal");
    let sent = ops::get_event::Params {
        on: Some("2024-01-31".to_owned()),
        above: Some(above),
        ..ops::get_event::Params::new(ID.to_owned())
    };
    let got = api::block_on(client.get_event(sent.clone()));
    assert_eq!(got.ok(), Some(ops::get_event::Response::Ok(full_event())));
    assert_eq!(events.0.lock().expect("lock").take(), Some(sent));
}

/// The discriminator picks the branch by its tag although both branches have one type. A tag that no branch has is
/// `Other`.
#[test]
fn the_tag_picks_the_branch_and_an_unknown_tag_is_other() {
    let payment = |p: serde_json::Value| -> Option<types::EventPayment> {
        serde_json::from_value::<types::Event>(json!({"id": ID, "at": AT, "payment": p})).expect("decodes").payment
    };
    assert_matches!(payment(json!({"method": "Transfer", "amount": 2})), Some(types::EventPayment::Transfer(_)));
    assert_matches!(payment(json!({"method": "Card", "amount": 2})), Some(types::EventPayment::Card(_)));
    assert_matches!(payment(json!({"method": "Cash", "amount": 2})), Some(types::EventPayment::Other(_)));
}

/// The `minimum` of a decimal is checked inside the branch that the tag picked. A value that no branch takes is
/// reported.
#[test]
fn a_union_checks_its_branch_and_reports_a_value_no_branch_takes() {
    let check = |extra: serde_json::Value| -> Vec<String> {
        let mut event = json!({"id": ID, "at": AT});
        event.as_object_mut().expect("an object").extend(extra.as_object().expect("an object").clone());
        let e: types::Event = serde_json::from_value(event).expect("decodes");
        let mut out = vec![];
        api::Validate::validate(&e, &api::InstancePath::Root, &mut out);
        out.iter().map(|v| v.path.clone()).collect()
    };
    assert_eq!(check(json!({"payment": {"method": "Transfer", "amount": 0}})), Vec::<String>::new());
    assert_eq!(check(json!({"payment": {"method": "Transfer", "amount": -0.01}})), ["/payment/amount"]);
    assert_eq!(check(json!({"payment": {"method": "Cash", "amount": 1}})), ["/payment"]);
    assert_eq!(check(json!({"charge": {"refund": 1}})), Vec::<String>::new());
    assert_eq!(check(json!({"charge": {"neither": 1}})), ["/charge"]);
}

/// `decimal128`, as a number and as a string, holds what IEEE 754 decimal128 holds: at most 34 significant digits
/// within its exponent range.
#[test]
fn decimal128_holds_what_ieee_decimal128_holds() {
    let violations = |extra: &str| {
        let e: types::Event = serde_json::from_str(&format!(r#"{{"id": "x", "at": "t", {extra}}}"#)).expect("decodes");
        let mut out = vec![];
        api::Validate::validate(&e, &api::InstancePath::Root, &mut out);
        out.into_iter().filter(|v| v.path == "/sum" || v.path == "/total").map(|v| v.message).collect::<Vec<_>>()
    };
    let digits34 = "1234567890123456789012345678901234";
    assert_eq!(violations(&format!(r#""sum": {digits34}e-10, "total": "{digits34}e-10""#)), Vec::<String>::new());
    let digits35 = format!("{digits34}5");
    assert_eq!(
        violations(&format!(r#""sum": {digits35}"#)),
        [format!("`{digits35}` has 35 significant digits, more than the 34 of decimal128")]
    );
    assert_eq!(violations(&format!(r#""total": "{digits35}""#)).len(), 1, "the string too");
    assert_eq!(violations(r#""sum": 1e6145"#), ["`1e6145` is out of the range of decimal128"]);
}
