//! Shape of the generated code that only a compiled crate would otherwise
//! catch. That is adapter features, per-operation servers and `Default` on `Params`.

use super::common;

use serde_json::json;

/// The `pub mod <op> { ... }` block of `operations.rs`.
fn op_module<'a>(ops: &'a str, op: &str) -> &'a str {
    let header = format!("pub mod {op} {{");
    let (_, rest) = ops.split_once(&header).unwrap_or_else(|| panic!("no module {op}"));
    rest.split_once("\npub mod ").map_or(rest, |(module, _)| module)
}

fn operations(test: &str) -> String {
    let doc = common::spec(
        "3.1.0",
        &json!({
            "/a": {"get": {
                "operationId": "a",
                "servers": [{"url": "https://a.example.com"}],
                "parameters": [{"name": "q", "in": "query", "schema": {"type": "string"}}],
                "responses": {"204": {"description": "ok"}},
            }},
            // A required parameter whose type has no `Default`: an open string enum.
            "/b": {"get": {
                "operationId": "b",
                "parameters": [{"name": "kind", "in": "query", "required": true, "schema": {"type": "string", "enum": ["x", "y"]}}],
                "responses": {"204": {"description": "ok"}},
            }},
            // A required body: defaults when its one media type does.
            "/d": {"post": {
                "operationId": "d",
                "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "properties": {"n": {"type": "integer"}}}}}},
                "responses": {"204": {"description": "ok"}},
            }},
            // A required body with two media types is an enum: no default. Its
            // multipart part header is left out of samples (the sample may be JSON).
            "/e": {"post": {
                "operationId": "e",
                "requestBody": {"required": true, "content": {
                    "application/json": {"schema": {"type": "object", "properties": {"n": {"type": "integer"}}}},
                    "multipart/form-data": {
                        "schema": {"type": "object", "properties": {"file": {"type": "string", "format": "binary"}}},
                        "encoding": {"file": {"headers": {"X-Sum": {"schema": {"type": "string"}}}}},
                    },
                }},
                "responses": {"204": {"description": "ok"}},
            }},
            // A multipart-only body: its part header is sampled like any parameter.
            "/f": {"post": {
                "operationId": "f",
                "requestBody": {"required": true, "content": {"multipart/form-data": {
                    "schema": {"type": "object", "properties": {"file": {"type": "string", "format": "binary"}}},
                    "encoding": {"file": {"headers": {"X-Sum": {"schema": {"type": "string"}}}}},
                }}},
                "responses": {"204": {"description": "ok"}},
            }},
            // An optional body of that type defaults to `None`.
            "/c": {"post": {
                "operationId": "c",
                "requestBody": {"content": {"application/json": {"schema": {"type": "string", "enum": ["x", "y"]}}}},
                "responses": {"204": {"description": "ok"}},
            }},
        }),
        &json!({}),
    );
    let files = common::generate(test, &doc, &crate::emit::Options::default());
    common::file(&files, "operations.rs").to_owned()
}

#[test]
fn only_operations_with_their_own_servers_get_servers() {
    let ops = operations("shape-servers");
    let a = op_module(&ops, "a");
    assert!(a.contains("pub const SERVERS: &[::honig::ServerUrl]") && a.contains("\"https://a.example.com\""), "{ops}");
    assert!(!op_module(&ops, "b").contains("SERVERS"), "{ops}");
}

/// A handler returns its operation's `Result`: its `Response` or an error that the server answers.
#[test]
fn a_handler_returns_its_operations_result() {
    let doc = common::spec(
        "3.2.0",
        &json!({"/a": {"get": {"operationId": "a", "responses": {"204": {"description": "done"}}}}}),
        &json!({}),
    );
    let files = common::generate("op-result", &doc, &crate::emit::Options::default());
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains("pub type Result<T = Response, E = ::honig::ApiError> = std::result::Result<T, E>;"), "{ops}");
    let server = common::file(&files, "server.rs");
    assert!(server.contains("Output = ops::a::Result>"), "{server}");
}

/// honig's checked text of a format sits beside the types that hold it: `types::UuidStr`. A schema of that name
/// keeps it.
#[test]
fn format_strs_sit_beside_the_types() {
    let types = |schemas: serde_json::Value| {
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": schemas}));
        common::file(&common::generate("format-strs", &doc, &crate::emit::Options::default()), "types.rs").to_owned()
    };
    let id = json!({"type": "object", "properties": {"id": {"type": "string", "format": "uuid"}}});
    assert!(types(json!({"Item": id})).contains("pub use ::honig::UuidStr;"));
    let own = types(json!({"Item": id, "UuidStr": {"type": "integer"}}));
    assert!(!own.contains("pub use ::honig::UuidStr"), "{own}");
}

/// A `oneOf` branch that only lists `required` is counted by its names: no type of its own decodes the value.
#[test]
fn a_required_only_branch_is_counted_by_its_names() {
    let contact = json!({"type": "object", "properties": {"email": {"type": "string"}, "phone": {"type": "string"}},
        "oneOf": [{"required": ["email"]}, {"required": ["phone"], "description": "by phone"}]});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Contact": contact}}));
    let types =
        common::file(&common::generate("required-keys", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains(r#"json.contains_key("phone")"#), "{types}");
    assert!(!types.contains("NotChecked"), "{types}");
}

/// Twin types, two enums of the same values or two unions of the same variants, each write their impls through runtime
/// macros that take their variants: no file writes a macro of its own.
#[test]
fn twin_types_share_runtime_macros() {
    let kind = json!({"type": "string", "enum": ["a", "b"]});
    let union = json!({"oneOf": [{"type": "string"}, {"type": "integer"}]});
    let doc =
        common::spec("3.2.0", &json!({}), &json!({"schemas": {"Kind": kind, "Sort": kind, "A": union, "B": union}}));
    let types =
        common::file(&common::generate("twin-macros", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(!types.contains("macro_rules!"), "{types}");
    assert!(types.contains(r#"::honig::str_enum_impls!(Sort { A => "a", B => "b" });"#), "{types}");
    assert!(types.contains("::honig::union_serialize!(B {"), "{types}");
}

/// A struct with a required field has no `Default`: `Error { code, ..Default::default() }` would still compile after the
/// description adds a required field, and the new field would start as an empty value. One without has it.
#[test]
fn a_struct_with_a_required_field_has_no_default() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {
            "Error": {"type": "object", "required": ["code"], "properties": {"code": {"type": "string"}}},
            "Filter": {"type": "object", "properties": {"q": {"type": "string"}}}
        }}),
    );
    let types = common::file(&common::generate("shape-no-default", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    let derive_of = |name: &str| {
        let (before, _) = types.split_once(&format!("pub struct {name} ")).expect("the struct");
        before.rsplit_once("#[derive(").expect("a derive").1.split(")]").next().unwrap_or_default().to_owned()
    };
    assert!(!derive_of("Error").contains("Default"), "{types}");
    assert!(derive_of("Filter").contains("Default"), "{types}");
}

/// `into_result` gives a tuple of the body and headers only when no two have the same type: a reorder of two headers
/// of one type would swap them without a compile error. Else it gives a `Success` struct that names each.
#[test]
fn into_result_gives_no_tuple_of_two_values_of_one_type() {
    let header = |schema: serde_json::Value| json!({"schema": schema});
    let responses = |second: serde_json::Value| json!({"200": {"description": "ok", "headers": {"ETag": header(json!({"type": "string"})), "Rev": header(second)}}});
    let ops = |name: &str, second: serde_json::Value| {
        let paths = json!({"/a": {"get": {"operationId": "a", "responses": responses(second)}}});
        let doc = common::spec("3.2.0", &paths, &json!({}));
        common::file(&common::generate(name, &doc, &crate::emit::Options::default()), "operations.rs").to_owned()
    };
    let same = ops("tuple-same", json!({"type": "string"}));
    assert!(!same.contains("pub type Success"), "{same}");
    assert!(
        same.contains("pub struct Success {\n        pub e_tag: Option<String>,\n        pub rev: Option<String>,"),
        "{same}"
    );
    assert!(same.contains("pub fn into_result(self) -> Result<Success, Box<Self>>"), "{same}");
    let different = ops("tuple-different", json!({"type": "integer"}));
    assert!(different.contains("pub type Success"), "{different}");
}

/// The `then` of a conditional keeps every limit: `maxProperties` is checked, and the `name` that the base and the
/// `then` both describe meets both. A branch of a union that took only the `then` would drop the other.
#[test]
fn a_then_keeps_its_limits_beside_those_of_the_base() {
    let person = json!({"type": "object",
        "properties": {"kind": {"type": "string"}, "name": {"type": "string", "maxLength": 5}},
        "if": {"properties": {"kind": {"const": "person"}}},
        "then": {"properties": {"name": {"minLength": 1}}, "maxProperties": 2}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Person": person}}));
    let types =
        common::file(&common::generate("then-limits", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("`then`: "), "the `then` is checked as written: {types}");
    assert!(!types.contains("pub enum Person"), "no union that drops the limits of the `then`: {types}");
}

/// A checked integer with only bounds has a `const fn new` that returns an `Option`: in a constant, `expect` makes a
/// literal outside them a build error. One that another keyword also limits has none: the check cannot run in a
/// `const fn`.
#[test]
fn a_bounded_integer_has_a_const_constructor() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {
            "Limit": {"type": "integer", "minimum": 1, "maximum": 100},
            "Level": {"type": "integer", "exclusiveMinimum": 0},
            "Even": {"type": "integer", "minimum": 0, "multipleOf": 2},
            "Rank": {"type": "integer", "format": "uint8", "maximum": 9}
        }}),
    );
    let types =
        common::file(&common::generate("const-literal", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    let has = |name: &str| {
        let (before, _) = types.split_once(&format!("::honig::checked_num_impls!({name},")).expect("the type");
        before.rsplit_once("impl ").is_some_and(|(_, last)| {
            last.starts_with(&format!("{name} {{")) && last.contains("const fn new(value: i64) -> Option<Self>")
        })
    };
    assert!(has("Limit") && has("Level"), "{types}");
    assert!(!has("Even"), "{types}");
}

/// A `default` response carries a status enum of the registered codes that no other response declares: a status that
/// a client would decode as another variant has no value.
#[test]
fn a_default_status_leaves_out_the_declared_codes() {
    let ok = json!({"description": "ok"});
    let doc = common::spec(
        "3.2.0",
        &json!({"/a": {"get": {"operationId": "a", "responses": {"200": ok, "404": ok, "5XX": ok, "default": ok}}}}),
        &json!({}),
    );
    let files = common::generate("default-status", &doc, &crate::emit::Options::default());
    let (ops, server) = (common::file(&files, "operations.rs"), common::file(&files, "server.rs"));
    assert!(!server.contains("| ops::a::Response::Default"), "two status types in one arm: {server}");
    let (_, statuses) = ops.split_once("DefaultStatus {").expect("the enum");
    let statuses = statuses.split_once('}').expect("its end").0;
    assert!(statuses.contains("Conflict = 409") && !statuses.contains("BadGateway"), "{statuses}");
    assert!(!statuses.contains("Ok =") && !statuses.contains("NotFound ="), "{statuses}");
}

/// An `OAuth` helper of the client takes the scope enum of its scheme: a scope that the description does not list does
/// not compile.
#[test]
fn an_oauth_helper_takes_the_scopes_of_its_scheme() {
    let flows = json!({"clientCredentials": {"tokenUrl": "https://id.example/token", "scopes": {"read": "r"}}});
    let doc = common::spec(
        "3.2.0",
        &json!({"/a": {"get": {"operationId": "a", "security": [{"app": ["read"]}],
            "responses": {"204": {"description": "ok"}}}}}),
        &json!({"securitySchemes": {"app": {"type": "oauth2", "flows": flows}}}),
    );
    let client = common::file(&common::generate("oauth-scopes", &doc, &crate::emit::Options::default()), "client.rs")
        .split_whitespace()
        .collect::<String>();
    assert!(client.contains("client_secret:&str,scopes:&[ops::AppScope],"), "{client}");
}

/// Every `Params` has `new` accepting what is required; `Default` only when that is nothing.
#[test]
fn params_always_have_new_and_default_only_without_arguments() {
    let ops = operations("shape-default");
    let module = |op: &str| op_module(&ops, op);
    let has_default = |op: &str| module(op).contains("::honig::default_from_new!(Params)");
    for op in ["a", "b", "c", "d", "e", "f"] {
        assert!(module(op).contains("fn new("), "{op} has new: {ops}");
        assert!(!module(op).contains("Default)]\n    pub struct Params"), "{op} derives no Default: {ops}");
    }
    assert!(module("a").contains("pub const fn new() -> Self"), "one optional parameter: {ops}");
    assert!(has_default("a"), "new() without arguments is Default: {ops}");
    assert!(module("b").contains("pub const fn new(kind: "), "a required query parameter is an argument: {ops}");
    assert!(!has_default("b"), "{ops}");
    assert!(module("c").contains("pub const fn new() -> Self"), "an optional body starts at None: {ops}");
    assert!(module("d").contains("pub const fn new(body: "), "a required body is an argument: {ops}");
    assert!(module("e").contains("pub const fn new(body: "), "a body with two media types too: {ops}");
}

/// A string with a `pattern` is always a checked newtype. The description uses `pattern`. So generated code needs the
/// `regress` feature of honig.
#[test]
fn pattern_strings_are_always_checked_newtypes() {
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {"Team": {"type": "object", "properties": {"slug": {"type": "string", "pattern": "^[a-z]+$"}}}}}),
    );
    let g = crate::generate::generate(&common::write("shape-pattern", &doc, &[]), &crate::emit::Options::default())
        .expect("generates");
    let types = common::file(&g.files, "types.rs");
    assert!(types.contains("pub struct TeamSlug(pub(super) String);"), "{types}");
    assert!(types.contains("impl TryFrom<String> for TeamSlug"), "{types}");
    assert!(g.runtime_features.contains(&crate::settings::REGRESS), "{:?}", g.runtime_features);
    // No `pattern`: no `regress`.
    let plain = common::spec("3.1.0", &json!({}), &json!({"schemas": {"Team": {"type": "string"}}}));
    let g =
        crate::generate::generate(&common::write("shape-pattern-none", &plain, &[]), &crate::emit::Options::default())
            .expect("generates");
    assert!(!g.runtime_features.contains(&crate::settings::REGRESS), "{:?}", g.runtime_features);
    assert!(!g.files.iter().any(|(_, t)| t.contains("pattern_matches")), "no pattern checks");
}

/// Only a client of an `authorizationCode` flow needs `pkce` and with it `sha2`. Other flows and the server do not.
#[test]
fn the_authorization_code_client_alone_turns_on_pkce() {
    let flows = |flow: &str, urls: serde_json::Value| {
        common::spec(
            "3.1.0",
            &json!({}),
            &json!({"securitySchemes": {"shop_auth": {"type": "oauth2", "flows": {flow: urls}}}}),
        )
    };
    let code = flows(
        "authorizationCode",
        json!({"authorizationUrl": "https://shop.example/auth", "tokenUrl": "https://shop.example/token", "scopes": {}}),
    );
    let features = |name: &str, doc: &serde_json::Value, opts: &crate::emit::Options| {
        crate::generate::generate(&common::write(name, doc, &[]), opts).expect("generates").runtime_features
    };
    let both = crate::emit::Options::default();
    let server = crate::emit::Options { client: false, ..crate::emit::Options::default() };
    assert!(features("shape-pkce", &code, &both).contains(&crate::settings::PKCE));
    assert!(!features("shape-pkce-server", &code, &server).contains(&crate::settings::PKCE));
    let credentials = flows("clientCredentials", json!({"tokenUrl": "https://shop.example/token", "scopes": {}}));
    assert!(!features("shape-pkce-none", &credentials, &both).contains(&crate::settings::PKCE));
}

/// A client of any `OAuth` 2.0 flow, or of discovery alone, turns on honig's `oauth` helpers.
#[test]
fn each_oauth_flow_alone_turns_on_oauth() {
    let urls = json!({"tokenUrl": "https://shop.example/token", "authorizationUrl": "https://shop.example/auth",
        "deviceAuthorizationUrl": "https://shop.example/device", "scopes": {}});
    let schemes = ["clientCredentials", "password", "authorizationCode", "implicit", "deviceAuthorization"]
        .map(|flow| json!({"type": "oauth2", "flows": {flow: urls}}))
        .into_iter()
        .chain([json!({"type": "openIdConnect", "openIdConnectUrl": "https://shop.example/.well-known/openid-configuration"})]);
    for (i, scheme) in schemes.enumerate() {
        let doc = common::spec("3.2.0", &json!({}), &json!({"securitySchemes": {"auth": scheme}}));
        let g = crate::generate::generate(
            &common::write(&format!("shape-oauth-{i}"), &doc, &[]),
            &crate::emit::Options::default(),
        )
        .expect("generates");
        assert!(g.runtime_features.contains(&crate::settings::OAUTH), "{scheme}: {:?}", g.runtime_features);
    }
}

/// Any one deprecated item allows `deprecated` in the module: a schema, a scheme, an operation, a parameter or a
/// response header.
#[test]
fn one_deprecated_item_allows_deprecated() {
    let op = |op: serde_json::Value| json!({"/a": {"get": op}});
    let ok = json!({"200": {"description": "ok"}});
    let docs = [
        common::spec(
            "3.2.0",
            &json!({}),
            &json!({"securitySchemes": {"k": {"type": "apiKey", "in": "header", "name": "K", "deprecated": true}}}),
        ),
        common::spec("3.2.0", &op(json!({"operationId": "a", "deprecated": true, "responses": ok})), &json!({})),
        common::spec(
            "3.2.0",
            &op(json!({"operationId": "a", "responses": ok,
            "parameters": [{"name": "q", "in": "query", "deprecated": true, "schema": {"type": "string"}}]})),
            &json!({}),
        ),
        common::spec(
            "3.2.0",
            &op(json!({"operationId": "a", "responses": {"200": {"description": "ok",
            "headers": {"X-Old": {"deprecated": true, "schema": {"type": "string"}}}}}})),
            &json!({}),
        ),
    ];
    for (i, doc) in docs.iter().enumerate() {
        let files = common::generate(&format!("shape-deprecated-{i}"), doc, &crate::emit::Options::default());
        let root = common::file(&files, "mod.rs").split_whitespace().collect::<String>();
        assert!(root.contains("#![allow(deprecated,"), "{i}: {root}");
    }
}

/// An operation that sends `If-None-Match` and declares `304`, or the `3XX` range, decodes it as declared: the client
/// adds no `NotModified` of its own.
#[test]
fn a_declared_not_modified_is_decoded_as_declared() {
    for status in ["304", "3XX"] {
        let doc = common::spec(
            "3.1.0",
            &json!({"/a": {"get": {"operationId": "a",
                "parameters": [{"name": "If-None-Match", "in": "header", "schema": {"type": "string"}}],
                "responses": {"200": {"description": "ok"}, status: {"description": "same"}}}}}),
            &json!({}),
        );
        let files = common::generate(&format!("shape-304-{status}"), &doc, &crate::emit::Options::default());
        let client = common::file(&files, "client.rs");
        assert!(!client.contains("ClientError::NotModified"), "{status}: {client}");
    }
}

/// A client or a server for some operations lists each by its `operationId` or by its generated name. Two ids that
/// make one name (`getA`, `get_a`) give the second a number, and its id still lists it.
#[test]
fn an_operation_is_listed_by_its_id_or_its_name() {
    let ok = json!({"204": {"description": "ok"}});
    let doc = common::spec(
        "3.1.0",
        &json!({"/a": {"get": {"operationId": "getA", "responses": ok}},
            "/b": {"get": {"operationId": "fetchB", "responses": ok}},
            "/c": {"get": {"responses": ok}}}),
        &json!({}),
    );
    let listed = |names: &[&str]| {
        let opts = crate::emit::Options {
            operations: Some(names.iter().map(|n| (*n).to_owned()).collect()),
            ..crate::emit::Options::default()
        };
        let files = common::generate(&format!("shape-listed-{}", names.join("-")), &doc, &opts);
        let client = common::file(&files, "client.rs");
        ["get_a(", "fetch_b(", "get_c("].map(|f| client.contains(&format!("pub async fn {f}")))
    };
    assert_eq!(listed(&["getA"]), [true, false, false]);
    assert_eq!(listed(&["get_c"]), [false, false, true]);
    assert_eq!(listed(&["fetchB"]), [false, true, false], "by id");
    assert_eq!(listed(&["fetch_b"]), [false, true, false], "by name");
}

/// A tag nested deeper than 16 parents is left out, and a parent shows only for a descendant that shows.
#[test]
fn a_tag_too_deep_is_left_out() {
    let mut doc = common::spec(
        "3.2.0",
        &json!({"/a": {"get": {"operationId": "a", "tags": ["t20"], "responses": {"204": {"description": "ok"}}}}}),
        &json!({}),
    );
    let tags: Vec<serde_json::Value> = (0..=20)
        .map(|i| {
            if i == 0 {
                json!({"name": "t0"})
            } else {
                json!({"name": format!("t{i}"), "parent": format!("t{}", i - 1)})
            }
        })
        .collect();
    doc["tags"] = serde_json::Value::Array(tags);
    let files = common::generate("shape-tag-depth", &doc, &crate::emit::Options::default());
    let ops = common::file(&files, "operations.rs");
    assert!(!ops.contains("pub mod tags"), "{ops}");
}

/// The string formats of a description turn on honig features that check them, and nothing else: `idn` for mail
/// addresses and host names (typed or not), `regress` for `regex` without a library, `jiff` for `time` with jiff dates.
#[test]
fn formats_turn_on_the_features_that_check_them() {
    let features = |schema: serde_json::Value, libs: crate::settings::FormatLibs| {
        let doc = common::spec("3.1.0", &json!({}), &json!({"schemas": {"S": schema}}));
        let mut opts = crate::emit::Options::default();
        opts.rust.libs = libs;
        let g =
            crate::generate::generate(&common::write("shape-format-features", &doc, &[]), &opts).expect("generates");
        assert!(
            g.runtime_features.iter().all(|f| crate::settings::FEATURES.iter().any(|(k, _)| k == f)),
            "{:?}",
            g.runtime_features
        );
        g.runtime_features
    };
    let none = crate::settings::FormatLibs::default;
    let has = |schema: serde_json::Value, f: &str| features(schema, none()).contains(&f);
    assert!(has(json!({"type": "string", "format": "email"}), crate::settings::IDN));
    assert!(has(json!({"format": "hostname"}), crate::settings::IDN), "a format without `type` is checked at run time");
    assert!(!has(json!({"type": "object", "properties": {"format": {"type": "string"}}}), crate::settings::IDN));
    assert!(has(json!({"type": "string", "format": "regex"}), crate::settings::REGRESS));
    // A type feature is yours: the code builds with or without it.
    let jiff = crate::settings::FormatLibs { jiff: true, ..none() };
    assert!(!features(json!({"type": "string", "format": "time"}), jiff).contains(&crate::settings::JIFF));
}

/// A tag with a `parent` is a module of its own, and a server URL that is a path is the base path of the server. A path
/// variable that no parameter declares and a `parent` that is no tag are errors: `generate` fails at their pointers.
#[test]
#[expect(clippy::literal_string_with_formatting_args, reason = "path templates of the description")]
fn child_tags_and_relative_servers() {
    let y = json!([{"name": "y", "in": "path", "required": true, "schema": {"type": "string"}}]);
    let with = |path: &str, parent: &str| {
        let mut doc = common::spec(
            "3.2.0",
            &json!({path: {"get": {"operationId": "a", "tags": ["child"], "parameters": y,
                "responses": {"204": {"description": "ok"}}}}}),
            &json!({}),
        );
        doc["tags"] = json!([{"name": "root"}, {"name": "child", "parent": parent}]);
        doc["servers"] = json!([{"url": "/v1"}, {"url": "v2"}]);
        doc
    };
    let files = common::generate("shape-child-tags", &with("/a/{y}", "root"), &crate::emit::Options::default());
    let (server, ops) = (common::file(&files, "server.rs"), common::file(&files, "operations.rs"));
    assert!(server.contains(r#"static BASE_PATHS: &[&str] = &["/v1"];"#), "{server}");
    assert!(ops.contains("pub mod child"), "{ops}");
    for (name, doc, pointer) in [
        ("shape-undeclared-variable", with("/a/{x}/{y}", "root"), "/paths/~1a~1{x}~1{y}/get"),
        ("shape-missing-parent", with("/a/{y}", "missing"), "/tags/1/parent"),
    ] {
        let path = common::write(name, &doc, &[]);
        let err = crate::generate::generate(&path, &crate::emit::Options::default()).expect_err(name);
        assert!(err.iter().any(|d| d.pointer == pointer), "{name}: {:#?}", err.0);
    }
}

/// A header parameter other than `Range` leaves ranges to the server: only a `Range` parameter of its own handles them.
#[test]
fn another_header_leaves_ranges_to_the_server() {
    let doc = common::spec(
        "3.2.0",
        &json!({"/a": {"get": {"operationId": "a", "parameters": [{"name": "X-Trace", "in": "header", "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok", "content": {"application/octet-stream": {"schema": {"type": "string", "format": "binary"}}}}}}}}),
        &json!({}),
    );
    let server =
        common::file(&common::generate("shape-ranges-header", &doc, &crate::emit::Options::default()), "server.rs")
            .to_owned();
    assert!(server.contains("ranges: true,"), "{server}");
}

/// An `http` scheme other than basic or bearer keeps its spelling: a server may compare `Token` case-sensitively.
#[test]
fn another_http_scheme_keeps_its_spelling() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/orders": {"get": {
            "operationId": "listOrders",
            "security": [{"shop_token": []}],
            "responses": {"204": {"description": "OK"}}
        }}}),
        &json!({"securitySchemes": {"shop_token": {"type": "http", "scheme": "Token"}}}),
    );
    let files = common::generate("shape-http-token", &doc, &crate::emit::Options::default());
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains(r#"::honig::__private::Scheme::Http("shop_token", "Token")"#), "{ops}");
}

