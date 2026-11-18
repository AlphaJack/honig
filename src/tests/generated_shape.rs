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
    assert!(op_module(&ops, "a").contains("pub const SERVERS: &[&str] = &[\"https://a.example.com\"];"), "{ops}");
    assert!(!op_module(&ops, "b").contains("SERVERS"), "{ops}");
}

/// Every `Params` has `new` taking what is required; `Default` only when that is nothing.
#[test]
fn params_always_have_new_and_default_only_without_arguments() {
    let ops = operations("shape-default");
    let module = |op: &str| op_module(&ops, op);
    // `Default` is written in the module or by a macro that the file shares (`shared_impl_N!(Params);`).
    let shared: Vec<String> = ops
        .split("macro_rules! ")
        .skip(1)
        .filter(|m| m.split("};").next().is_some_and(|body| body.contains("impl Default for $t")))
        .filter_map(|m| m.split_whitespace().next().map(|name| format!("{name}!(Params)")))
        .collect();
    let has_default = |op: &str| {
        let m = module(op);
        m.contains("impl Default for Params") || shared.iter().any(|s| m.contains(s.as_str()))
    };
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

/// A string with a `pattern` is always a checked newtype. The description uses `pattern`. So `generate` turns on the
/// `regress` feature of the runtime.
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
    assert!(types.contains("pub struct TeamSlug(String);"), "{types}");
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
    assert!(ops.contains(r#"runtime::Scheme::Http("shop_token", "Token")"#), "{ops}");
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
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let params: Vec<serde_json::Value> = names
        .iter()
        .map(|n| json!({"name": n, "in": "query", "required": true, "schema": {"type": "integer"}}))
        .collect();
    let properties: serde_json::Map<String, serde_json::Value> =
        names.iter().map(|n| ((*n).to_owned(), json!({"type": "integer"}))).collect();
    let example: serde_json::Map<String, serde_json::Value> =
        names.iter().map(|n| ((*n).to_owned(), json!(1))).collect();
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

/// A crate that denies `clippy::allow_attributes` takes the generated code: every `#[allow]` is an inner `#![allow]`,
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

/// A binary without a client or a server names none of the operations, nor their `SECURITY_<n>`: the file allows
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
    assert!(ops.contains("reason = \"without a client or a server a binary names none"), "{ops}");
}

/// A `text/plain` string error body holds the detail: the server fills it for the requests it refuses itself and no
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
    assert!(compact.contains(r#"runtime::ErrorSchema{media:"text/plain",fields:&[],open:false,}"#), "{server}");
    assert!(!g.diagnostics.iter().any(|d| d.message.contains("cannot fill")), "{:?}", g.diagnostics);
}

/// Each operation names its span by the HTTP server conventions of OpenTelemetry. The `tracing`
/// feature of the runtime opens it. So generated code never names `tracing`.
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
    for field in ["span: \"GET /products/{productId}\"", "route: \"/products/{productId}\"", "method: \"GET\""] {
        assert!(server.contains(field), "{field} in {server}");
    }
    // The span itself is the runtime's `tracing` feature: generated code never names `tracing`.
    assert!(!server.contains("tracing"), "{server}");
}

/// `Params::new` takes path parameters in URL order and a required body.
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
                    {"name": "repo", "in": "path", "required": true, "schema": {"type": "string"}},
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
    assert!(ops.contains("pub const fn new(org: String, repo: String) -> Self"), "{ops}");
    // A string default is built at run time (`to_owned`). So that `new` cannot be `const`.
    assert!(op_module(ops, "sorted").contains("    pub fn new() -> Self"), "{ops}");
    assert!(!ops.contains("Params as "), "no short aliases: {ops}");
    let server = common::file(&files, "server.rs");
    assert!(server.contains("params: ops::get_repo::Params,"), "{server}");
    assert!(server.contains("params: ops::list::Params,"), "{server}");
}

/// A description without schemas. `types.rs` has no type to check. So there is no runtime import.
#[test]
fn types_without_schemas_import_no_runtime() {
    let doc = common::spec("3.2.0", &json!({}), &json!({}));
    let opts = crate::emit::Options::default();
    let types = common::file(&common::generate("shape-no-schemas", &doc, &opts), "types.rs").to_owned();
    assert!(!types.contains("use super::runtime;"), "{types}");
}

/// `new` takes exactly the required properties without a spec default; the rest start empty or at that default.
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
    assert!(flat.contains("tag:None,note:runtime::Nullable::Absent,"), "{types}");
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
        assert!(server.contains(&format!("ranges: {ranges},")), "{name}: {server}");
    }
}

