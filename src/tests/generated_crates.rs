//! A browser client generated from a fixture and compiled for `wasm32`. `tests/regen.rs` keeps `tests/e2e` and
//! `examples/shop` fresh.

use std::path::Path;
use std::process::Command;

/// A browser client: client-only module with the `gloo-net` transport. It must
/// compile for `wasm32-unknown-unknown` (where futures are `!Send`). It must still
/// compile natively where the adapter is left out. It needs the wasm32 target:
/// `rustup target add wasm32-unknown-unknown`.
#[test]
#[ignore = "compiles generated crates"]
fn wasm_client() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = root.join("target").join("gen-tests").join("wasm-client");
    // A missing directory is fine: there is nothing to clean up.
    std::fs::remove_dir_all(&out).ok();
    let manifest = r#"[package]
name = "wasm-client"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[target.'cfg(target_arch = "wasm32")'.dependencies]
gloo-net = { version = "0.6", default-features = false, features = ["http"] }
"#;
    let opts = crate::emit::Options {
        server: false,
        tests: false,
        rust: crate::emit::rust::Options::default(),
        ..crate::emit::Options::default()
    };
    // Streams, OAuth flows and webhooks are the client paths that hold futures.
    let spec = root.join("tests").join("fixtures").join("v32").join("shop.yaml");
    let generated = crate::generate::generate(&spec, &opts).unwrap_or_else(|d| panic!("generation failed: {:?}", d.0));
    crate::disk::write_module(
        &out.join("src").join("api"),
        &out.join("src").join("api").join("mod.rs"),
        &generated.files,
    )
    .unwrap();
    std::fs::write(
        out.join("Cargo.toml"),
        crate::manifest::with_runtime(manifest, &[&generated.runtime_features[..], &["gloo-net"]].concat()).unwrap(),
    )
    .unwrap();
    super::common::use_local_runtime(&out.join("Cargo.toml"));
    // `deny(warnings)`: a stray `+ Send` that refines the trait shows up as a lint.
    std::fs::write(
        out.join("src").join("lib.rs"),
        "#![deny(warnings)]\npub mod api;\n#[cfg(target_arch = \"wasm32\")]\npub fn client() -> api::Client<api::runtime::GlooTransport> {\n    api::Client::new(api::runtime::GlooTransport::new(\"/api\"))\n}\n",
    )
    .unwrap();
    for target in [Some("wasm32-unknown-unknown"), None] {
        let mut cmd = Command::new(env!("CARGO"));
        cmd.args(["check", "--quiet"])
            .current_dir(&out)
            .env("CARGO_TARGET_DIR", root.join("target").join("gen-tests").join("target"));
        if let Some(t) = target {
            cmd.args(["--target", t]);
        }
        let status = cmd.status().expect("cargo runs");
        assert!(status.success(), "wasm client does not compile for {}", target.unwrap_or("the host"));
    }
}

/// The date and time formats with `jiff`: a `time` keeps its offset and its leap second in `runtime::FullTime`.
#[test]
#[ignore = "compiles generated crates"]
fn jiff_dates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = root.join("target").join("gen-tests").join("jiff-dates");
    // A missing directory is fine: there is nothing to clean up.
    std::fs::remove_dir_all(&out).ok();
    let manifest = r#"[package]