/// A required header of an error response is one that the server's own refusals cannot carry: a warning says so.
#[test]
fn a_required_error_header_is_warned_about() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/orders": {"get": {
            "operationId": "listOrders",
            "responses": {
                "204": {"description": "OK"},
                "default": {"description": "Error", "headers": {
                    "Shop-Version": {"required": true, "schema": {"type": "string"}},
                    "Shop-Hint": {"schema": {"type": "string"}}
                }}
            }
        }}}),
        &json!({}),
    );
    let g =
        crate::generate::generate(&common::write("shape-error-header", &doc, &[]), &crate::emit::Options::default())
            .expect("generates");
    let pointers: Vec<&str> =
        g.diagnostics.iter().filter(|d| d.message.contains("tower layer")).map(|d| d.pointer.as_str()).collect();
    assert_eq!(pointers, ["/paths/~1orders/get/responses/default/headers/Shop-Version"]);
}

/// A binary that calls the API gets `Params::new` and `api::Client` as a library does. One that only serves it gets
/// no `Params::new`. In both the unused constructors are no dead code warning.
#[test]
fn a_binary_with_a_client_gets_the_constructors() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/orders/{id}": {"get": {
            "operationId": "getOrder",
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "OK", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Order"}}}}}
        }}}),
        &json!({"schemas": {"Order": {"type": "object", "required": ["id"], "properties": {"id": {"type": "integer"}}}}}),
    );
    let opts = |client: bool| crate::emit::Options {
        client,
        rust: crate::emit::rust::Options { binary: true, ..Default::default() },
        ..crate::emit::Options::default()
    };
    let files = common::generate("shape-binary-client", &doc, &opts(true));
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains("pub(crate) const fn new(id: i64) -> Self"), "{ops}");
    assert!(ops.contains("#![allow(\n    dead_code,"), "{ops}");
    assert!(common::file(&files, "mod.rs").contains("pub(crate) use client::Client;"));
    // `types::Order::new` too: a binary may decode every order and build none.
    assert!(common::file(&files, "types.rs").contains("#![allow(dead_code, reason = \"a binary may build none"));
    let files = common::generate("shape-binary-server", &doc, &opts(false));
    let ops = common::file(&files, "operations.rs");
    assert!(!ops.contains("fn new("), "{ops}");
    // A handler may build only some of the responses of its operation.
    assert!(ops.contains("#![allow(\n    dead_code,\n    reason = \"a handler may build only some"), "{ops}");
}

/// A binary that serves the API: an operation with a choice of media types, more required parameters than clippy
/// allows, one security requirement, and a type with more required properties than clippy allows and an example that
/// `documented_examples.rs` decodes.
fn wide_binary(test: &str, client: bool) -> Vec<(String, String)> {
    // Each value an enum of its own: no two can swap, so `new` accepts them all.
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let params: Vec<serde_json::Value> = names
        .iter()
        .map(|n| json!({"name": n, "in": "query", "required": true, "schema": {"type": "string", "enum": ["x", "y"]}}))
        .collect();
    let properties: serde_json::Map<String, serde_json::Value> =
        names.iter().map(|n| ((*n).to_owned(), json!({"type": "string", "enum": ["x", "y"]}))).collect();
    let example: serde_json::Map<String, serde_json::Value> =
        names.iter().map(|n| ((*n).to_owned(), json!("x"))).collect();
    let mut doc = common::spec(
        "3.1.0",
        &json!({"/wide": {"get": {
            "operationId": "wide",
            "security": [{"key": []}],
            "parameters": params,
            "responses": {"200": {"description": "ok", "content": {
                "application/json": {"schema": {"$ref": "#/components/schemas/Wide"}},
                "application/xml": {"schema": {"$ref": "#/components/schemas/Wide"}},
            }}},
        }}}),
        &json!({
            "schemas": {"Wide": {"type": "object", "required": names, "properties": properties, "example": example}},
            "securitySchemes": {"key": {"type": "apiKey", "in": "header", "name": "X-Key"}},
        }),
    );
    doc["info"]["title"] = json!(test);
    let opts = crate::emit::Options {
        client,
        tests: true,
        rust: crate::emit::rust::Options { binary: true, ..Default::default() },
        ..crate::emit::Options::default()
    };
    let files = common::generate(test, &doc, &opts);
    common::file(&files, "documented_examples.rs");
    files
}

/// The `#![allow]`s that every generated file needs sit once in `mod.rs` and apply to its children.
#[test]
fn the_module_allows_once() {
    let files = wide_binary("shape-allow-once", true);
    let reason = "generated from the OpenAPI description";
    let holders: Vec<&str> = files.iter().filter(|(_, t)| t.contains(reason)).map(|(f, _)| f.as_str()).collect();
    assert_eq!(holders, ["mod.rs"]);
}

/// A server decodes a request body with no status (`None`), a client a response with its status.
#[test]
fn a_request_decodes_without_a_status() {
    let body = json!({"required": true, "content": {"application/json": {"schema": {"type": "object"}}}});
    let doc = common::spec(
        "3.1.0",
        &json!({"/a": {"post": {"operationId": "a", "requestBody": body, "responses": {"200": {"description": "ok",
            "content": {"application/json": {"schema": {"type": "object"}}}}}}}}),
        &json!({}),
    );
    let files = common::generate("shape-request-status", &doc, &crate::emit::Options::default());
    let server = common::file(&files, "server.rs");
    assert!(server.contains("::honig::__private::json_decode(None, &body)"), "{server}");
    let client = common::file(&files, "client.rs");
    assert!(client.contains("::honig::__private::json_decode(Some(status), &body)"), "{client}");
}

