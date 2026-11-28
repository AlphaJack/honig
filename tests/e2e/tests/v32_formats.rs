//! Hand-written checks for the `formats` fixture.

use assert_matches::assert_matches;

use e2e::v32_formats::api::{self, operations as ops, types};
use serde_json::json;

/// An `id` and an `at` that their formats accept. Without the uuid and date libraries they are strings checked as text.
const ID: &str = "01234567-89ab-4def-8123-456789abcdef";
const AT: &str = "2024-01-31T13:45:30Z";

#[test]
fn json_inside_a_string_is_typed() {
    // `payload` carries JSON text inside a string (`contentMediaType`).
    let json = json!({"id": ID, "at": AT, "payload": json!({"n": 7}).to_string()});
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

/// The event with a `port`, or the error that decoding it gives.
fn with_port(port: i64) -> Result<types::Event, String> {
    serde_json::from_value(json!({"id": ID, "at": AT, "port": port})).map_err(|e| e.to_string())
}

#[test]
fn small_integer_formats_are_their_rust_types() {
    let e: types::Event =
        serde_json::from_value(json!({"id": ID, "at": AT, "level": -128, "port": 65_535})).expect("decodes");
    let (level, port): (Option<i8>, Option<u16>) = (e.level, e.port.map(types::EventPort::get));
    assert_eq!((level, port), (Some(-128), Some(65_535)));
    for out_of_range in [json!({"id": ID, "at": AT, "level": 300}), json!({"id": ID, "at": AT, "port": 70_000})] {
        assert!(serde_json::from_value::<types::Event>(out_of_range).is_err());
    }
    // The explicit `minimum: 1024` still applies inside `uint16`.
    assert_eq!(with_port(80).err(), Some("80 is less than the minimum 1024".to_owned()));
    assert!(with_port(1024).is_ok());
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

/// Without the uuid and date libraries `id` and `at` are checked text: only text of their format decodes, as the
/// libraries would parse it. So does every format that no library types.
#[test]
fn a_format_without_its_library_is_checked_as_text() {
    let e: types::Event = serde_json::from_value(json!({"id": ID, "at": AT})).expect("decodes");
    assert_eq!((e.id.as_str(), e.at.as_str()), (ID, AT));
    for (field, bad) in [
        ("id", "{01234567-89ab-4def-8123-456789abcdef}"),
        ("at", "2024-13-45"),
        ("expires", "Sun, 31 Nov 1994 08:49:37 GMT"),
        ("lasts", "P1Y2D"),
        ("link", "a b"),
        ("page", "/no-scheme"),
        ("rel", "#a#b"),
        ("at_path", "/~2"),
        ("up", "-1/a"),
        ("filter", "("),
        ("search", "{x"),
    ] {
        let mut event = json!({"id": ID, "at": AT});
        event[field] = json!(bad);
        let err = serde_json::from_value::<types::Event>(event).expect_err(field).to_string();
        assert!(err.starts_with(&format!("`{bad}` is not a valid ")), "{field}: {err}");
    }
}

/// `{type: number, format: decimal}` keeps every digit through a JSON body. An `f64` would round
/// `12345678901234567.89` to `12345678901234568`. `minimum: 0` still applies.
#[test]
fn a_decimal_number_keeps_every_digit() {
    let wide = r#"{"id":"01234567-89ab-4def-8123-456789abcdef","at":"2024-01-31T13:45:30Z","amount":12345678901234567.89,"sum":0.1000000000000000055511151231257827}"#;
    let e: types::Event = serde_json::from_str(wide).expect("decodes");
    assert_eq!(e.amount.as_ref().expect("present").as_str(), "12345678901234567.89");
    let text = serde_json::to_string(&e).expect("encodes");
    assert!(text.contains(r#""amount":12345678901234567.89"#), "{text}");
    assert!(text.contains(r#""sum":0.1000000000000000055511151231257827"#), "{text}");
    let negative = serde_json::from_str::<types::Event>(
        r#"{"id":"01234567-89ab-4def-8123-456789abcdef","at":"2024-06-01T12:30:00Z","amount":-1}"#,
    )
    .expect_err("`minimum: 0` still applies");
    assert!(negative.to_string().starts_with("-1 is less than the minimum 0"), "{negative}");
}

/// `char`, `uri` (with `url`) and the CIDR formats (with `ipnet`) are typed. Without jiff the local date and time
/// formats are checked text.
#[test]
fn registry_formats_take_their_library_types() {
    let wire = json!({"id": ID, "at": AT, "initial": "Z", "home": "https://example.com/a?b=c",
        "net4": "192.0.2.0/24", "net6": "2001:db8::/32", "local": "2024-01-31T13:45:30", "clock": "13:45:30"});
    let e: types::Event = serde_json::from_value(wire.clone()).expect("decodes");
    let initial: char = e.initial.expect("present");
    let home: honig::url::Url = e.home.clone().expect("present");
    let net4: honig::ipnet::Ipv4Net = e.net4.expect("present");
    let net6: honig::ipnet::Ipv6Net = e.net6.expect("present");
    let local: api::DateTimeLocalStr = e.local.clone().expect("present");
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
    let Some(types::EventCharge::Fee(charge)) = e.charge.as_ref() else { panic!("the fee branch: {:?}", e.charge) };
    assert_eq!(charge.fee.as_str(), "12345678901234567.89");
}

/// A decimal in a branch of a union told apart by a discriminator keeps its digits. This applies whether the
/// discriminator comes first or last. The tag is parsed from the JSON text. Then the one branch is parsed from it.
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
        on: Some("2024-01-31".parse().expect("a date")),
        above: Some(above),
        below: Some("1234567890123456789012345678901234".parse().expect("34 digits")),
        ..ops::get_event::Params::new(ID.parse().expect("a uuid"))
    };
    let got = honig::block_on(client.get_event(sent.clone()));
    assert_eq!(got.ok(), Some(ops::get_event::Response::Ok(full_event())));
    assert_eq!(events.0.lock().expect("lock").take(), Some(sent));
}

/// The discriminator picks the branch by its tag although both branches have one type. A tag that no branch has does
/// not decode.
#[test]
fn the_tag_picks_the_branch_and_an_unknown_tag_does_not_decode() {
    let payment = |p: serde_json::Value| {
        serde_json::from_value::<types::Event>(json!({"id": ID, "at": AT, "payment": p})).map(|e| e.payment)
    };
    assert_matches!(payment(json!({"method": "Transfer", "amount": 2})), Ok(Some(types::EventPayment::Transfer(_))));
    assert_matches!(payment(json!({"method": "Card", "amount": 2})), Ok(Some(types::EventPayment::Card(_))));
    assert_matches!(payment(json!({"method": "Cash", "amount": 2})), Err(_));
}

/// The `minimum` of a decimal is checked inside the branch that the tag picked. A value that no branch accepts does not
/// decode.
#[test]
fn a_union_checks_its_branch_and_refuses_a_value_no_branch_takes() {
    let event = |extra: serde_json::Value| {
        let mut event = json!({"id": ID, "at": AT});
        event.as_object_mut().expect("an object").extend(extra.as_object().expect("an object").clone());
        serde_json::from_value::<types::Event>(event)
    };
    assert!(event(json!({"payment": {"method": "Transfer", "amount": 0}})).is_ok());
    let below = event(json!({"payment": {"method": "Transfer", "amount": -0.01}})).expect_err("below the minimum");
    assert!(below.to_string().starts_with("-0.01 is less than the minimum 0"), "{below}");
    assert!(event(json!({"payment": {"method": "Cash", "amount": 1}})).is_err());
    assert!(event(json!({"charge": {"refund": 1}})).is_ok());
    assert!(event(json!({"charge": {"neither": 1}})).is_err());
}

/// Every decimal128 number is one type, whatever schema it is in: `Event.sum` goes into `Invoice.total` as it is. It
/// refuses 39 significant digits however it is built.
#[test]
fn every_decimal128_number_is_one_type() {
    let digits34: api::Decimal128 = "1234567890123456789012345678901234".parse().expect("34 digits fit");
    let e: types::Event =
        serde_json::from_str(&format!(r#"{{"id": "{ID}", "at": "{AT}", "sum": {digits34}}}"#)).unwrap();
    let invoice = types::Invoice { total: e.sum, floor: None };
    assert_eq!(invoice.total, Some(digits34));
    let digits39 = "123456789012345678901234567890123456789";
    assert!(digits39.parse::<api::Decimal128>().is_err());
    assert!(api::Decimal128::try_from(digits39.parse::<api::Decimal>().unwrap()).is_err());
    let decoded = serde_json::from_str::<types::Invoice>(&format!(r#"{{"total": {digits39}}}"#));
    assert!(decoded.is_err(), "{decoded:?}");
}

/// A `minimum` beside `decimal128` is checked on top of the limits of decimal128.
#[test]
fn a_bounded_decimal128_checks_its_bound_and_its_limits() {
    let floor = |n: &str| serde_json::from_str::<types::Invoice>(&format!(r#"{{"floor": {n}}}"#)).map(|i| i.floor);
    let five = floor("5").expect("in range").expect("present");
    assert_eq!(api::Decimal128::from(five), api::Decimal128::from(5_i64));
    assert!(floor("-1").is_err(), "below the minimum");
    assert!(floor("123456789012345678901234567890123456789").is_err(), "39 digits");
}

/// `decimal128`, as a number and as a string, represents what IEEE 754 decimal128 represents: at most 34 significant
/// digits within its exponent range. Past them a value does not decode.
#[test]
fn decimal128_holds_what_ieee_decimal128_holds() {
    let event = |extra: &str| {
        serde_json::from_str::<types::Event>(&format!(r#"{{"id": "{ID}", "at": "{AT}", {extra}}}"#))
            .map_err(|e| e.to_string())
    };
    let digits34 = "1234567890123456789012345678901234";
    let e = event(&format!(r#""sum": {digits34}e-10, "total": "{digits34}e-10""#)).expect("34 digits fit");
    assert_eq!(e.sum.as_deref().map(api::Decimal::as_str), Some(format!("{digits34}e-10").as_str()));
    assert_eq!(e.total.as_deref(), Some(format!("{digits34}e-10").as_str()));
    let digits35 = format!("{digits34}5");
    let sum = event(&format!(r#""sum": {digits35}"#)).expect_err("35 digits");
    assert!(sum.contains("has 35 significant digits, more than the 34 of decimal128"), "{sum}");
    let total = event(&format!(r#""total": "{digits35}""#)).expect_err("35 digits");
    assert!(total.starts_with(&format!("`{digits35}` is not a valid decimal128")), "{total}");
    assert!(event(r#""sum": 1e6145"#).is_err(), "past the exponent range");
}