name = "jiff-dates"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
jiff = { version = "1", features = ["serde"] }
"#;
    let doc = super::common::spec(
        "3.2.0",
        &serde_json::json!({}),
        &serde_json::json!({"schemas": {"Shift": {
            "type": "object",
            "required": ["day", "opens", "starts", "local", "ends"],
            "properties": {
                "day": {"type": "string", "format": "date"},
                "opens": {"type": "string", "format": "time"},
                "starts": {"type": "string", "format": "date-time"},
                "local": {"type": "string", "format": "date-time-local"},
                "ends": {"type": "string", "format": "time-local"},
            },
        }}}),
    );
    let libs = crate::settings::FormatLibs { dates: Some(crate::settings::DateLib::Jiff), ..Default::default() };
    let opts = crate::emit::Options {
        client: false,
        server: false,
        tests: false,
        rust: crate::emit::rust::Options { libs, ..Default::default() },
        ..crate::emit::Options::default()
    };
    let spec = super::common::write("jiff-dates", &doc, &[]);
    let generated = crate::generate::generate(&spec, &opts).unwrap_or_else(|d| panic!("generation failed: {:?}", d.0));
    assert!(generated.runtime_features.contains(&"jiff"), "{:?}", generated.runtime_features);
    let api = out.join("src").join("api");
    crate::disk::write_module(&api, &api.join("mod.rs"), &generated.files).unwrap();
    std::fs::write(
        out.join("Cargo.toml"),
        crate::manifest::with_runtime(manifest, &generated.runtime_features).unwrap(),
    )
    .unwrap();
    super::common::use_local_runtime(&out.join("Cargo.toml"));
    std::fs::write(
        out.join("src").join("lib.rs"),
        r##"#![deny(warnings)]
pub mod api;

#[test]
fn a_shift_keeps_every_offset_as_sent() {
    let text = r#"{"day":"2024-02-29","opens":"23:59:60Z","starts":"2024-02-29T08:30:06+02:00","local":"2024-02-29T08:30:06","ends":"17:00:00"}"#;
    let shift: api::types::Shift = serde_json::from_str(text).unwrap();
    assert!(shift.opens.leap_second);
    assert_eq!(shift.starts, "2024-02-29T06:30:06Z".parse::<jiff::Timestamp>().unwrap());
    assert_eq!(serde_json::to_value(&shift).unwrap()["opens"], "23:59:60Z");
    assert!(serde_json::from_str::<api::types::Shift>(&text.replace("23:59:60Z", "10:00:00")).is_err());
}
"##,
    )
    .unwrap();
    let status = Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", root.join("target").join("gen-tests").join("target"))
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the jiff crate does not build or its test fails");
}

/// A library type reads more than its format allows: `jiff` and `chrono` read `2024-06-19 08:30:06Z`, `uuid` reads
/// `{...}`, `url` reads a space and `ipnet` reads `01.2.3.4/8`. Generated code checks the text first, in every place a
/// field can hold it.
#[test]
#[ignore = "compiles generated crates"]
fn library_types_read_only_their_format() {
    let types = Crate { paths: serde_json::json!({}), schemas: validated_schemas(), test: VALIDATED_TEST, http: false };
    let all = crate::settings::FormatLibs { uuid: true, url: true, ipnet: true, dates: None, ..Default::default() };
    let jiff = "jiff = { version = \"1\", features = [\"serde\"] }";
    let with_jiff = crate::settings::FormatLibs { dates: Some(crate::settings::DateLib::Jiff), ..all.clone() };
    build_validated("validated-jiff", jiff, with_jiff, &types);
    let chrono = "chrono = { version = \"0.4\", features = [\"serde\"] }";
    let with_chrono = crate::settings::FormatLibs { dates: Some(crate::settings::DateLib::Chrono), ..all };
    build_validated("validated-chrono", chrono, with_chrono, &types);
}