/// A crate that denies `clippy::allow_attributes` accepts the generated code: every `#[allow]` is an inner `#![allow]`,
/// which that lint leaves alone. `new` allows its many arguments inside its body, not with an `#[expect]` that a crate
/// with a higher `too-many-arguments-threshold` would leave unfulfilled.
#[test]
fn a_binary_gets_no_outer_allow() {
    // A client binary gets `Params::new` too.
    for (test, client, name) in
        [("shape-outer-allow", false, "types.rs"), ("shape-outer-allow-client", true, "operations.rs")]
    {
        let files = wide_binary(test, client);
        for (file, text) in &files {
            let outer: Vec<&str> = text.lines().filter(|l| l.trim_start().starts_with("#[allow(")).collect();
            assert!(outer.is_empty(), "{file}: {outer:?}");
            assert!(!text.contains("#[expect("), "{file}: {text}");
        }
        let text = common::file(&files, name);
        let compact: String = text.split_whitespace().collect();
        assert!(compact.contains("->Self{#![allow(clippy::too_many_arguments,"), "{name}: {text}");
    }
}

/// A binary without a client or a server refers to none of the operations, nor their `SECURITY_<n>`: the file allows
/// that dead code once and no constant carries an `#[allow]` of its own.
#[test]
fn a_binary_without_halves_allows_dead_code_once() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/a": {"get": {"operationId": "a", "security": [{"key": []}], "responses": {"204": {"description": "ok"}}}}}),
        &json!({"securitySchemes": {"key": {"type": "apiKey", "in": "header", "name": "X-Key"}}}),
    );
    let opts = crate::emit::Options {
        client: false,
        server: false,
        rust: crate::emit::rust::Options { binary: true, ..Default::default() },
        ..crate::emit::Options::default()
    };
    let files = common::generate("shape-binary-types", &doc, &opts);
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains("const SECURITY_0"), "{ops}");
    assert!(!ops.contains("#[allow("), "{ops}");
    assert!(ops.contains("reason = \"without a client or a server a binary refers to none"), "{ops}");
}

/// A `text/plain` string error body contains the detail: the server fills it for the requests it refuses itself and no
/// warning says otherwise.
#[test]
fn a_text_error_body_is_filled() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/orders/{id}": {"delete": {
            "operationId": "deleteOrder",
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {
                "204": {"description": "Deleted"},
                "404": {"description": "Not found", "content": {"text/plain": {"schema": {"type": "string"}}}}
            }
        }}}),
        &json!({}),
    );
    let g = crate::generate::generate(&common::write("shape-text-error", &doc, &[]), &crate::emit::Options::default())
        .expect("generates");
    let server = common::file(&g.files, "server.rs");
    let compact: String = server.split_whitespace().collect();
    assert!(compact.contains(r#"::honig::ProblemSchema{media:"text/plain",fields:&[],open:false,}"#), "{server}");
    assert!(!g.diagnostics.iter().any(|d| d.message.contains("cannot fill")), "{:?}", g.diagnostics);
}

/// Each operation sets the name of its span by the HTTP server conventions of OpenTelemetry. The `tracing`
/// feature of honig opens it. So generated code never refers to `tracing`.
#[test]
fn operations_carry_their_otel_span_name() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/products/{productId}": {"get": {
            "operationId": "getProduct",
            "parameters": [{"name": "productId", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"204": {"description": "ok"}},
        }}}),
        &json!({}),
    );
    let server =
        common::file(&common::generate("shape-span", &doc, &crate::emit::Options::default()), "server.rs").to_owned();
    let squashed: String = server.split_whitespace().collect();
    assert!(squashed.contains(r#""GET","/products/{productId}","GET/products/{productId}""#), "{server}");
    // The span itself is honig's `tracing` feature: generated code never refers to `tracing`.
    assert!(!server.contains("tracing"), "{server}");
}

/// `Params::new` accepts path parameters in URL order and a required body.
/// `Params` and `Response` have one name each: the operation's module path.
#[test]
fn params_new_and_one_name_per_type() {
    let doc = common::spec(
        "3.1.0",
        &json!({
            "/orgs/{org}/repos/{repo}": {"get": {
                "operationId": "getRepo",
                // Declared out of URL order on purpose.
                "parameters": [
                    {"name": "repo", "in": "path", "required": true, "schema": {"type": "integer"}},
                    {"name": "org", "in": "path", "required": true, "schema": {"type": "string"}},
                    {"name": "page", "in": "query", "schema": {"type": "integer"}},
                ],
                "responses": {"204": {"description": "ok"}},
            }},
            "/a": {"get": {"operationId": "list", "responses": {"204": {"description": "ok"}}}},
            "/b": {"get": {"operationId": "listResponse", "responses": {"204": {"description": "ok"}}}},
            "/c": {"get": {
                "operationId": "sorted",
                "parameters": [{"name": "sort", "in": "query", "schema": {"type": "string", "default": "asc"}}],
                "responses": {"204": {"description": "ok"}},
            }},
        }),
        &json!({}),
    );
    let files = common::generate("shape-new-aliases", &doc, &crate::emit::Options::default());
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains("pub const fn new(org: String, repo: i64) -> Self"), "{ops}");
    // A string default is built at run time (`to_owned`). So that `new` cannot be `const`.
    assert!(op_module(ops, "sorted").contains("    pub fn new() -> Self"), "{ops}");
    assert!(!ops.contains("Params as "), "no short aliases: {ops}");
    let server = common::file(&files, "server.rs");
    assert!(server.contains("params: ops::get_repo::Params,"), "{server}");
    assert!(server.contains("params: ops::list::Params,"), "{server}");
}

/// `new` accepts exactly the required properties without a spec default; the rest start empty or at that default.
#[test]
fn structs_have_new_with_their_required_properties() {
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {
            "Product": {
                "type": "object",
                "required": ["name", "id"],
                "properties": {
                    "id": {"type": "integer"},
                    "name": {"type": "string"},
                    "tag": {"type": "string"},
                    "note": {"type": ["string", "null"]},
                },
                "additionalProperties": {"type": "string"},
            },
            "Page": {"type": "object", "properties": {"size": {"type": "integer", "default": 10}}},
        }}),
    );
    let types = common::file(&common::generate("shape-struct-new", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    let flat = types.split_whitespace().collect::<String>();
    // Document order and not `required` order.
    assert!(flat.contains("pubconstfnnew(id:i64,name:String)->Self"), "{types}");
    assert!(flat.contains("tag:None,note:::honig::Nullable::Absent,"), "{types}");
    assert!(flat.contains("additional_properties:std::collections::BTreeMap::new()"), "{types}");
    // A spec default is filled by its function: `const` for a number literal.
    assert!(flat.contains("pubconstfnnew()->Self{Self{size:Self::default_size()}}"), "{types}");
    assert!(flat.contains("pubconstfndefault_size()->i64{10i64}"), "{types}");
}

/// Generated code keeps some names for itself. Each of these gets a number: a parameter named `body` next to the
/// request body, a response header named `status` next to the status and an operation named like a generated module.
#[test]
fn names_the_generated_code_keeps_get_a_number() {
    let doc = common::spec(
        "3.2.0",
        &json!({"/send": {"post": {
            "operationId": "client",
            "parameters": [{"name": "body", "in": "query", "required": true, "schema": {"type": "string"}}],
            "requestBody": {"content": {"application/json": {"schema": {"type": "string"}}}},
            "responses": {"default": {
                "description": "any",
                "headers": {"status": {"schema": {"type": "integer"}}},
            }},
        }}}),
        &json!({}),
    );
    let files = common::generate("shape-reserved-names", &doc, &crate::emit::Options::default());
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains("pub mod client2 {"), "operation module: {ops}");
    assert!(ops.contains("pub body2: String,"), "query parameter: {ops}");
    assert!(ops.contains("status2:"), "response header: {ops}");
}

/// The server answers `Range` itself only on a `GET` of a binary body whose operation leaves ranges to it: one that
/// declares its own `206` or `Range` parameter answers them in its handler.
#[test]
fn the_server_answers_ranges_only_for_binary_gets_that_leave_them_to_it() {
    let zip = json!({"200": {"description": "ok", "content": {"application/zip": {}}}});
    let range = json!([{"name": "Range", "in": "header", "schema": {"type": "string"}}]);
    let cases = [
        ("binary", json!({"get": {"operationId": "get", "responses": zip}}), true),
        (
            "json",
            json!({"get": {"operationId": "get", "responses": {"200": {"description": "ok",
            "content": {"application/json": {"schema": {"type": "string"}}}}}}}),
            false,
        ),
        ("put", json!({"put": {"operationId": "put", "responses": zip}}), false),
        (
            "own-206",
            json!({"get": {"operationId": "get", "responses": {"200": zip["200"].clone(),
            "206": {"description": "part", "content": {"application/zip": {}}}}}}),
            false,
        ),
        ("own-range", json!({"get": {"operationId": "get", "parameters": range, "responses": zip}}), false),
    ];
    for (name, item, ranges) in cases {
        let doc = common::spec("3.1.0", &json!({"/exports": item}), &json!({}));
        let files = common::generate(&format!("shape-ranges-{name}"), &doc, &crate::emit::Options::default());
        let server = common::file(&files, "server.rs");
        assert_eq!(server.contains("ranges: true,"), ranges, "{name}: {server}");
    }
}

/// Every schema the description declares keeps its name, though another one has its shape: `PaymentMethod` and
/// `DeliverySpeed` are both `enum: [0, 1]`, and so are the inline `status` and `priority` of `Order`. Inline twins in
/// one role (`status` of two operations) still share one type, named for what they share (`OkStatus`). A component
/// parameter's schema is named after its key, not after the first operation that uses it.
#[test]
fn a_named_schema_keeps_its_name_beside_a_twin() {
    let flag = json!({"type": "integer", "enum": [0, 1]});
    let path = |op: &str| {
        json!({"get": {"operationId": op,
            "parameters": [{"$ref": "#/components/parameters/OrderId"}],
            "responses": {"200": {"description": "ok", "content": {"application/json": {"schema": {
                "type": "object", "properties": {"status": flag.clone(), "priority": flag.clone()}}}}}}}})
    };
    let doc = common::spec(
        "3.1.0",
        &json!({"/orders/{orderId}": path("getOrder"), "/orders/{orderId}/copy": path("getOrderCopy")}),
        &json!({
            "schemas": {"PaymentMethod": flag.clone(), "DeliverySpeed": flag.clone()},
            "parameters": {"OrderId": {"name": "orderId", "in": "path", "required": true,
                "schema": {"type": "string", "pattern": "^[a-z]{4}$"}}}
        }),
    );
    let types =
        common::file(&common::generate("shape-twins", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    let aliases: Vec<&str> = types.lines().filter(|l| l.starts_with("pub type ")).collect();
    assert_eq!(
        aliases,
        [
            "pub type GetOrderCopyOk = GetOrderOk;",
            "pub type GetOrderCopyOkStatus = OkStatus;",
            "pub type GetOrderCopyOkPriority = OkPriority;",
        ]
    );
    for own in [
        "pub enum PaymentMethod",
        "pub enum DeliverySpeed",
        "pub enum OkStatus",
        "pub enum OkPriority",
        "pub struct OrderId(",
    ] {
        assert!(types.contains(own), "{own} in {types}");
    }
}

#[test]
fn a_body_of_your_own_type_is_left_to_it() {
    // A body whose `not` no Rust type judges: the generated code checks it, unless the schema is a type of yours.
    let generate = |test: &str, rust_type: Option<&str>| {
        let mut note = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
            "not": {"required": ["a", "b"]},
        });
        if let Some(path) = rust_type {
            note["x-rust-type"] = json!(path);
        }
        let note_ref = json!({"$ref": "#/components/schemas/Note"});
        let doc = common::spec(
            "3.1.0",
            &json!({
                // One JSON body: checked as sent.
                "/one": {"post": {
                    "operationId": "one",
                    "requestBody": {"content": {"application/json": {"schema": note_ref}}},
                    "responses": {"204": {"description": "ok"}},
                }},
                // A JSON body or a form: checked once decoded.
                "/two": {"post": {
                    "operationId": "two",
                    "requestBody": {"content": {
                        "application/json": {"schema": note_ref},
                        "application/x-www-form-urlencoded": {"schema": {"type": "object"}},
                    }},
                    "responses": {"204": {"description": "ok"}},
                }},
            }),
            &json!({"schemas": {"Note": note}}),
        );
        let files = common::generate(test, &doc, &crate::emit::Options::default());
        (common::file(&files, "types.rs").to_owned(), common::file(&files, "server.rs").to_owned())
    };
    // `Note` is a checked type: decoding it, whatever the media type, checks its `not`.
    let (types, _) = generate("shape-own-body-generated", None);
    assert!(types.contains("pub struct Note(pub(super) NoteFields);") && types.contains("matches `not`"), "{types}");
    let (types, _) = generate("shape-own-body-yours", Some("crate::Note"));
    assert!(!types.contains("matches `not`"), "{types}");
}

/// A response of a union of unions: each layer contains the two before it, so 2^40 paths reach the bottom. Its size is
/// found once per type and to the bottom: about 340 bytes for `U39` against about 100 for the struct of `201`, so `Ok`
/// is boxed.
#[test]
fn a_response_of_a_union_of_unions_is_sized_to_the_bottom() {
    let mut schemas = serde_json::Map::new();
    schemas.insert("U0".into(), json!({"type": "object", "required": ["a"], "properties": {"a": {"type": "integer"}}}));
    schemas.insert("U1".into(), json!({"type": "object", "required": ["b"], "properties": {"b": {"type": "string"}}}));
    for i in 2..40 {
        let r = |j: usize| json!({"$ref": format!("#/components/schemas/U{j}")});
        schemas.insert(format!("U{i}"), json!({"anyOf": [r(i - 1), r(i - 2)]}));
    }
    let fields: Vec<String> = (0..13).map(|i| format!("f{i}")).collect();
    let props: serde_json::Map<String, serde_json::Value> =
        fields.iter().map(|f| (f.clone(), json!({"type": "integer", "format": "int64"}))).collect();
    let body =
        |schema: serde_json::Value| json!({"description": "ok", "content": {"application/json": {"schema": schema}}});
    let doc = common::spec(
        "3.1.0",
        &json!({"/u": {"get": {"operationId": "u", "responses": {
            "200": body(json!({"$ref": "#/components/schemas/U39"})),
            "201": body(json!({"type": "object", "required": fields, "properties": props})),
        }}}}),
        &json!({"schemas": schemas}),
    );
    let files = common::generate("shape-union-of-unions", &doc, &crate::emit::Options::default());
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains("Ok(Box<types::U39>)"), "{ops}");
}

/// A `$ref` beside an extension (`x-patch-strategy`) is the referenced type: an extension asserts nothing.
/// Properties named like keywords (`$ref`, `type`) are field names, not keywords.
#[test]
fn a_ref_beside_an_extension_or_keyword_names_is_typed() {
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {
            "Props": {"type": "object", "properties": {
                "$ref": {"type": "string"},
                "type": {"type": "string"},
                "items": {"type": "string"},
                "not": {"type": "string"},
                "strategy": {"$ref": "#/components/schemas/Strategy", "x-patch-strategy": "retainKeys"},
            }},
            "Strategy": {"type": "object", "properties": {"kind": {"type": "string", "maxLength": 9}}},
        }}),
    );
    let files = common::generate("shape-keyword-names", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    assert!(types.contains("pub strategy: Strategy,") && types.contains(r#"#[serde(rename = "$ref")]"#), "{types}");
}

/// Every struct gets a builder, with a checked field or without: the builder is the one way to set fields by name.
#[test]
fn every_struct_gets_a_builder() {
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {
            "Plain": {"type": "object", "required": ["name"], "properties": {
                "name": {"type": "string"}, "count": {"type": "integer"}, "kind": {"type": "string", "enum": ["a", "b"]},
            }},
            "Coded": {"type": "object", "properties": {"code": {"type": "string", "pattern": "^[A-Z]+$"}}},
            "Tagged": {"type": "object", "properties": {
                "tags": {"type": "array", "items": {"type": "string", "pattern": "^[a-z]+$"}},
            }},
        }}),
    );
    let files = common::generate("shape-builders-where-they-pay", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    for name in ["Plain", "Coded", "Tagged"] {
        assert!(types.contains(&format!("{name} => super::{name}")), "{name}: {types}");
    }
}