/// Every schema the description names keeps its name, though another one has its shape: `PaymentMethod` and
/// `DeliverySpeed` are both `enum: [0, 1]`, and so are the inline `status` and `priority` of `Order`. Inline twins
/// in one role (`status` of two operations) still share one type, named for what they share (`OkStatus`). A component parameter's schema is named after its
/// key, not after the first operation that uses it.
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
    // A body of two object schemas that no Rust type judges alone: the generated code checks it as JSON, unless the
    // schema is a type of yours.
    let generate = |test: &str, rust_type: Option<&str>| {
        let mut note = json!({"oneOf": [
            {"type": "object", "required": ["a"], "properties": {"a": {"type": "string"}}},
            {"type": "object", "required": ["b"], "properties": {"b": {"type": "string"}}},
        ]});
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
        (common::file(&files, "operations.rs").to_owned(), common::file(&files, "server.rs").to_owned())
    };
    let (ops, server) = generate("shape-own-body-generated", None);
    assert!(op_module(&ops, "two").contains("check_schema"), "{ops}");
    assert!(server.contains("check_raw_json"), "{server}");
    let (ops, server) = generate("shape-own-body-yours", Some("crate::Note"));
    assert!(!ops.contains("check_schema"), "{ops}");
    assert!(!server.contains("check_raw_json"), "{server}");
}

/// The body of a generated `macro_rules!` is printed as the items it expands to, one per line, and not as one line of
/// tokens.
#[test]
fn a_macro_body_reads_as_items() {
    let doc = common::spec("3.1.0", &json!({}), &json!({"schemas": {"Kind": {"type": "string", "enum": ["a", "b"]}}}));
    let files = common::generate("shape-macro-body", &doc, &crate::emit::Options::default());
    let types = common::file(&files, "types.rs");
    assert!(
        types.contains("macro_rules! str_enum_impls {\n    ($n:ident) => {\n        impl std::fmt::Display for $n {\n"),
        "{types}"
    );
}

/// A response of a union of unions: each layer holds the two before it, so 2^40 paths reach the bottom. Its size is
/// found once per type and to the bottom: about 340 bytes for `U39` against about 100 for the struct of `201`, so `Ok`
/// is boxed. Cut at 16 levels, `U39` read as about 180.
#[test]
fn a_response_of_a_union_of_unions_is_sized_to_the_bottom() {
    let mut schemas = serde_json::Map::new();
    schemas.insert("U0".into(), json!({"type": "object", "properties": {"a": {"type": "integer"}}}));
    schemas.insert("U1".into(), json!({"type": "object", "properties": {"b": {"type": "string"}}}));
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

/// A `$ref` beside an extension (`x-patch-strategy`) is the referenced type: an extension asserts nothing, so nothing
/// is checked whole. Properties named like keywords (`$ref`, `type`) are field names, not keywords.
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
    let all: String = files.iter().map(|(_, text)| text.as_str()).collect();
    assert!(!all.contains("check_schema"), "{all}");
}

/// A schema that many schemas checked whole reach is in the generated code once: in `runtime.rs`, which every
/// check of the module shares. Each check used to carry its own copy, so the code grew with the product of the two.
#[test]
fn a_schema_that_many_whole_checks_reach_is_generated_once() {
    let mut schemas = serde_json::Map::new();
    schemas.insert(
        "Big".to_owned(),
        json!({"type": "object", "properties": {"code": {"type": "string", "maxLength": 4242}}}),
    );
    let mut paths = serde_json::Map::new();
    for i in 0..20 {
        // `not` has no Rust type: each of these is checked whole at run time, and reaches `Big`.
        schemas.insert(
            format!("S{i}"),
            json!({"type": "object", "properties": {"big": {"$ref": "#/components/schemas/Big"}}, "not": {"required": ["x"]}}),
        );
        paths.insert(
            format!("/s{i}"),
            json!({"post": {
                "operationId": format!("s{i}"),
                "requestBody": {"content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/S{i}")}}}},
                "responses": {"204": {"description": "ok"}},
            }}),
        );
    }
    let doc = common::spec("3.1.0", &serde_json::Value::Object(paths), &json!({"schemas": schemas}));
    let files = common::generate("shape-shared-defs", &doc, &crate::emit::Options::default());
    let copies: usize = files.iter().map(|(_, text)| text.matches(r#"maxLength\":4242"#).count()).sum();
    assert_eq!(copies, 1, "{files:#?}");
    assert!(common::file(&files, "runtime.rs").contains("fn schema_defs"), "{files:#?}");
}