/// A struct with a checked format in every place a field can hold one: itself, `Option`, `runtime::Nullable`, a
/// list, a tuple, an array, an alias, the undeclared properties, a union branch and a newtype.
fn validated_schemas() -> serde_json::Value {
    serde_json::json!({
        "Stamp": {"type": "string", "format": "date-time"},
        "ByKind": {"oneOf": [{"type": "string", "format": "date-time"}, {"type": "integer"}]},
        "SameKind": {"oneOf": [{"type": "string", "format": "date-time"}, {"type": "string", "format": "uuid"}]},
        "Rec": {
            "type": "array",
            "prefixItems": [
                {"type": "string", "format": "date"},
                {"oneOf": [{"$ref": "#/components/schemas/Rec"}, {"type": "null"}]},
            ],
            "items": false,
            "minItems": 2,
        },
        "Validated": {
            "type": "object",
            "required": ["at", "id", "maybe_at"],
            "properties": {
                "at": {"type": "string", "format": "date-time"},
                "id": {"type": "string", "format": "uuid"},
                "maybe_at": {"type": ["string", "null"], "format": "date-time"},
                "home": {"type": "string", "format": "uri"},
                "net": {"type": "string", "format": "ipv4-cidr"},
                "day": {"type": ["string", "null"], "format": "date"},
                "clock": {"type": "string", "format": "time-local"},
                "local": {"type": "string", "format": "date-time-local"},
                "seen": {"type": "array", "items": {"type": "string", "format": "date-time"}},
                "pair": {
                    "type": "array",
                    "prefixItems": [{"type": "string", "format": "date"}, {"type": "integer"}],
                    "items": false,
                    "minItems": 2,
                },
                "two": {"type": "array", "items": {"type": "string", "format": "date"}, "minItems": 2, "maxItems": 2},
                "stamp": {"$ref": "#/components/schemas/Stamp"},
                "by_kind": {"$ref": "#/components/schemas/ByKind"},
                "same_kind": {"$ref": "#/components/schemas/SameKind"},
                "rec": {"$ref": "#/components/schemas/Rec"},
            },
            "additionalProperties": {"type": "string", "format": "date"},
        },
    })
}

/// The test of the generated crate: each place refuses a text that its library alone would read.
const VALIDATED_TEST: &str = r##"#![deny(warnings)]
pub mod api;

#[cfg(test)]
mod tests {
use super::api;

const OK: &str = r#"{"at":"2024-06-19T08:30:06Z","id":"01234567-89ab-4def-8123-456789abcdef","maybe_at":null,
"home":"https://example.com/a","net":"192.168.0.0/24","day":"2024-06-19","clock":"08:30:06",
"local":"2024-06-19T08:30:06","seen":["2024-06-19T08:30:06Z"],"pair":["2024-06-19",1],
"two":["2024-06-19","2024-06-20"],"stamp":"2024-06-19T08:30:06Z","extra":"2024-06-19",
"by_kind":"2024-06-19T08:30:06Z","same_kind":"01234567-89ab-4def-8123-456789abcdef","rec":["2024-06-19",null]}"#;

/// Decodes `json` as a server decodes a request and a client a response: the text of library types is validated.
fn decodes(json: &str) -> bool {
    serde_json::from_str::<api::types::Validated>(json).is_ok()
}

/// jiff reads a space in a `date-time`. Decoding refuses it before jiff reads it, on either side.
#[test]
fn text_that_the_library_reads_but_the_format_refuses_does_not_decode() {
    let spaced = OK.replace("\"at\":\"2024-06-19T08:30:06Z\"", "\"at\":\"2024-06-19 08:30:06Z\"");
    assert!(serde_json::from_str::<api::types::Validated>(&spaced).is_err());
}

#[test]
fn every_place_checks_its_text() {
    assert!(decodes(OK), "{:?}", serde_json::from_str::<api::types::Validated>(OK).err());
    assert!(decodes(&OK.replace("08:30:06Z\",\"id", "08:30:06.1234567891Z\",\"id")), "more than 9 fraction digits");
    let mut decoded = vec![];
    for (good, bad) in [
        ("\"at\":\"2024-06-19T08:30:06Z\"", "\"at\":\"2024-06-19 08:30:06Z\""),
        ("\"maybe_at\":null", "\"maybe_at\":\"2024-06-19T08:30:06+0100\""),
        ("\"id\":\"01234567-89ab-4def-8123-456789abcdef\"", "\"id\":\"{01234567-89ab-4def-8123-456789abcdef}\""),
        ("\"home\":\"https://example.com/a\"", "\"home\":\"https://example.com/a b\""),
        ("\"net\":\"192.168.0.0/24\"", "\"net\":\"192.168.000.0/24\""),
        ("\"day\":\"2024-06-19\"", "\"day\":\"+2024-06-19\""),
        ("\"clock\":\"08:30:06\"", "\"clock\":\"08:30\""),
        ("\"local\":\"2024-06-19T08:30:06\"", "\"local\":\"2024-06-19 08:30:06\""),
        ("\"seen\":[\"2024-06-19T08:30:06Z\"]", "\"seen\":[\"2024-06-19T08:30Z\"]"),
        ("\"pair\":[\"2024-06-19\",1]", "\"pair\":[\"2024-6-9\",1]"),
        ("\"two\":[\"2024-06-19\",\"2024-06-20\"]", "\"two\":[\"2024-06-19\",\"20240620\"]"),
        ("\"stamp\":\"2024-06-19T08:30:06Z\"", "\"stamp\":\"2024-06-19T08:30:06+01\""),
        ("\"extra\":\"2024-06-19\"", "\"extra\":\"2024-06-19T00:00:00\""),
        ("\"by_kind\":\"2024-06-19T08:30:06Z\"", "\"by_kind\":\"2024-06-19 08:30:06Z\""),
        ("\"rec\":[\"2024-06-19\",null]", "\"rec\":[\"20240619\",null]"),
    ] {
        assert!(OK.contains(good), "{good}");
        if decodes(&OK.replace(good, bad)) {
            decoded.push(bad);
        }
    }
    assert!(decoded.is_empty(), "decoded: {decoded:?}");
}