/// An XML field states only what differs from an element of its own name: the rest comes from `XmlField::new`.
#[test]
fn an_xml_field_names_only_what_differs() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/b": {"get": {"operationId": "b", "responses": {"200": {"description": "ok",
            "content": {"application/xml": {"schema": {"$ref": "#/components/schemas/Book"}}}}}}}}),
        &json!({"schemas": {"Book": {"type": "object", "required": ["id"], "properties": {
            "id": {"type": "string", "xml": {"attribute": true}}, "title": {"type": "string"},
        }}}}),
    );
    let types = common::file(&common::generate("xml-defaults", &doc, &crate::emit::Options::default()), "types.rs")
        .split_whitespace()
        .collect::<String>();
    assert!(
        types.contains(
            r#"::honig::__private::XmlField{attribute:true,required:true,..::honig::__private::XmlField::new("id")}"#
        ),
        "{types}"
    );
    assert!(types.contains(r#",::honig::__private::XmlField::new("title"),"#), "{types}");
}

/// A server operation states only what differs from `Op::new`: an operation without security, body or ranges is the
/// call alone.
#[test]
fn a_server_operation_names_only_what_differs() {
    let files = wide_binary("op-defaults", false);
    let server = common::file(&files, "server.rs").split_whitespace().collect::<String>();
    assert!(server.contains("staticWIDE_OP:::honig::__private::Op=::honig::__private::Op{security:"), "{server}");
    assert!(!server.contains("takes_body:false") && !server.contains("ranges:false"), "{server}");
}

/// A branch whose tag property is a `const` of its own tag writes itself: no `serde_json::Value` in between.
#[test]
fn a_const_tag_serializes_without_a_value_tree() {
    let branch = |kind: &str| json!({"type": "object", "required": ["kind"], "properties": {"kind": {"const": kind}}});
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {
            "A": branch("a"), "B": branch("b"),
            "Either": {"oneOf": [{"$ref": "#/components/schemas/A"}, {"$ref": "#/components/schemas/B"}],
                "discriminator": {"propertyName": "kind", "mapping": {"a": "#/components/schemas/A", "b": "#/components/schemas/B"}}},
        }}),
    );
    let types =
        common::file(&common::generate("const-tag", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(!types.contains("serde_json::to_value(v)"), "{types}");
}

/// A union that is one of its own branches (`Loop` is `Loop` or `null`, `Ping` is `Pong` or a string and `Pong` is
/// `Ping` or a number) generates: a `$ref` to it with a sibling keyword does not spread over its branches for ever, and
/// every walk through its aliases stops.
#[test]
fn a_union_that_is_its_own_branch_generates() {
    let r = |n: &str| json!({"$ref": format!("#/components/schemas/{n}")});
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {
            "Loop": {"anyOf": [r("Loop"), {"type": "null"}]},
        "Solo": {"oneOf": [r("Solo")]},
        "Many": {"anyOf": [r("Solo"), {"type": "string", "minLength": 3}]},
            "Ping": {"anyOf": [r("Pong"), {"type": "string"}]},
            "Pong": {"anyOf": [r("Ping"), {"type": "number"}]},
            "Either": {"anyOf": [r("Loop"), {"type": "string", "minLength": 2}]},
            "Holder": {"type": "object", "properties": {
                "e": r("Either"),
                "l": {"$ref": "#/components/schemas/Loop", "minLength": 1},
                "p": {"$ref": "#/components/schemas/Ping", "minLength": 1},
            }},
        }}),
    );
    let files = common::generate("shape-union-loop", &doc, &crate::emit::Options::default());
    assert!(common::file(&files, "types.rs").contains("pub struct Holder"));
}

/// An `anyOf` or `oneOf` whose branches share a type needs no check of the whole value: decoding checks every keyword
/// of a branch. The first branch that decodes proves an `anyOf`. A `oneOf` decodes every branch and counts.
#[test]
fn an_any_of_is_judged_by_its_variants() {
    let union = |key: &str| {
        json!({key: [
            {"type": "string", "enum": [""]},
            {"type": "string", "maxLength": 5000},
        ]})
    };
    let doc = common::spec("3.1.0", &json!({}), &json!({"schemas": {"Any": union("anyOf"), "One": union("oneOf")}}));
    let files = common::generate("shape-any-of-typed", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    // A union checked whole is the checked type of its variants.
    let checked = |name: &str| types.contains(&format!("pub struct {name}(pub(super) {name}Variants)"));
    assert!(!checked("Any"), "{types}");
    assert!(!checked("One"), "{types}");
    assert!(types.contains("of `oneOf`, not exactly one"), "the `oneOf` counts: {types}");
}

/// `format: time` is honig's `FullTime` whatever the date library: neither date library has a time with an
/// offset, and `FullTime` stores plain numbers.
#[test]
fn a_time_with_its_offset_is_a_full_time_with_any_date_library() {
    let doc = common::spec(
        "3.1.0",
        &json!({}),
        &json!({"schemas": {"Shift": {"type": "object", "required": ["start"], "properties": {
            "start": {"type": "string", "format": "time"},
        }}}}),
    );
    let files = common::generate("shape-full-time", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    assert!(types.contains("pub start: ::honig::FullTime"), "{types}");
}

/// `dependentRequired` over properties that the struct declares is a check of its fields.
/// A name that the struct does not declare, or an optional one whose default hides its absence, is checked on the JSON object.
#[test]
fn dependent_required_checks_the_fields() {
    let types = |schema: serde_json::Value| {
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Payment": schema}}));
        common::file(&common::generate("dependent-required", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned()
    };
    let typed = types(json!({"type": "object", "properties": {"card": {"type": "string"}, "cvc": {"type": "string"}},
        "dependentRequired": {"card": ["cvc"]}}));
    assert!(typed.contains("is required when"), "{typed}");
    let undeclared = types(json!({"type": "object", "properties": {"card": {"type": "string"}},
        "dependentRequired": {"card": ["cvc"]}}));
    // An undeclared name is kept by the struct and checked on the JSON object.
    assert!(undeclared.contains("is required when"), "{undeclared}");
    let defaulted = types(json!({"type": "object", "properties": {"card": {"type": "string"},
        "cvc": {"type": "string", "default": "000"}}, "dependentRequired": {"card": ["cvc"]}}));
    // A name with a default fills in no default here: its absence shows.
    assert!(defaulted.contains("is required when"), "{defaulted}");
}

/// `propertyNames` of `pattern`, `minLength` and `maxLength` is a check of each name. Any
/// other schema of names is a type that each name decodes as. A map component enforces it where it is used, as
/// `minProperties`.
#[test]
fn property_names_checks_each_name() {
    let types = |names: serde_json::Value| {
        let labels = json!({"type": "object", "additionalProperties": {"type": "string"}, "propertyNames": names});
        let tagged = json!({"type": "object", "properties": {"labels": {"$ref": "#/components/schemas/Labels"}}});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Labels": labels, "Tagged": tagged}}));
        common::file(&common::generate("property-names", &doc, &crate::emit::Options::default()), "types.rs").to_owned()
    };
    let typed = types(json!({"pattern": "^[a-z]+$", "maxLength": 8}));
    assert!(typed.contains("for name in"), "{typed}");
    let other = types(json!({"enum": ["a", "b"]}));
    assert!(other.contains("is not an allowed property name"), "{other}");
}

/// One `patternProperties` entry of a closed map is a map of the pattern's schema whose names are checked against the
/// pattern. An open map or two patterns check each matched value.
#[test]
fn a_closed_map_of_one_pattern_is_typed() {
    let types = |scores: serde_json::Value| {
        let scored = json!({"type": "object", "properties": {"scores": {"$ref": "#/components/schemas/Scores"}}});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Scores": scores, "Scored": scored}}));
        common::file(&common::generate("pattern-properties", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned()
    };
    let int = json!({"type": "integer", "minimum": 0});
    let typed = types(json!({"type": "object", "patternProperties": {"^[a-z]+$": int}, "additionalProperties": false}));
    assert!(typed.contains("for name in"), "{typed}");
    let open = types(json!({"type": "object", "patternProperties": {"^[a-z]+$": int}}));
    assert!(open.contains("pub pattern_1: std::collections::BTreeMap<String, ScoresPattern1>,"), "{open}");
    assert!(
        open.contains("pub additional_properties: std::collections::BTreeMap<String, serde_json::Value>,"),
        "{open}"
    );
    let two = types(json!({"type": "object", "patternProperties": {"^[a-z]+$": int, "^x": int},
        "additionalProperties": false}));
    assert!(two.contains("pub pattern_2: std::collections::BTreeMap<String, ScoresPattern2>,"), "{two}");
    assert!(two.contains("pub additional_properties: ::honig::__private::Closed,"), "{two}");
}

/// A schema without `type` whose keywords each apply to one kind of value (`minimum`, `maxLength`, `maxItems`, ...) is
/// a `serde_json::Value` checked by its kind.
#[test]
fn keywords_without_a_type_check_values_of_their_kind() {
    let any = json!({"type": "object", "properties": {"v": {"minimum": 1, "maxLength": 3, "maxItems": 2}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Loose": any}}));
    let types =
        common::file(&common::generate("typeless", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("serde_json::Value::Number"), "{types}");
}

/// `not: S` is a check that the value does not decode as the type of `S`.
#[test]
fn not_is_a_check_against_the_type_of_its_schema() {
    let code = json!({"type": "object", "properties": {"code": {"type": "string", "not": {"type": "string", "pattern": "^x"}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Coded": code}}));
    let types = common::file(&common::generate("not", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains(".matches::<"), "{types}");
}

/// A range check states each bound it has (`max: Some(100)`) and leaves the others to `Bounds::NONE`: no four
/// positional options.
#[test]
fn a_range_check_names_its_bounds() {
    let schema = |s: serde_json::Value| json!({"type": "object", "required": ["n"], "properties": {"n": s}});
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {
            "Int": schema(json!({"type": "integer", "maximum": 100})),
            "Float": schema(json!({"type": "number", "minimum": 0.5, "maximum": 2, "exclusiveMinimum": 0, "exclusiveMaximum": 3})),
        }}),
    );
    let types = common::file(&common::generate("named-bounds", &doc, &crate::emit::Options::default()), "types.rs")
        .split_whitespace()
        .collect::<String>();
    assert!(types.contains("::honig::__private::Bounds{max:Some(100),..::honig::__private::Bounds::NONE}"), "{types}");
    assert!(
        types.contains(
            "::honig::__private::Bounds{min:Some(0.5),max:Some(2.0),exclusive_min:Some(0.0),exclusive_max:Some(3.0),}"
        ),
        "{types}"
    );
}

/// A pattern check prints its pattern once, in its `static`: honig writes the message from it. A checked type
/// also exposes it as `PATTERN`.
#[test]
fn a_pattern_is_printed_once_per_check() {
    let doc =
        common::spec("3.2.0", &json!({}), &json!({"schemas": {"Code": {"type": "string", "pattern": "^[A-Z]+$"}}}));
    let types =
        common::file(&common::generate("pattern-once", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert_eq!(types.matches("^[A-Z]+$").count(), 2, "{types}");
}

/// `dependentSchemas` over properties that the struct declares is a check that the value decodes as the type of the
/// schema when the property is present.
#[test]
fn dependent_schemas_check_against_the_type_of_the_schema() {
    let card = json!({"type": "object", "properties": {"card": {"type": "string"}, "cvc": {"type": "string"}},
        "dependentSchemas": {"card": {"required": ["cvc"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Payment": card}}));
    let types =
        common::file(&common::generate("dependent-schemas", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is present: "), "{types}");
    let voucher = json!({"type": "object", "properties": {"code": {"type": "string"}, "amount": {"type": "integer"}},
        "dependentSchemas": {"code": {"properties": {"amount": {"maximum": 100}}, "required": ["amount"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Voucher": voucher}}));
    let types =
        common::file(&common::generate("dependent-schemas2", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is present: "), "{types}");
}

/// An `if` that is no tagged union checks the value against the types of its schemas: a value of the type of `if` is
/// one of the type of `then`, any other one of the type of `else`.
#[test]
fn if_then_else_checks_against_the_types_of_its_schemas() {
    let size = json!({"type": "object", "properties": {"unit": {"type": "string"}, "value": {"type": "number"}},
        "if": {"properties": {"unit": {"type": "string", "pattern": "^cm$"}}},
        "then": {"properties": {"value": {"type": "number", "maximum": 300}}},
        "else": {"properties": {"value": {"type": "number", "maximum": 120}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Size": size}}));
    let types =
        common::file(&common::generate("if-then-else", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("`then`: "), "{types}");
}

/// A named type checks its own `not`: a field of it gets no checked type of its own.
#[test]
fn a_field_of_a_type_that_checks_itself_takes_no_second_check() {
    let code = json!({"type": "object", "properties": {"a": {"type": "string"}}, "not": {"required": ["a"]}});
    let holder = json!({"type": "object", "properties": {"code": {"$ref": "#/components/schemas/Code"}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Code": code, "Holder": holder}}));
    let types =
        common::file(&common::generate("held-not", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("#[optional]\n        pub code: Code"), "{types}");
}

/// `contains` on an array counts the items that decode as the type of its schema, limited to `minContains` and
/// `maxContains`.
#[test]
fn contains_counts_the_items_of_its_type() {
    let tags = json!({"type": "array", "items": {"type": "string"}, "contains": {"const": "vip"}, "maxContains": 1});
    let tagged = json!({"type": "object", "properties": {"tags": tags}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Tagged": tagged}}));
    let types =
        common::file(&common::generate("contains", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("matching items, more than allowed"), "{types}");
}

/// An `anyOf` with a branch that asserts nothing is true for every value.
#[test]
fn an_any_of_with_a_branch_of_any_value_is_typed() {
    let money = json!({"type": "object", "properties": {"amount": {"type": "integer"}}});
    let price =
        json!({"type": "object", "properties": {"price": {"anyOf": [{"$ref": "#/components/schemas/Money"}, {}]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Money": money, "Price": price}}));
    let types =
        common::file(&common::generate("any-branch", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("Money(Money),\n    Other(serde_json::Value),"), "{types}");
}

/// A `oneOf` whose discriminator picks one branch while two share a type counts the branches that the value decodes
/// as.
#[test]
fn a_one_of_of_shared_types_counts_its_branches() {
    let car = json!({"type": "object", "required": ["wheels"], "properties": {"kind": {"type": "string"}, "wheels": {"type": "integer"}}});
    let van = json!({"type": "object", "required": ["doors"], "properties": {"kind": {"type": "string"}, "doors": {"type": "integer"}}});
    let vehicle = json!({"oneOf": [{"$ref": "#/components/schemas/Car"}, {"$ref": "#/components/schemas/Van"}],
        "discriminator": {"propertyName": "kind"}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Car": car, "Van": van, "Vehicle": vehicle}}));
    let types =
        common::file(&common::generate("one-of-count", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("of `oneOf`, not exactly one"), "{types}");
}

/// `required`, `uniqueItems` and `format` without `type` apply to objects, arrays and strings: no schema is checked
/// whole.
#[test]
fn required_unique_and_format_without_a_type_check_their_kind() {
    let any =
        json!({"type": "object", "properties": {"v": {"required": ["id"], "uniqueItems": true, "format": "uuid"}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Loose": any}}));
    let types =
        common::file(&common::generate("typeless-required", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("::honig::__private::check_unique"), "{types}");
}

/// `patternProperties` beside declared properties is a map per pattern, of the type of its schema: each undeclared
/// name decodes into the map of its pattern, and `additionalProperties: false` refuses the rest. No check inspects the
/// names after decoding. The maps number in the order of the pattern text, so a reorder of the patterns renames none.
#[test]
fn pattern_properties_beside_properties_are_typed_maps() {
    let tags = |first: (&str, serde_json::Value), second: (&str, serde_json::Value)| {
        let mut patterns = serde_json::Map::new();
        patterns.extend([first, second].map(|(p, s)| (p.to_owned(), s)));
        let tags = json!({"type": "object", "properties": {"id": {"type": "string"}},
            "patternProperties": patterns, "additionalProperties": false});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Tags": tags}}));
        common::file(&common::generate("patterns", &doc, &crate::emit::Options::default()), "types.rs").to_owned()
    };
    let (x, n) = (("^x_", json!({"type": "string"})), ("^n_", json!({"type": "integer", "minimum": 0})));
    let types = tags(x.clone(), n.clone());
    assert!(types.contains("pub pattern_1: std::collections::BTreeMap<String, TagsPattern1>,"), "{types}");
    assert!(types.contains("pub pattern_2: std::collections::BTreeMap<String, String>,"), "{types}");
    assert!(
        types.contains("::honig::__private::Closed") && !types.contains("::honig::__private::pattern_matches("),
        "{types}"
    );
    assert_eq!(types, tags(n, x));
}

/// `additionalProperties` with a schema beside `patternProperties` types the names that no pattern matches: a map of
/// each, decoded as its schema, with no check of the names after decoding.
#[test]
fn additional_properties_beside_patterns_types_the_other_names() {
    let tags = json!({"type": "object", "patternProperties": {"^x_": {"type": "string"}},
        "additionalProperties": {"type": "integer"}});
    let holder = json!({"type": "object", "properties": {"tags": tags}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types = common::file(
        &common::generate("additional-beside-patterns", &doc, &crate::emit::Options::default()),
        "types.rs",
    )
    .to_owned();
    assert!(types.contains("pub pattern_1: std::collections::BTreeMap<String, String>,"), "{types}");
    assert!(types.contains("pub additional_properties: std::collections::BTreeMap<String, i64>,"), "{types}");
    assert!(!types.contains("let mut matched = false;"), "{types}");
}

/// A tuple that no Rust tuple represents (`prefixItems` with fewer `minItems`, or open) checks each item against the
/// type of its schema.
#[test]
fn a_tuple_checks_each_item_against_its_schema() {
    let pair = json!({"type": "array", "prefixItems": [{"type": "string"}, {"type": "integer"}], "items": false});
    let holder = json!({"type": "object", "properties": {"pair": pair}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types = common::file(&common::generate("tuple", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("::honig::InstanceLocation::Index"), "{types}");
}

/// `required` of an object without `properties` is a map that must have those names.
#[test]
fn a_map_checks_the_names_it_requires() {
    let holder = json!({"type": "object", "properties": {"tags": {"type": "object", "required": ["foo"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("map-required", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("::honig::__private::check_required") && types.contains("\"foo\""), "{types}");
}

/// Keywords beside a `oneOf` apply to whichever branch matches: the value also decodes as their type.
#[test]
fn keywords_beside_a_union_hold_for_each_branch() {
    let a = json!({"type": "object", "properties": {"kind": {"const": "a"}, "x": {"type": "integer"}}, "required": ["kind"]});
    let b = json!({"type": "object", "properties": {"kind": {"const": "b"}, "y": {"type": "integer"}}, "required": ["kind"]});
    let item = json!({"type": "object", "maxProperties": 3,
        "oneOf": [{"$ref": "#/components/schemas/A"}, {"$ref": "#/components/schemas/B"}]});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"A": a, "B": b, "Item": item}}));
    let types =
        common::file(&common::generate("beside-union", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains(".decode_ref::<"), "{types}");
}

/// An `allOf` whose members one merged type cannot represent (two `maxLength`) is that type, and the value also decodes
/// as each member.
#[test]
fn an_all_of_that_loses_keywords_checks_each_member() {
    let code = json!({"allOf": [{"type": "string", "maxLength": 8}, {"type": "string", "maxLength": 4}]});
    let holder = json!({"type": "object", "properties": {"code": code}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("lossy-all-of-typed", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains(".decode_ref::<"), "{types}");
}

/// An `enum` or `const` of values that no scalar type represents (numbers with strings, objects) is a type of unit
/// structs that each decode one value: nothing is compared as JSON after decoding.
#[test]
fn values_that_no_scalar_type_holds_are_unit_types() {
    let mixed = json!({"type": "object", "properties": {"size": {"enum": [1, "large", {"w": 2}]}, "unit": {"const": {"si": true}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Sized": mixed}}));
    let types = common::file(&common::generate("untyped-values", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    assert!(
        types.contains("::honig::const_impls!(") && !types.contains("::honig::__private::content_equal"),
        "{types}"
    );
}

/// `unevaluatedProperties` refuses the names that the schema, what it applies in place and the branches that the value
/// matches leave.
#[test]
fn unevaluated_properties_refuses_what_nothing_evaluates() {
    let base = json!({"type": "object", "properties": {"id": {"type": "string"}}});
    let item = json!({"type": "object", "allOf": [{"$ref": "#/components/schemas/Base"}],
        "properties": {"size": {"type": "integer"}},
        "if": {"type": "object", "properties": {"size": {"type": "integer", "minimum": 10}}},
        "then": {"type": "object", "properties": {"label": {"type": "string"}}},
        "unevaluatedProperties": false});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Base": base, "Item": item}}));
    let types = common::file(
        &common::generate("unevaluated-properties-typed", &doc, &crate::emit::Options::default()),
        "types.rs",
    )
    .to_owned();
    assert!(types.contains("is not allowed (unevaluatedProperties)"), "{types}");
}

/// `unevaluatedProperties` beside a union sees the names that the matched branches evaluate: it is not split off.
#[test]
fn unevaluated_properties_beside_a_union_sees_its_branches() {
    let item = json!({"type": "object", "anyOf": [{"type": "object", "properties": {"foo": {"type": "integer"}}}, true],
        "unevaluatedProperties": false});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Item": item}}));
    let types =
        common::file(&common::generate("unevaluated-beside-union", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert_eq!(types.matches("is not allowed (unevaluatedProperties)").count(), 1, "{types}");
    assert!(types.contains("matches!(name, \"foo\")"), "{types}");
}

/// `unevaluatedItems` refuses the items that `contains` and the branches that the value matches leave.
#[test]
fn unevaluated_items_refuses_what_nothing_evaluates() {
    let pair = json!({"type": "array", "contains": {"type": "integer"}, "unevaluatedItems": false});
    let holder = json!({"type": "object", "properties": {"pair": pair}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("unevaluated-items-typed", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is not allowed (unevaluatedItems)"), "{types}");
}

/// What a nested `then` evaluates counts only when the `if` around it matched too: the branch keeps its own
/// alternatives together.
#[test]
fn a_nested_branch_counts_only_inside_its_own() {
    let contains = |v: &str| json!({"type": "array", "contains": {"const": v}});
    let list = json!({"type": "array", "if": contains("a"), "then": {"type": "array", "if": contains("b")},
        "unevaluatedItems": false});
    let holder = json!({"type": "object", "properties": {"list": list}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("nested-branch", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    let flat: String = types.split_whitespace().collect();
    assert!(flat.contains("is not allowed (unevaluatedItems)".replace(' ', "").as_str()), "{types}");
    assert!(flat.contains("&&(item.matches"), "{types}");
}

/// A `$dynamicRef` to an anchor that one resource declares always lands on it, as its type does: no schema is checked
/// whole.
#[test]
fn a_dynamic_ref_to_one_anchor_is_typed() {
    let tree = json!({"$dynamicAnchor": "node", "type": "object",
        "properties": {"name": {"type": "string"}, "children": {"type": "array", "items": {"$dynamicRef": "#node"}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Tree": tree}}));
    let types =
        common::file(&common::generate("dynamic-ref-typed", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("pub children: Vec<Self>,"), "{types}");
}

/// A `$dynamicRef` in a resource with an `$id`, to the one `$dynamicAnchor` of that name, is typed as its target.
#[test]
fn a_dynamic_ref_in_a_resource_with_an_id_is_typed() {
    let list = json!({"$id": "https://example.test/list", "type": "array", "items": {"$dynamicRef": "#items"},
        "$defs": {"item": {"$dynamicAnchor": "items", "type": "string"}}});
    let holder = json!({"type": "object", "required": ["v"], "properties": {"v": list}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types = common::file(&common::generate("dynamic-ref-id", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    assert!(types.contains("pub v: Vec<String>,"), "{types}");
}

/// A schema that refers to a generic and declares its `$dynamicAnchor` at its own root binds the generic's dynamic
/// references to itself: the children of a strict tree are strict trees.
#[test]
fn a_root_dynamic_anchor_binds_the_generic_to_its_site() {
    let tree = json!({"$dynamicAnchor": "node", "type": "object",
        "properties": {"data": {"type": "string"}, "children": {"type": "array", "items": {"$dynamicRef": "#node"}}}});
    let strict = json!({"$dynamicAnchor": "node",
        "$ref": "#/components/schemas/Tree", "maxProperties": 2});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Tree": tree, "StrictTree": strict}}));
    let types =
        common::file(&common::generate("root-anchor", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("#[optional]\n        pub children: Vec<Self>"), "{types}");
}

/// `propertyNames` of any schema decodes each name, as a JSON string, as its type.
#[test]
fn property_names_of_any_schema_decode_each_name() {
    let labels = json!({"type": "object", "additionalProperties": {"type": "string"},
        "propertyNames": {"enum": ["red", "green"]}});
    let holder = json!({"type": "object", "properties": {"labels": labels}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("property-names-any", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is not an allowed property name"), "{types}");
}

/// `dependentSchemas` of a name that the struct does not declare keeps that name and checks the schema when it is
/// present.
#[test]
fn dependent_schemas_of_an_undeclared_name_are_typed() {
    let card = json!({"type": "object", "properties": {"cvc": {"type": "string"}},
        "dependentSchemas": {"card": {"type": "object", "required": ["cvc"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Payment": card}}));
    let types =
        common::file(&common::generate("dependent-undeclared", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is present: "), "{types}");
}

/// A `dependentSchemas` of an undeclared name beside `additionalProperties: false` keeps the struct closed: the name
/// is refused anyway.
#[test]
fn an_undeclared_dependent_name_keeps_a_closed_struct_closed() {
    let item = json!({"type": "object", "properties": {"a": {"type": "string"}},
        "dependentSchemas": {"b": {"required": ["a"]}}, "additionalProperties": false});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Item": item}}));
    let types = common::file(
        &common::generate("dependent-undeclared-closed", &doc, &crate::emit::Options::default()),
        "types.rs",
    )
    .to_owned();
    assert!(types.contains("deny_unknown_fields") && !types.contains("additional_properties"), "{types}");
}

/// `contains`, `patternProperties`, `propertyNames` and `dependentRequired` on a schema without `type` apply to values
/// of their kind only: each is checked on its own.
#[test]
fn keywords_of_one_kind_without_a_type_are_checked() {
    for (keyword, schema, check) in [
        ("contains", json!({"contains": {"minimum": 5}}), "matching item(s)"),
        ("patternProperties", json!({"patternProperties": {"^f": {"type": "integer"}}}), "pattern_matches"),
        ("propertyNames", json!({"propertyNames": {"maxLength": 3}}), "is not an allowed property name"),
        ("dependentRequired", json!({"dependentRequired": {"a": ["b"]}}), "is required when"),
    ] {
        let holder = json!({"type": "object", "required": ["v"], "properties": {"v": schema}});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
        let files = common::generate(&format!("typeless-{keyword}"), &doc, &crate::emit::Options::default());
        let types = common::file(&files, "types.rs");
        assert!(types.contains(check), "{keyword}: {types}");
    }
}

/// An `if` on one property's constant without `type: object` is no tagged union: its value may be of any kind. The
/// generated code checks its `then` and `else`, so a value of the wrong branch is not accepted without a word.
#[test]
fn a_conditional_without_an_object_type_is_checked() {
    let v = json!({"if": {"properties": {"kind": {"const": "n"}}, "required": ["kind"]},
        "then": {"properties": {"x": {"type": "number"}}}, "else": {"properties": {"x": {"type": "string"}}}});
    let holder = json!({"type": "object", "required": ["v"], "properties": {"v": v}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("typeless-conditional", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("`then`: "), "{types}");
}

/// `contains` and `dependentRequired` without `type` beside `properties` or `patternProperties`: the value is the union
/// of an implied shape, and each keyword is still checked on the JSON value.
#[test]
fn keywords_beside_an_implied_shape_are_checked() {
    for (keyword, schema, check) in [
        (
            "contains",
            json!({"properties": {"a": {"type": "integer"}}, "contains": {"multipleOf": 3}}),
            "matching item(s)",
        ),
        (
            "dependentRequired",
            json!({"patternProperties": {"^x": {"type": "integer"}}, "dependentRequired": {"a": ["b"]}}),
            "is required when",
        ),
    ] {
        let holder = json!({"type": "object", "required": ["v"], "properties": {"v": schema}});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
        let files = common::generate(&format!("implied-{keyword}"), &doc, &crate::emit::Options::default());
        let types = common::file(&files, "types.rs");
        assert!(types.contains(check), "{keyword}: {types}");
    }
}

/// `false` refuses any value that is present, and `items`, `prefixItems` or `additionalProperties` without `type` are
/// stored by the union of their implied shape.
#[test]
fn false_and_implied_shapes_are_checked_by_types() {
    let holder = json!({"type": "object", "properties": {
        "list": {"items": {"multipleOf": 2}},
        "pair": {"prefixItems": [{"type": "integer"}]},
        "map": {"additionalProperties": {"type": "integer"}},
        "named": {"patternProperties": {"^a": {}}, "additionalProperties": false},
        "nothing": false,
    }});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types = common::file(&common::generate("false-and-shapes", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    assert!(types.contains("no value is allowed here"), "{types}");
    // The names of the object that `named` accepts match its one pattern.
    let flat: String = types.split_whitespace().collect();
    assert!(flat.contains("::honig::__private::check_pattern(") && flat.contains(r#"Pattern::new("^a",)"#), "{types}");
    // Each item of `list` and each value of `map` decodes as its schema: two item checks besides those of `pair`, and
    // one key check besides the pattern of `named`, whose names its pattern alone judges.
    assert_eq!(types.matches("InstanceLocation::Index").count(), 4, "{types}");
    assert_eq!(types.matches("InstanceLocation::Key").count(), 2, "{types}");
}

/// A `oneOf` or `anyOf` whose branches no union tells apart (a branch without one `type`) counts the branches that the
/// value decodes as: exactly one of `oneOf`, one at least of `anyOf`.
#[test]
fn a_loose_union_counts_its_branches() {
    for (keyword, check) in [("oneOf", "not exactly one"), ("anyOf", "matches none of `anyOf`")] {
        let v = json!({keyword: [{"type": "integer"}, {"minimum": 2}]});
        let holder = json!({"type": "object", "required": ["v"], "properties": {"v": v}});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
        let files = common::generate(&format!("loose-{keyword}"), &doc, &crate::emit::Options::default());
        let types = common::file(&files, "types.rs");
        assert!(types.contains(check), "{keyword}: {types}");
    }
}

/// Keywords of one kind beside an implied shape (`required` beside `properties`, `minItems` beside `items`, no `type`)
/// are checked on the value of the union by its kind.
#[test]
fn keywords_of_one_kind_beside_an_implied_shape_are_checked() {
    for (name, schema, check) in [
        ("required", json!({"properties": {"a": {"type": "integer"}}, "required": ["a"]}), "check_required"),
        ("min-items", json!({"items": {"type": "integer"}, "minItems": 2}), "check_len"),
    ] {
        let holder = json!({"type": "object", "required": ["v"], "properties": {"v": schema}});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
        let files = common::generate(&format!("implied-{name}"), &doc, &crate::emit::Options::default());
        let types = common::file(&files, "types.rs");
        assert!(types.contains(check), "{name}: {types}");
    }
}

/// A `$ref` with keywords beside it is the `allOf` of the two: the merged type enforces them, and a member that it
/// cannot enforce is checked on its own.
#[test]
fn keywords_beside_a_ref_are_checked() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {
            "Str": {"type": "string"},
            "Obj": {"type": "object", "properties": {"a": {"type": "integer"}}},
            "Holder": {"type": "object", "required": ["s", "o", "n"], "properties": {
                "s": {"$ref": "#/components/schemas/Str", "maxLength": 3},
                "o": {"$ref": "#/components/schemas/Obj", "properties": {"b": {"type": "string"}}, "required": ["b"]},
                "n": {"$ref": "#/components/schemas/Obj", "not": {"required": ["a"]}},
            }},
        }}),
    );
    let types =
        common::file(&common::generate("ref-siblings", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("check_len") && types.contains("matches `not`"), "{types}");
    assert!(types.contains("pub b: String"), "{types}");
}

/// Draft 7 `dependencies` is split as 2019-09 did: a list of names is `dependentRequired`, a schema `dependentSchemas`.
/// Both are checked by generated code.
#[test]
fn draft_7_dependencies_are_split() {
    let card = json!({"type": "object", "properties": {"card": {"type": "string"}, "cvc": {"type": "string"}},
        "dependencies": {"card": ["cvc"], "cvc": {"required": ["card"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Card": card}}));
    let types =
        common::file(&common::generate("dependencies", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("is required when") && types.contains("is present: "), "{types}");
}

/// A property that `dependentRequired` uses keeps its absence: its `default` is not filled in, so the check sees
/// whether it was sent.
#[test]
fn a_dependent_name_with_a_default_keeps_its_absence() {
    let card = json!({"type": "object", "properties": {"card": {"type": "string"},
        "cvc": {"type": "string", "default": "000"}}, "dependentRequired": {"card": ["cvc"]}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Card": card}}));
    let types =
        common::file(&common::generate("dependent-default", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("#[optional]\n        pub cvc: String") && types.contains("is required when"), "{types}");
}

/// A resource that declares a `$dynamicAnchor` and reaches a generic through a plain `$ref` binds it: the outermost
/// resource in the dynamic scope wins (JSON Schema 2020-12 section 8.2.3.2). The list's items are its strings.
#[test]
fn a_resource_binds_the_generic_that_it_reaches() {
    let root = json!({"$id": "https://example.test/root", "type": "object", "required": ["list"],
    "properties": {"list": {"$ref": "list"}},
    "$defs": {
        "item": {"$dynamicAnchor": "item", "type": "string"},
        "list": {"$id": "list", "type": "array", "items": {"$dynamicRef": "#item"},
            "$defs": {"item": {"$dynamicAnchor": "item"}}},
    }});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Root": root}}));
    let types =
        common::file(&common::generate("dynamic-scope", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("pub list: Vec<String>"), "{types}");
}

/// A resource nested in a binding resource is not in its dynamic scope when it is reached directly: its `$dynamicRef`
/// keeps its own anchor. Only the copy that the binding resource reaches by reference is bound.
#[test]
fn a_nested_resource_reached_directly_keeps_its_anchor() {
    let root = json!({"$id": "https://example.test/main", "type": "object", "required": ["item"],
    "properties": {"item": {"$ref": "item"}},
    "$defs": {"bar": {"$id": "bar", "type": "array", "items": {"$ref": "item"},
        "$defs": {
            "item": {"$id": "item", "type": "object", "properties": {"content": {"$dynamicRef": "#content"}},
                "$defs": {"default": {"$dynamicAnchor": "content", "type": "integer"}}},
            "content": {"$dynamicAnchor": "content", "type": "string"},
        }}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Main": root}}));
    let types = common::file(&common::generate("nested-resource", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    assert!(types.contains("#[optional]\n        pub content: i64"), "{types}");
}

/// `dependentRequired` of a name that the struct does not declare: the struct keeps that name, and the rule is checked
/// on the JSON object.
#[test]
fn an_undeclared_dependent_required_name_is_checked() {
    let card = json!({"type": "object", "properties": {"card": {"type": "string"}},
        "dependentRequired": {"card": ["cvc"]}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Card": card}}));
    let types = common::file(
        &common::generate("dependent-required-undeclared", &doc, &crate::emit::Options::default()),
        "types.rs",
    )
    .to_owned();
    assert!(types.contains("`cvc` is required when `card` is present"), "{types}");
}

/// `items` and `additionalProperties` see only their own schema's `prefixItems` and `properties`, not those of an
/// `allOf` member. Without `type` both are checked on the JSON value, and each member is decoded as its own type.
#[test]
fn keywords_beside_an_all_of_see_only_their_own_schema() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {
            "List": {"allOf": [{"prefixItems": [{"minimum": 3}]}], "items": {"minimum": 5}},
            "Bag": {"allOf": [{"properties": {"foo": {}}}], "additionalProperties": {"type": "boolean"}},
            "Holder": {"type": "object", "properties": {
                "list": {"$ref": "#/components/schemas/List"},
                "bag": {"$ref": "#/components/schemas/Bag"},
            }},
        }}),
    );
    let types =
        common::file(&common::generate("all-of-own", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains(".skip(0)") && types.contains("InstanceLocation::Key"), "{types}");
}

/// `unevaluatedProperties` sees the names that a `dependentSchemas` schema evaluates when its name is present.
#[test]
fn unevaluated_properties_see_dependent_schemas() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Dep": {"type": "object", "properties": {"foo": {"type": "string"}},
            "dependentSchemas": {"foo": {"properties": {"bar": {"const": "bar"}}, "required": ["bar"]}},
            "unevaluatedProperties": false},
            "Holder": {"type": "object", "properties": {"dep": {"$ref": "#/components/schemas/Dep"}}}}}),
    );
    let types =
        common::file(&common::generate("dependent-unevaluated", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is present: ") && types.contains("(matched_0 && (matches!(name, \"bar\")))"), "{types}");
}

/// Checks that decode the value as a schema's type (`not`, `oneOf`, `if`) use one buffer of it, with strings borrowed:
/// none clones the value.
#[test]
fn subschema_checks_read_one_buffer() {
    let item = json!({"type": "object", "properties": {"a": {"type": "string"}}, "not": {"required": ["b"]},
        "if": {"required": ["a"]}, "then": {"properties": {"a": {"minLength": 2}}}});
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Item": item, "Holder": {"type": "object", "properties": {
            "item": {"$ref": "#/components/schemas/Item"}}}}}),
    );
    let types =
        common::file(&common::generate("one-buffer", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert_eq!(types.matches("::honig::checked_json!(").count(), 1, "{types}");
    assert!(!types.contains("::honig::__private::to_value(") && !types.contains(".clone())"), "{types}");
}

/// `contains` on a typed list counts the items of the buffer of the list: no item is written out as JSON again.
#[test]
fn typed_contains_reads_the_buffer() {
    let holder = json!({"type": "object", "properties": {"parcels": {"type": "array",
        "items": {"type": "object", "properties": {"fragile": {"type": "boolean"}}},
        "contains": {"properties": {"fragile": {"const": true}}, "required": ["fragile"]}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types = common::file(&common::generate("typed-contains", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    assert!(types.contains("item.matches::<") && !types.contains("::honig::__private::to_value(item)"), "{types}");
}

/// A closed `prefixItems` tuple is a Rust tuple, which enforces its items by their types: a field of it, or of a list
/// of it, needs no wrapper that checks the items again.
#[test]
fn a_rust_tuple_is_checked_once() {
    let point = json!({"type": "array", "prefixItems": [{"type": "number"}, {"type": "string"}], "items": false,
        "minItems": 2});
    let route = json!({"type": "object", "required": ["start", "stops"], "properties": {
        "start": {"$ref": "#/components/schemas/Point"},
        "stops": {"type": "array", "items": {"$ref": "#/components/schemas/Point"}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Point": point, "Route": route}}));
    let types =
        common::file(&common::generate("tuple-once", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("pub start: Point,") && types.contains("pub stops: Vec<Point>,"), "{types}");
}

/// A resource that `$ref`s a generic beside its own `properties` keeps both: the generic's become a member of `allOf`.
/// The generic's original in `$defs`, reached by nothing once bound, is no `$dynamicRef` that varies.
#[test]
fn a_bound_generic_keeps_the_properties_of_its_site() {
    let v = json!({"$id": "https://t.test/root", "$dynamicAnchor": "meta", "type": "object",
        "properties": {"foo": {"const": "pass"}}, "$ref": "extended",
        "$defs": {"extended": {"$id": "extended", "$dynamicAnchor": "meta", "type": "object",
            "properties": {"bar": {"$ref": "bar"}}},
            "bar": {"$id": "bar", "type": "object", "properties": {"baz": {"$dynamicRef": "extended#meta"}}}}});
    let doc =
        common::spec("3.2.0", &json!({}), &json!({"schemas": {"Case": {"type": "object", "properties": {"v": v}}}}));
    let spec = common::write("bound-generic-site", &doc, &[]);
    let generated =
        crate::generate::generate(&spec, &crate::emit::Options::default()).unwrap_or_else(|d| panic!("{d:?}"));
    let errors: Vec<_> = generated.diagnostics.0.iter().filter(|d| d.level == crate::diag::Level::Error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let types = common::file(&generated.files, "types.rs");
    assert!(types.contains("pub foo:") && types.contains("pub bar:"), "{types}");
}

/// A `const` of an object is a unit struct that decodes only that object: no check compares a decoded value as JSON.
#[test]
fn an_object_const_is_a_unit_struct() {
    let origin = json!({"const": {"x": 0, "y": 0}});
    let holder = json!({"type": "object", "properties": {"origin": {"$ref": "#/components/schemas/Origin"}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Origin": origin, "Holder": holder}}));
    let types =
        common::file(&common::generate("object-const", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("pub struct Origin;") && !types.contains("content_equal"), "{types}");
}

/// A schema nested deeper than one pass types is a named type filled later: the innermost keyword is still checked.
#[test]
fn a_deeply_nested_schema_is_typed() {
    let mut schema = json!({"type": "string", "maxLength": 2});
    for _ in 0..60 {
        schema = json!({"type": "object", "properties": {"a": schema}});
    }
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Deep": schema}}));
    let spec = common::write("deep", &doc, &[]);
    let generated =
        crate::generate::generate(&spec, &crate::emit::Options::default()).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(generated.diagnostics.0.is_empty(), "{:?}", generated.diagnostics);
    let types = common::file(&generated.files, "types.rs");
    assert!(types.contains("Some(2)") && !types.contains("serde_json::Value"), "{types}");
}

/// A site binds its anchor in every generic that it reaches, however long the chain: the 20th still resolves to the
/// site's type, not the generic's own default.
#[test]
fn a_long_chain_of_generics_binds_each() {
    let n = 20;
    let mut schemas = serde_json::Map::new();
    for i in 0..n {
        let next =
            if i + 1 < n { json!({"$ref": format!("https://t.test/g{}", i + 1)}) } else { json!({"type": "string"}) };
        let node = json!({"$dynamicAnchor": "node", "type": "integer"});
        schemas.insert(
            format!("G{i}"),
            json!({"$id": format!("https://t.test/g{i}"), "$defs": {"node": node}, "type": "object",
                "properties": {"me": {"$dynamicRef": "#node"}, "next": next}}),
        );
    }
    let node = json!({"$dynamicAnchor": "node", "type": "boolean"});
    schemas.insert("Site".into(), json!({"$ref": "#/components/schemas/G0", "$defs": {"node": node}}));
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": schemas}));
    let types =
        common::file(&common::generate("generic-chain", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    let last = types.split("pub struct SiteG19 {").nth(1).unwrap_or_else(|| panic!("{types}"));
    assert!(last.split('}').next().is_some_and(|body| body.contains("#[optional]\n        pub me: bool,")), "{types}");
}

/// A client response arm checks the media type of what it received whole in one call on its head and body.
#[test]
fn a_response_arm_checks_its_media_in_one_call() {
    let files = wide_binary("shape-media-of", true);
    let client = common::file(&files, "client.rs").split_whitespace().collect::<String>();
    assert!(
        client
            .contains(r#"::honig::__private::check_media_of(&head,&body,&["application/json","application/xml"],)?;"#),
        "{client}"
    );
    assert!(!client.contains("Some(&body)"), "{client}");
}

/// `*/*` is the least specific key: a response checks `image/*` first and uses `*/*` for anything else, whatever the
/// order of the description.
#[test]
fn any_media_type_is_tried_last() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/a": {"get": {"operationId": "a", "responses": {"200": {"description": "ok", "content": {
            "*/*": {"schema": {"type": "string"}},
            "image/*": {"schema": {"type": "string", "format": "binary"}},
        }}}}}}),
        &json!({}),
    );
    let files = common::generate("shape-any-media-last", &doc, &crate::emit::Options::default());
    let client = common::file(&files, "client.rs");
    assert!(client.contains(r#"::honig::__private::media_matches(ct, "image/*")"#), "{client}");
    assert!(!client.contains(r#"::honig::__private::media_matches(ct, "*/*")"#), "{client}");
    assert!(client.contains("Any("), "{client}");
}

/// A part's own `encoding`, `prefixEncoding` and `itemEncoding` nest three deep at most: a deeper part keeps no media
/// type of its own.
#[test]
fn a_part_encoding_nests_three_deep_at_most() {
    for seg in ["p", "0", "*"] {
        let mut enc = json!({"contentType": "text/csv"});
        for _ in 0..4 {
            enc = match seg {
                "p" => json!({"encoding": {"p": enc}}),
                "0" => json!({"prefixEncoding": [enc]}),
                _ => json!({"itemEncoding": enc}),
            };
            enc["contentType"] = json!("multipart/mixed");
        }
        let body = json!({"content": {"multipart/form-data": {
            "schema": {"type": "object", "properties": {"f": {}}},
            "encoding": {"f": enc},
        }}});
        let doc = common::spec(
            "3.2.0",
            &json!({"/a": {"post": {"operationId": "a", "requestBody": body, "responses": {"204": {"description": "ok"}}}}}),
            &json!({}),
        );
        let files = common::generate("shape-encoding-depth", &doc, &crate::emit::Options::default());
        let all: String = files.iter().map(|(_, t)| t.as_str()).collect();
        assert!(all.contains(&format!("\"f/{seg}/{seg}/{seg}\"")), "{all}");
        assert!(!all.contains(&format!("\"f/{seg}/{seg}/{seg}/{seg}\"")), "{all}");
    }
}

/// Binary data in JSON is base64: a field of it turns on honig's `base64`, as does a list of it.
#[test]
fn binary_data_turns_on_base64() {
    for field in [
        json!({"type": "string", "format": "byte"}),
        json!({"type": "array", "items": {"type": "string", "format": "byte"}}),
    ] {
        let doc = common::spec(
            "3.1.0",
            &json!({}),
            &json!({"schemas": {"Blob": {"type": "object", "properties": {"data": field}}}}),
        );
        let g = crate::generate::generate(&common::write("shape-base64", &doc, &[]), &crate::emit::Options::default())
            .expect("generates");
        assert!(g.runtime_features.contains(&crate::settings::BASE64), "{field}: {:?}", g.runtime_features);
    }
}

/// Two unwrapped XML lists whose items share an element name write their items side by side, and neither reads back
/// its own: a warning at the second. One alone, or one wrapped, needs none.
#[test]
fn xml_lists_of_one_item_name_are_a_warning() {
    let list = |wrapped: bool| json!({"type": "array", "xml": {"wrapped": wrapped}, "items": {"type": "string", "xml": {"name": "item"}}});
    let made = |props: serde_json::Value| {
        let doc = common::spec(
            "3.1.0",
            &json!({"/b": {"get": {"operationId": "b", "responses": {"200": {"description": "ok",
                "content": {"application/xml": {"schema": {"$ref": "#/components/schemas/Bag"}}}}}}}}),
            &json!({"schemas": {"Bag": {"type": "object", "properties": props}}}),
        );
        crate::generate::generate(&common::write("shape-xml-wrap", &doc, &[]), &crate::emit::Options::default())
    };
    let warned = |props: serde_json::Value| {
        let made = made(props).expect("a valid description generates");
        made.diagnostics.iter().any(|d| d.pointer == "/components/schemas/Bag/properties/b")
    };
    assert!(warned(json!({"a": list(false), "b": list(false)})), "one item name");
    assert!(!warned(json!({"a": list(false)})), "one list");
    assert!(!warned(json!({"a": list(false), "b": list(true)})), "one wrapped");
}

/// `Content-Type`, `Accept` and `Authorization` headers belong to honig: such a header parameter is left out. A
/// query parameter of that name is the description's own.
#[test]
fn only_header_parameters_of_runtime_names_are_left_out() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/a": {"get": {"operationId": "a", "parameters": [
            {"name": "Authorization", "in": "header", "schema": {"type": "string"}},
            {"name": "accept", "in": "query", "schema": {"type": "string"}},
        ], "responses": {"204": {"description": "ok"}}}}}),
        &json!({}),
    );
    let ops = common::file(
        &common::generate("shape-runtime-headers", &doc, &crate::emit::Options::default()),
        "operations.rs",
    )
    .to_owned();
    assert!(ops.contains("pub accept: Option<String>") && !ops.contains("pub authorization"), "{ops}");
}

/// A response's `summary` leads its doc and its `description` follows, once: the same text is not repeated.
#[test]
fn a_response_summary_and_description_of_one_text_show_once() {
    let doc = common::spec(
        "3.2.0",
        &json!({"/a": {"get": {"operationId": "a", "responses": {"200": {"summary": "Found it", "description": "Found it"}}}}}),
        &json!({}),
    );
    let ops =
        common::file(&common::generate("shape-response-doc", &doc, &crate::emit::Options::default()), "operations.rs")
            .to_owned();
    assert_eq!(ops.matches("Found it").count(), 1, "{ops}");
}

/// Extra derives from the settings go on every type (`*`) or on one by its Rust name, once each, and never a derive
/// that honig writes itself.
#[test]
fn extra_derives_go_where_the_settings_say() {
    let obj = json!({"type": "object", "properties": {"a": {"type": "string"}}});
    let doc = common::spec("3.1.0", &json!({}), &json!({"schemas": {"Item": obj, "Other": obj}}));
    let derives = |list: Vec<(&str, Vec<&str>)>| {
        let mut opts = crate::emit::Options::default();
        opts.rust.derives =
            list.into_iter().map(|(k, v)| (k.to_owned(), v.into_iter().map(str::to_owned).collect())).collect();
        let types = common::file(&common::generate("shape-derives", &doc, &opts), "types.rs").to_owned();
        let of = |n: &str| {
            types
                .split("::honig::object!")
                .find(|b| b.contains(&format!("pub struct {n} {{")))
                .unwrap_or_default()
                .to_owned()
        };
        (of("Item"), of("Other"))
    };
    let (item, other) = derives(vec![("Item", vec!["x::Marker", "x::Marker", "Clone"])]);
    assert_eq!((item.matches("x::Marker").count(), other.contains("x::Marker")), (1, false), "{item} {other}");
    let (item, other) = derives(vec![("*", vec!["x::Marker"])]);
    assert!(item.contains("x::Marker") && other.contains("x::Marker"), "{item} {other}");
}

/// A deprecated object type is deprecated as a struct: the attribute sits on the struct inside its macro.
#[test]
fn a_deprecated_object_type_is_a_deprecated_struct() {
    let obj = json!({"type": "object", "deprecated": true, "properties": {"a": {"type": "string"}}});
    let doc = common::spec("3.1.0", &json!({}), &json!({"schemas": {"Old": obj}}));
    let types =
        common::file(&common::generate("shape-deprecated-object", &doc, &crate::emit::Options::default()), "types.rs")
            .split_whitespace()
            .collect::<String>();
    assert!(types.contains("#[deprecated]pubstructOld{"), "{types}");
}

/// A multipart part whose schema gives a `contentMediaType` is sent as that type when `encoding` gives none.
#[test]
fn a_part_takes_the_media_type_of_its_schema() {
    let paths = json!({"/upload": {"post": {
        "requestBody": {"content": {"multipart/form-data": {"schema": {"type": "object", "properties": {
            "icon": {"type": "string", "contentMediaType": "image/png"}}}}}},
        "responses": {"204": {"description": "ok"}}
    }}});
    let doc = common::spec("3.2.0", &paths, &json!({}));
    let files = common::generate("part-media", &doc, &crate::emit::Options::default());
    let client = common::file(&files, "client.rs");
    assert!(client.contains(r#"("icon", "image/png")"#), "{client}");
}

/// `minProperties` and `maxProperties` on a struct count the declared properties that are present and the names that
/// it does not declare.
#[test]
fn a_struct_counts_its_properties() {
    let s = json!({"type": "object", "properties": {"a": {"type": "string", "default": "x"}}, "maxProperties": 1});
    let holder = json!({"type": "object", "properties": {"s": {"$ref": "#/components/schemas/S"}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"S": s, "Holder": holder}}));
    let types =
        common::file(&common::generate("struct-count", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    let flat: String = types.split_whitespace().collect();
    assert!(flat.contains("usize::from(value.a.is_some())") && flat.contains("\"properties\""), "{types}");
}

/// A list component with keywords that `Vec` does not enforce is a checked type of it, and a field of it needs no
/// wrapper.
#[test]
fn a_list_component_with_keywords_is_a_checked_type() {
    let tags = json!({"type": "array", "items": {"type": "string"}, "maxItems": 3});
    let holder = json!({"type": "object", "properties": {"tags": {"$ref": "#/components/schemas/Tags"}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Tags": tags, "Holder": holder}}));
    let types =
        common::file(&common::generate("checked-alias", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("pub struct Tags(pub(super) Vec<String>);"), "{types}");
    assert!(types.contains("#[optional]\n        pub tags: Tags,"), "{types}");
}

/// A `default` that breaks a keyword of its schema fails generation: a field that the sender leaves out would decode as
/// a value that no sender could have written, and `{maximum: 100, default: 500}` would write `PageLimit { 0: 500 }`.
#[test]
fn a_default_that_breaks_its_schema_fails_generation() {
    let holder = |property: serde_json::Value| json!({"type": "object", "properties": {"label": property}});
    for (i, property) in [
        json!({"type": "string", "minLength": 4, "default": "bad"}),
        json!({"type": "integer", "maximum": 100, "default": 500}),
        json!({"type": "string", "enum": ["a", "b"], "default": "c"}),
        json!({"type": "integer", "default": "five"}),
    ]
    .into_iter()
    .enumerate()
    {
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder(property)}}));
        let path = common::write(&format!("default-breaks-{i}"), &doc, &[]);
        let err = crate::generate::generate(&path, &crate::emit::Options::default()).expect_err("must fail");
        assert!(err.iter().any(|d| d.pointer == "/components/schemas/Holder/properties/label/default"), "{:#?}", err.0);
    }
}

/// The generated test checks every documented example, however many, each decoded from its JSON text straight into its
/// type: no untyped JSON value on the way.
#[test]
fn every_example_is_decoded_from_its_text() {
    let mut properties = serde_json::Map::new();
    for i in 0..250 {
        properties.insert(format!("p{i}"), json!({"type": "integer", "example": i}));
    }
    let doc =
        common::spec("3.2.0", &json!({}), &json!({"schemas": {"Many": {"type": "object", "properties": properties}}}));
    let opts = crate::emit::Options { tests: true, ..crate::emit::Options::default() };
    let files = common::generate("many-examples", &doc, &opts);
    let examples = common::file(&files, "documented_examples.rs");
    assert_eq!(examples.matches("/example\"").count(), 250, "{examples}");
    assert!(examples.contains("serde_json::from_str") && !examples.contains("serde_json::Value"), "{examples}");
}

/// The request copy of a type with `readOnly` fields lacks them: the examples, which may contain them, are checked
/// against the type that decodes responses only.
#[test]
fn a_request_copy_takes_no_examples() {
    let s =
        json!({"type": "object", "properties": {"id": {"type": "string", "readOnly": true}}, "example": {"id": "x"}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"S": s}}));
    let opts = crate::emit::Options { tests: true, ..crate::emit::Options::default() };
    let files = common::generate("request-copy-examples", &doc, &opts);
    assert!(common::file(&files, "types.rs").contains("pub struct SRequest"));
    let examples = common::file(&files, "documented_examples.rs");
    assert_eq!(examples.matches("/components/schemas/S/example\"").count(), 1, "{examples}");
}

/// One implementation of JSON Schema: the generated types. `generate` judges no example; the generated test decodes
/// each through its type, which refuses one that breaks its schema.
#[test]
fn examples_are_judged_by_the_generated_types_only() {
    let s = json!({"type": "object", "properties": {"n": {"type": "integer", "minimum": 5, "example": 1}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"S": s}}));
    let spec = common::write("examples-judged", &doc, &[]);
    let opts = crate::emit::Options { tests: true, ..crate::emit::Options::default() };
    let generated = crate::generate::generate(&spec, &opts).unwrap_or_else(|d| panic!("{d:?}"));
    assert!(generated.diagnostics.0.is_empty(), "{:?}", generated.diagnostics);
    let examples = common::file(&generated.files, "documented_examples.rs");
    assert!(examples.contains("/properties/n/example"), "{examples}");
}

/// Keywords beside an implied shape check the JSON text that the value decoded from: the value is not written out
/// again.
#[test]
fn implied_keywords_read_the_decoded_text() {
    let s = json!({"properties": {"name": {"type": "string"}}, "required": ["name"], "minLength": 2});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"S": s}}));
    let types =
        common::file(&common::generate("implied-text", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains(", once);") && types.contains("::honig::__private::Content::Str(text)"), "{types}");
    assert!(!types.contains("to_value(&value).unwrap_or_default()"), "{types}");
}

/// A checked type whose checks see the value as JSON parses JSON text once: the value decodes from it and the checks
/// use it, with no value written out again.
#[test]
fn a_checked_type_reads_json_text_once() {
    let item = json!({"type": "object", "properties": {"a": {"type": "string"}}, "not": {"required": ["b"]}});
    let holder = json!({"type": "object", "properties": {"item": {"$ref": "#/components/schemas/Item"}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Item": item, "Holder": holder}}));
    let types =
        common::file(&common::generate("read-once", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("::honig::checked_deserialize!(") && types.contains(", once);"), "{types}");
}

/// The impls that every enum and checked newtype shares come from honig: a module calls them and contains no
/// copy.
#[test]
fn shared_impls_are_runtime_macros_and_not_copies() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {
            "Kind": {"type": "string", "enum": ["a", "b"]},
            "Level": {"type": "integer", "enum": [1, 2]},
            "Code": {"type": "string", "pattern": "^[A-Z]+$"},
            "Count": {"type": "integer", "minimum": 1}
        }}),
    );
    let files = common::generate("shared-impls", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    assert!(!types.contains("macro_rules!"), "{types}");
    for call in [
        r#"::honig::str_enum_impls!(Kind { A => "a", B => "b" })"#,
        "::honig::int_enum_impls!(Level)",
        "::honig::checked_str_impls!(Code)",
    ] {
        assert!(types.contains(call), "{call} in {types}");
    }
    assert!(types.contains("::honig::checked_num_impls!(Count, i64)"), "{types}");
}

/// An inferred tag is a property that every branch requires: a branch that leaves it out also accepts a value without
/// it, which no tag picks.
#[test]
fn an_inferred_tag_is_a_required_property() {
    let tagged = |required_kind: bool| {
        let kind: Vec<&str> = if required_kind { vec!["kind"] } else { vec![] };
        let b_required: Vec<&str> = kind.iter().copied().chain(["n"]).collect();
        let pick = json!({"anyOf": [
            {"type": "object", "required": kind, "properties": {"kind": {"const": "a"}}},
            {"type": "object", "required": b_required, "properties": {"kind": {"const": "b"}, "n": {"type": "integer"}}}
        ]});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Pick": pick}}));
        let types = common::file(&common::generate("inferred-tag", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
        types.contains("::honig::tagged_deserialize!(Pick")
    };
    assert!(tagged(true), "a required `kind` tags the union");
    assert!(!tagged(false), "a `kind` that may be absent does not");
}

/// A checked type with the base and keywords of an earlier one lets that one check: its `TryFrom` delegates. The impls
/// of a checked type, a JSON text value and a tagged union come from runtime macros.
#[test]
fn twins_delegate_and_shared_impls_are_runtime_macros() {
    let code = json!({"type": "string", "pattern": "^[A-Z]+$"});
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {
            "Code": code, "Other": code,
            "Pick": {"oneOf": [{"$ref": "#/components/schemas/A"}, {"$ref": "#/components/schemas/B"}],
                "discriminator": {"propertyName": "k"}},
            "A": {"type": "object", "required": ["a"], "properties": {"k": {"type": "string"}, "a": {"type": "string"}}},
            "B": {"type": "object", "required": ["b"], "properties": {"k": {"type": "string"}, "b": {"type": "integer"}}},
            "Text": {"type": "string", "contentMediaType": "application/json", "contentSchema": {"type": "integer"}}
        }}),
    );
    let types = common::file(&common::generate("twins", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(!types.contains("macro_rules!"), "{types}");
    assert!(types.contains("::honig::checked_twin!(Other, Code, String)"), "{types}");
    assert!(types.contains("::honig::tagged_deserialize!(PickVariants)"), "{types}");
    assert!(types.contains("::honig::json_text_impls!(Text)"), "{types}");
}

/// Two constants and two operations with the same responses share their impls through runtime macros: no file
/// writes a macro of its own for them.
#[test]
fn constants_and_responses_share_runtime_macros() {
    let op = |id: &str| json!({"get": {"operationId": id, "responses": {"204": {"description": "done"}}}});
    let doc = common::spec(
        "3.2.0",
        &json!({"/a": op("a"), "/b": op("b")}),
        &json!({"schemas": {
            "A": {"const": "a"}, "B": {"const": "b"},
            "C": {"const": {"c": 1}}, "D": {"const": [1]}
        }}),
    );
    let files = common::generate("shared-consts", &doc, &crate::emit::Options::default());
    let (types, ops) = (common::file(&files, "types.rs"), common::file(&files, "operations.rs"));
    assert!(!types.contains("macro_rules!") && !ops.contains("macro_rules!"), "{types}{ops}");
    for call in ["::honig::const_impls!(A, VALUE)", "::honig::const_impls!(C)"] {
        assert!(types.contains(call), "{call} in {types}");
    }
    assert!(ops.contains("::honig::response_error!(Response") && ops.contains("::honig::default_from_new!(Params)"));
}

/// An object type is one `::honig::object!` that prints as a struct: a marker picks the kind of a field, and no serde
/// attribute that its kind implies is written out.
#[test]
fn an_object_is_one_runtime_macro_with_a_field_per_line() {
    let doc = common::spec(
        "3.2.0",
        &json!({}),
        &json!({"schemas": {"Item": {
            "type": "object",
            "required": ["name"],
            "properties": {"name": {"type": "string"}, "count": {"type": "integer"}, "owner": {"type": ["string", "null"]}}
        }}}),
    );
    let files = common::generate("object-macro", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    assert!(types.contains("::honig::object! {"), "{types}");
    assert!(!types.contains("remote = \"Self\"") && !types.contains("::honig::__private::not_null"), "{types}");
    let fields = "        pub name: String,\n        #[optional]\n        pub count: i64,\n        #[nullable]\n        pub owner: String,\n";
    assert!(types.contains(fields), "one field per line: {types}");
}

/// Names that collide in Rust (`userId`, `user_id`) are numbered by their sorted wire names, not by where they stand.
/// So after a reorder of properties, enum values, parameters or headers each wire name keeps its Rust name. The
/// declarations keep the order of the description.
#[test]
fn a_reorder_of_colliding_names_changes_nothing() {
    let generate = |flip: bool| {
        let pair = |a: serde_json::Value, b: serde_json::Value| if flip { json!([b, a]) } else { json!([a, b]) };
        let props = |a: &str, b: &str| {
            let mut m = serde_json::Map::new();
            for k in if flip { [b, a] } else { [a, b] } {
                m.insert(k.to_owned(), json!({"type": "string"}));
            }
            serde_json::Value::Object(m)
        };
        let param = |name: &str| json!({"name": name, "in": "query", "schema": {"type": "string"}});
        let header = |a: &str, b: &str| {
            let mut m = serde_json::Map::new();
            for k in if flip { [b, a] } else { [a, b] } {
                m.insert(k.to_owned(), json!({"schema": {"type": "string"}}));
            }
            serde_json::Value::Object(m)
        };
        let doc = common::spec(
            "3.1.0",
            &json!({"/u": {"get": {
                "operationId": "getUser",
                "parameters": pair(param("userId"), param("user_id")),
                "responses": {"200": {"description": "ok", "headers": header("X-Rate", "x_rate"),
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/User"}}}}},
            }}}),
            &json!({"schemas": {
                "User": {"type": "object", "properties": props("userId", "user_id")},
                "Kind": {"type": "string", "enum": pair(json!("a-b"), json!("a_b"))},
            }}),
        );
        common::generate(if flip { "reorder-b" } else { "reorder-a" }, &doc, &crate::emit::Options::default())
    };
    // Every wire name with the Rust name that stores it: a `rename` and its field, a match arm, a query encoded from a
    // field, and a header or query decoded into or encoded from the field named just before it. Builder entries
    // (`field(setter) : Type = "wire"`) print wrapped and refer to the same fields as the struct.
    let pairs = |text: &str| {
        let mut out = Vec::new();
        let (mut field, mut renamed, mut wrapped) = ("", None::<&str>, false);
        for l in text.lines().map(str::trim) {
            if let Some(r) = renamed.take() {
                out.push(format!("{r} {l}"));
            }
            if let Some((name, _)) = l.split_once(": ::honig::") {
                field = name;
            }
            if let Some(name) = l.strip_prefix("if let Some(h) = ::honig::__private::encode_header(&") {
                field = name;
            }
            // A long call prints its arguments one per line: the field is the first.
            if wrapped {
                field = l.trim_start_matches('&').trim_end_matches(',');
            }
            wrapped = l.ends_with("encode_header(");
            if l.contains("rename =") {
                renamed = Some(l);
            } else if (l.contains("=>") || l.contains("encode_query(")) && l.contains('"') && !l.contains(" : ") {
                out.push(l.to_owned());
            } else if ["query_text(", "joined_header(", "set_header("].iter().any(|c| l.contains(c)) {
                out.push(format!("{field} {l}"));
            }
        }
        out.sort();
        out
    };
    let (a, b) = (generate(false), generate(true));
    let types = common::file(&a, "types.rs");
    assert!(types.contains("user_id2") && types.contains("AB2"), "collisions to number: {types}");
    let found: Vec<String> = a.iter().flat_map(|(_, t)| pairs(t)).collect();
    for wire in ["\"userId\"", "\"user_id\"", "\"X-Rate\"", "\"x_rate\""] {
        assert!(found.iter().filter(|l| l.contains(wire)).count() >= 2, "{wire}: {found:#?}");
    }
    for (file, text) in &a {
        let (x, y) = (pairs(text), b.iter().find(|(f, _)| f == file).map(|(_, t)| pairs(t)).unwrap_or_default());
        assert_eq!(x, y, "{file}");
    }
}

/// Two required values of the same type could swap in a positional `new`: such a type or `Params` has only its builder,
/// which accepts them by name. One whose required values all differ in type keeps `new`.
#[test]
fn new_exists_only_where_no_two_required_values_can_swap() {
    let doc = common::spec(
        "3.2.0",
        &json!({
            "/orgs/{org}/repos/{repo}": {"get": {"operationId": "getRepo",
                "parameters": [
                    {"name": "org", "in": "path", "required": true, "schema": {"type": "string"}},
                    {"name": "repo", "in": "path", "required": true, "schema": {"type": "string"}}
                ],
                "responses": {"204": {"description": "ok"}}}},
            "/orders/{id}": {"get": {"operationId": "getOrder",
                "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}}],
                "responses": {"204": {"description": "ok"}}}}
        }),
        &json!({"schemas": {
            "Person": {"type": "object", "required": ["first_name", "last_name"],
                "properties": {"first_name": {"type": "string"}, "last_name": {"type": "string"}}},
            "Point": {"type": "object", "required": ["x", "label"],
                "properties": {"x": {"type": "number"}, "label": {"type": "string"}}}
        }}),
    );
    let files = common::generate("new-or-builder", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    let ops = common::file(&files, "operations.rs");
    assert!(!types.contains("impl Person {"), "no positional new for two strings: {types}");
    assert!(types.contains("impl Point {"), "a number and a string cannot swap: {types}");
    assert!(types.contains("Person => super::Person"), "Person has its builder: {types}");
    assert!(!op_module(ops, "get_repo").contains("fn new("), "two string path parameters: {ops}");
    assert!(op_module(ops, "get_order").contains("fn new("), "one path parameter: {ops}");
}

/// `anyOf` has no order: a value that several branches accept decodes as the same one whatever their order.
#[test]
fn a_reorder_of_any_of_branches_decodes_the_same() {
    let tries = |flip: bool| {
        let (a, b) = (json!({"$ref": "#/components/schemas/A"}), json!({"type": "string", "format": "uuid"}));
        let (c, d) = (json!({"$ref": "#/components/schemas/B"}), json!({"type": "string"}));
        let any = if flip { json!([d, c, b, a]) } else { json!([a, b, c, d]) };
        let doc = common::spec(
            "3.1.0",
            &json!({}),
            &json!({"schemas": {
                "A": {"type": "object", "required": ["a"], "properties": {"a": {"type": "string"}}},
                "B": {"type": "object", "required": ["b"], "properties": {"b": {"type": "string"}}},
                "Any": {"anyOf": any},
            }}),
        );
        let files =
            common::generate(if flip { "any-of-b" } else { "any-of-a" }, &doc, &crate::emit::Options::default());
        let types = common::file(&files, "types.rs");
        let body = types.split("for Any {").nth(1).unwrap_or_else(|| panic!("{types}")).to_owned();
        body.split("decode_ref::<")
            .skip(1)
            .filter_map(|t| t.split_once('>'))
            .map(|(t, _)| t.to_owned())
            .collect::<Vec<_>>()
    };
    let first = tries(false);
    assert_eq!(first.len(), 4, "{first:?}");
    assert_eq!(first, tries(true));
}

/// Untitled inline branches and colliding variant names follow the sorted schema text of their branch, so a reorder of
/// `anyOf` branches keeps every name on the same branch.
#[test]
fn a_reorder_of_union_branches_keeps_their_names() {
    let items = |flip: bool| {
        let mut branches = vec![
            json!({"type": "object", "required": ["a"], "properties": {"a": {"type": "string"}}}),
            json!({"type": "object", "required": ["b"], "properties": {"b": {"type": "string"}}}),
            json!({"type": "string", "pattern": "^x"}),
            json!({"type": "string", "pattern": "^y"}),
        ];
        if flip {
            branches.reverse();
        }
        let doc = common::spec("3.1.0", &json!({}), &json!({"schemas": {"Any": {"anyOf": branches}}}));
        let files = common::generate(if flip { "union-b" } else { "union-a" }, &doc, &crate::emit::Options::default());
        // Each top-level item with its lines sorted: a name stays with what it contains, whatever the order. The guards
        // `fit_<i>` of `Deserialize` are numbered by position and named nowhere else.
        let mut items: Vec<Vec<String>> = Vec::new();
        for l in common::file(&files, "types.rs").lines().filter(|l| !l.contains("fit_")) {
            match items.last_mut() {
                Some(item) if l.starts_with(' ') || l.starts_with('}') || l.starts_with(')') => {
                    item.push(l.trim().to_owned());
                }
                _ => items.push(vec![l.to_owned()]),
            }
        }
        for item in &mut items {
            item.sort();
        }
        items.sort();
        items
    };
    let first = items(false);
    assert!(first.iter().flatten().any(|l| l.contains("Variant4")), "{first:#?}");
    assert_eq!(first, items(true));
}

/// A generated module calls honig by its path: it has no `runtime.rs` and no `runtime::`. Its root re-exports the items
/// of honig that the generated signatures refer to, and none that they do not.
#[test]
fn a_module_calls_honig_by_its_path() {
    let ok = json!({"description": "ok", "content": {"application/json": {"schema": {"type": "string"}}}});
    let paths = json!({"/a": {"get": {"operationId": "a", "responses": {"200": ok}}}});
    let files =
        common::generate("honig-path", &common::spec("3.2.0", &paths, &json!({})), &crate::emit::Options::default());
    assert!(files.iter().all(|(name, text)| name != "runtime.rs" && !text.contains("runtime::")), "{files:#?}");
    let root: String = common::file(&files, "mod.rs").split_whitespace().collect();
    let listed = root.split_once("pubuse::honig::{").and_then(|(_, rest)| rest.split_once('}')).map(|(l, _)| l);
    let listed: Vec<&str> = listed.unwrap_or_default().split(',').filter(|n| !n.is_empty()).collect();
    assert!(listed.contains(&"ApiError") && !listed.contains(&"block_on"), "{root}");
    for name in listed {
        let used = files.iter().any(|(f, text)| f != "mod.rs" && text.contains(&format!("::honig::{name}")));
        assert!(used, "`{name}` is re-exported but no generated file refers to it");
    }
}

/// A module generated by an earlier version has a `runtime.rs`. Generating again removes it: no file refers to it.
#[test]
fn generating_removes_the_runtime_file_of_an_earlier_run() {
    let dir = std::env::temp_dir().join(format!("honig-old-runtime-{}", std::process::id())).join("api");
    let old = dir.join("runtime.rs");
    crate::disk::write(&old, "// @generated by honig\npub use honig::*;\n").unwrap();
    let files = common::generate(
        "old-runtime",
        &common::spec("3.2.0", &json!({}), &json!({})),
        &crate::emit::Options::default(),
    );
    crate::disk::write_module(&dir, &dir.join("mod.rs"), &files).unwrap();
    assert!(!crate::disk::is_file(&old) && crate::disk::is_file(&dir.join("mod.rs")));
}

/// The body of a builder and of the errors of a response reads as Rust: one entry per line, `Vec<String>` and not
/// `Vec < String >`. `prettyplease` prints a macro body as raw tokens.
#[test]
fn macro_bodies_print_one_entry_per_line() {
    let tags = json!({"type": "object", "properties": {"name": {"type": "string"},
        "tags": {"type": "array", "items": {"type": "string"}}}});
    let failed = |code: u16| {
        (
            code.to_string(),
            json!({"description": "no", "content": {"application/json":
        {"schema": {"$ref": "#/components/schemas/Tags"}}}}),
        )
    };
    let mut responses = serde_json::Map::from_iter([failed(400), failed(403), failed(404)]);
    responses.insert("201".into(), json!({"description": "made"}));
    let paths = json!({"/tags": {"post": {"operationId": "makeTags", "responses": responses}}});
    let files = common::generate(
        "macro-bodies",
        &common::spec("3.2.0", &paths, &json!({"schemas": {"Tags": tags}})),
        &crate::emit::Options::default(),
    );
    let types = common::file(&files, "types.rs");
    assert!(types.contains("        optional {\n            name(name): String = \"name\" => some, None,\n            tags(tags): Vec<String> = \"tags\" => some, None,\n        }\n"), "{types}");
    let ops = common::file(&files, "operations.rs");
    assert!(ops.contains("            Self::Created => \"status 201 (Created)\",\n"), "{ops}");
}

/// A keyword beside a `$ref` to a checked type (`minLength` beside a `maxLength` type) is not the type's own: the field
/// gets a checked type of its own that checks it.
#[test]
fn a_keyword_beside_a_checked_type_is_checked() {
    let code = json!({"type": "string", "maxLength": 9});
    let holder =
        json!({"type": "object", "properties": {"code": {"$ref": "#/components/schemas/Code", "minLength": 2}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Code": code, "Holder": holder}}));
    let types = common::file(&common::generate("beside-checked", &doc, &crate::emit::Options::default()), "types.rs")
        .split_whitespace()
        .collect::<String>();
    assert!(types.contains("pubcode:HolderCodeChecked,"), "{types}");
    assert!(
        types.contains("check_len(out,&::honig::InstanceLocation::Root,n,Some(2),Some(9),\"characters\""),
        "{types}"
    );
}

/// The default of a number field is a literal of the field's Rust type, for every width: `5i8`, `5u16`, `1.5f32`.
#[test]
fn a_number_default_is_a_literal_of_its_width() {
    let field = |t: &str, f: &str, d: serde_json::Value| json!({"type": t, "format": f, "default": d});
    let fields = json!({"a": field("integer", "int8", json!(5)), "b": field("integer", "int16", json!(5)),
        "c": field("integer", "uint8", json!(5)), "d": field("integer", "uint16", json!(5)),
        "e": field("integer", "uint32", json!(5)), "f": field("number", "double", json!(1.5)),
        "g": field("number", "float", json!(1.5))});
    let doc =
        common::spec("3.2.0", &json!({}), &json!({"schemas": {"Widths": {"type": "object", "properties": fields}}}));
    let types: String =
        common::file(&common::generate("default-widths", &doc, &crate::emit::Options::default()), "types.rs")
            .split_whitespace()
            .collect();
    for (t, literal) in [
        ("i8", "5i8"),
        ("i16", "5i16"),
        ("u8", "5u8"),
        ("u16", "5u16"),
        ("u32", "5u32"),
        ("f64", "1.5f64"),
        ("f32", "1.5f32"),
    ] {
        assert!(types.contains(&format!("->{t}{{{literal}}}")), "{literal}: {types}");
    }
}

/// A field of an alias of a checked type (`A: $ref S`, `S` with `not`) is that type: `S` checks its keywords, so the
/// field gets no second check of them.
#[test]
fn an_alias_of_a_checked_type_is_checked_once() {
    let schemas = json!({"schemas": {"S": {"type": "string", "not": {"const": "x"}}, "A": {"$ref": "#/components/schemas/S"},
        "Holder": {"type": "object", "properties": {"a": {"$ref": "#/components/schemas/A"}}}}});
    let doc = common::spec("3.2.0", &json!({}), &schemas);
    let types =
        common::file(&common::generate("alias-checked", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("pub a: A,") && !types.contains("pub struct HolderA("), "{types}");
}