/// A branch of an untagged union takes only its own format. Text that no branch takes does not decode.
#[test]
fn an_untagged_branch_checks_its_text() {
    use api::types::SameKind;
    let pick = |s: &str| serde_json::from_str::<SameKind>(s);
    assert!(matches!(pick("\"2024-06-19T08:30:06Z\""), Ok(SameKind::DateTime(_))));
    assert!(matches!(pick("\"01234567-89ab-4def-8123-456789abcdef\""), Ok(SameKind::Uuid(_))));
    assert!(pick("\"2024-06-19 08:30:06Z\"").is_err());
    assert!(pick("\"{01234567-89ab-4def-8123-456789abcdef}\"").is_err());
}
}
"##;

/// The description and the test of a generated crate. `http` generates its client and server too.
struct Crate<'a> {
    paths: serde_json::Value,
    schemas: serde_json::Value,
    test: &'a str,
    http: bool,
}

/// Generates `krate` with `libs` into a crate named `name` that depends on `lib` too, and runs its test.
fn build_validated(name: &str, lib: &str, libs: crate::settings::FormatLibs, krate: &Crate<'_>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out = root.join("target").join("gen-tests").join(name);
    // A missing directory is fine: there is nothing to clean up.
    std::fs::remove_dir_all(&out).ok();
    let manifest = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n[dependencies]\n\
         serde = {{ version = \"1\", features = [\"derive\"] }}\nserde_json = \"1\"\n{lib}\n\
         uuid = {{ version = \"1\", features = [\"serde\"] }}\nurl = {{ version = \"2\", features = [\"serde\"] }}\n\
         ipnet = {{ version = \"2\", features = [\"serde\"] }}\n"
    );
    let opts = crate::emit::Options {
        client: krate.http,
        server: krate.http,
        tests: false,
        rust: crate::emit::rust::Options { libs, ..Default::default() },
        ..crate::emit::Options::default()
    };
    let doc = super::common::spec("3.2.0", &krate.paths, &serde_json::json!({"schemas": krate.schemas}));
    let spec = super::common::write(name, &doc, &[]);
    let generated = crate::generate::generate(&spec, &opts).unwrap_or_else(|d| panic!("generation failed: {:?}", d.0));
    let api = out.join("src").join("api");
    crate::disk::write_module(&api, &api.join("mod.rs"), &generated.files).unwrap();
    let manifest = crate::manifest::with_runtime(&manifest, &generated.runtime_features).unwrap();
    std::fs::write(out.join("Cargo.toml"), manifest).unwrap();
    super::common::use_local_runtime(&out.join("Cargo.toml"));
    std::fs::write(out.join("src").join("lib.rs"), krate.test).unwrap();
    let status = Command::new(env!("CARGO"))
        .args(["test", "--quiet"])
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", root.join("target").join("gen-tests").join("target"))
        .status()
        .expect("cargo runs");
    assert!(status.success(), "{name} does not build or its test fails");
}

/// Operations whose parameters, bodies and response headers are library types.
fn validated_paths() -> serde_json::Value {
    serde_json::json!({
        "/items/{id}": {"get": {
            "operationId": "getItem",
            "parameters": [
                {"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}},
                {"name": "since", "in": "query", "schema": {"type": "string", "format": "date-time"}},
                {"name": "X-Day", "in": "header", "schema": {"type": "string", "format": "date"}},
                {"name": "days", "in": "query", "schema": {"type": "array", "items": {"type": "string", "format": "date"}}},
            ],
            "responses": {"200": {
                "description": "The time of the item.",
                "headers": {"Location": {"schema": {"type": "string", "format": "uri"}}},
                "content": {"application/json": {"schema": {"type": "string", "format": "date-time"}}},
            }},
        }},
        "/stamps": {"post": {
            "operationId": "postStamp",
            "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "string", "format": "date-time"}}}},
            "responses": {"204": {"description": "Kept."}},
        }},
        "/days": {"post": {
            "operationId": "postDays",
            "requestBody": {"required": true, "content": {"application/jsonl": {"itemSchema": {"type": "string", "format": "date"}}}},
            "responses": {"204": {"description": "Kept."}, "422": {"description": "An item is not a date."}},
        }},
        "/log": {"get": {
            "operationId": "getLog",
            "responses": {"200": {
                "description": "One day per line.",
                "content": {"application/jsonl": {"itemSchema": {"type": "string", "format": "date"}}},
            }},
        }},
    })
}

/// Parameters and bodies of requests that are library types have their text validated. Responses are left to the library.
#[test]
#[ignore = "compiles generated crates"]
fn library_types_in_requests_read_only_their_format() {
    let http =
        Crate { paths: validated_paths(), schemas: serde_json::json!({}), test: VALIDATED_HTTP_TEST, http: true };
    let jiff = "jiff = { version = \"1\", features = [\"serde\"] }";
    let libs = crate::settings::FormatLibs {
        uuid: true,
        url: true,
        ipnet: true,
        dates: Some(crate::settings::DateLib::Jiff),
        ..Default::default()
    };
    build_validated("validated-http", jiff, libs, &http);
}

/// The test of the generated client and server: a request with text that only the library reads fails. A response with it is read.
const VALIDATED_HTTP_TEST: &str = r#"#![deny(warnings)]
pub mod api;

#[cfg(test)]
mod tests {
use super::api::{self, operations as ops};

const ID: &str = "01234567-89ab-4def-8123-456789abcdef";

struct Items;

impl api::Api for Items {
    type Principal = ();

    async fn get_item(&self, _: ops::get_item::Params, _: &api::Context<'_>) -> Result<ops::get_item::Response, api::ApiError> {
        Ok(ops::get_item::Response::Ok { body: "2024-06-19T08:30:06Z".parse().unwrap(), location: None })
    }

    async fn post_stamp(&self, _: ops::post_stamp::Params, _: &api::Context<'_>) -> Result<ops::post_stamp::Response, api::ApiError> {
        Ok(ops::post_stamp::Response::NoContent)
    }

    async fn post_days(&self, p: ops::post_days::Params, _: &api::Context<'_>) -> Result<ops::post_days::Response, api::ApiError> {
        Ok(match p.body.collect().await {
            Ok(_) => ops::post_days::Response::NoContent,
            Err(_) => ops::post_days::Response::UnprocessableEntity,
        })
    }

    async fn get_log(&self, _: ops::get_log::Params, _: &api::Context<'_>) -> Result<ops::get_log::Response, api::ApiError> {
        Ok(ops::get_log::Response::Ok(api::Items::List(vec![])))
    }
}

/// The status the server answers.
fn status(method: &str, uri: &str, header: Option<(&str, &str)>, body: &str) -> u16 {
    use api::Service;
    let mut service = api::Server::new(Items).into_service();
    let ct = if uri == "/days" { "application/jsonl" } else { "application/json" };
    let mut req = api::http::Request::builder().method(method).uri(uri).header("content-type", ct);
    if let Some((name, value)) = header {
        req = req.header(name, value);
    }
    let req = req.body(body.to_owned()).unwrap();
    api::block_on(service.call(req)).unwrap().status().as_u16()
}

#[test]
fn the_server_checks_parameters_and_bodies() {
    let item = format!("/items/{ID}");
    let mut wrong = vec![];
    let mut expect = |want: u16, method: &str, uri: &str, header: Option<(&str, &str)>, body: &str| {
        let got = status(method, uri, header, body);
        if got != want {
            wrong.push(format!("{method} {uri} {header:?} {body}: {got}, not {want}"));
        }
    };
    expect(200, "GET", &item, None, "");
    expect(400, "GET", "/items/%7B01234567-89ab-4def-8123-456789abcdef%7D", None, "");
    expect(200, "GET", &format!("{item}?since=2024-06-19T08:30:06Z"), None, "");
    expect(400, "GET", &format!("{item}?since=2024-06-19%2008:30:06Z"), None, "");
    expect(200, "GET", &item, Some(("x-day", "2024-06-19")), "");
    expect(400, "GET", &item, Some(("x-day", "20240619")), "");
    expect(200, "GET", &format!("{item}?days=2024-06-19&days=2024-06-20"), None, "");
    expect(400, "GET", &format!("{item}?days=2024-06-19&days=20240620"), None, "");
    expect(204, "POST", "/stamps", None, "\"2024-06-19T08:30:06Z\"");
    expect(400, "POST", "/stamps", None, "\"2024-06-19 08:30:06Z\"");
    expect(204, "POST", "/days", None, "\"2024-06-19\"\n\"2024-06-20\"\n");
    // The body is read as it arrives: the wrong item reaches the handler as an error, and it answers 422.
    expect(422, "POST", "/days", None, "\"2024-06-19\"\n\"20240620\"\n");
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// A transport that answers every request with one response.
#[derive(Clone)]
struct Answer {
    content_type: &'static str,
    location: &'static str,
    body: &'static str,
}

impl api::Service<api::Request> for Answer {
    type Response = api::Response;
    type Error = std::convert::Infallible;
    type Future = std::future::Ready<Result<api::Response, std::convert::Infallible>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: api::Request) -> Self::Future {
        let response = api::http::Response::builder()
            .status(200)
            .header("content-type", self.content_type)
            .header("location", self.location)
            .body(api::Body::from(self.body))
            .unwrap();
        std::future::ready(Ok(response))
    }
}

/// A client checks the text before its library reads it, as the server does: jiff reads a space and url reads one too,
/// but the formats refuse them.
#[test]
fn the_client_reads_only_what_the_format_allows() {
    let item = |location: &'static str, body: &'static str| {
        let client = api::Client::new(Answer { content_type: "application/json", location, body });
        api::block_on(client.get_item(ops::get_item::Params::new(ID.parse().unwrap()))).is_ok()
    };
    let log = |body: &'static str| {
        let client = api::Client::new(Answer { content_type: "application/jsonl", location: "/", body });
        api::block_on(client.get_log(ops::get_log::Params {})).is_ok()
    };
    let streamed = |body: &'static str| {
        let client = api::Client::new(Answer { content_type: "application/jsonl", location: "/", body });
        api::block_on(async {
            match client.get_log_stream(ops::get_log::Params {}).await {
                Ok(api::Streamed::Ok(items)) => api::Items::Stream(items).collect().await.is_ok(),
                _ => false,
            }
        })
    };
    let read = [
        item("https://example.com/a", "\"2024-06-19 08:30:06Z\""),
        item("https://example.com/a b", "\"2024-06-19T08:30:06Z\""),
        log("\"2024-06-19\"\n\"20240620\"\n"),
        streamed("\"2024-06-19\"\n\"20240620\"\n"),
    ];
    let what = "a space in a date-time, a space in a URL, a date without hyphens whole and as it arrives";
    assert_eq!(read, [false; 4], "{what}");
}
}
"#;
