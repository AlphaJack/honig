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
    // A body whose `dependencies` no Rust type judges: the generated code checks it as JSON, unless the schema is a
    // type of yours. Empty, it asserts nothing.
    let generate = |test: &str, rust_type: Option<&str>| {
        let mut note = json!({
            "type": "object",
            "properties": {"a": {"type": "string"}, "b": {"type": "string"}},
            "dependencies": {},
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
    // `Note` is a checked type: decoding it, whatever the media type, checks its whole schema.
    let (types, server) = generate("shape-own-body-generated", None);
    assert!(types.contains("pub struct Note(pub(super) NoteFields);") && types.contains("check_schema"), "{types}");
    assert!(server.contains("check_raw_json"), "{server}");
    let (types, server) = generate("shape-own-body-yours", Some("crate::Note"));
    assert!(!types.contains("check_schema"), "{types}");
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
        // `dependencies` has no Rust type: each of these is checked whole at run time, and reaches `Big`. Empty, it
        // asserts nothing.
        schemas.insert(
            format!("S{i}"),
            json!({"type": "object", "properties": {"big": {"$ref": "#/components/schemas/Big"}},
                "dependencies": {}}),
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

/// A builder gathers the `?` of each checked field into one. A struct without a checked field builds with `new` and
/// struct update, with no `?`, so it gets no builder.
#[test]
fn only_a_struct_with_a_checked_field_gets_a_builder() {
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
    assert!(!types.contains("impl super::Plain {"), "{types}");
    assert!(types.contains("impl super::Coded {"), "{types}");
    assert!(types.contains("impl super::Tagged {"), "{types}");
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

/// `format: time` is the runtime's `FullTime` whatever the date library: neither date library has a time with an
/// offset, and `FullTime` holds plain numbers.
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
    assert!(types.contains("pub start: runtime::FullTime"), "{types}");
}

/// `dependentRequired` over properties that the struct declares is a check of its fields: no schema is checked whole.
/// A name that the struct does not declare, or an optional one whose default hides its absence, keeps the whole check.
#[test]
fn dependent_required_checks_the_fields() {
    let types = |schema: serde_json::Value| {
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Payment": schema}}));
        common::file(&common::generate("dependent-required", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned()
    };
    let typed = types(json!({"type": "object", "properties": {"card": {"type": "string"}, "cvc": {"type": "string"}},
        "dependentRequired": {"card": ["cvc"]}}));
    assert!(typed.contains("is required when") && !typed.contains("check_schema"), "{typed}");
    let undeclared = types(json!({"type": "object", "properties": {"card": {"type": "string"}},
        "dependentRequired": {"card": ["cvc"]}}));
    assert!(undeclared.contains("check_schema"), "{undeclared}");
    let defaulted = types(json!({"type": "object", "properties": {"card": {"type": "string"},
        "cvc": {"type": "string", "default": "000"}}, "dependentRequired": {"card": ["cvc"]}}));
    assert!(defaulted.contains("check_schema"), "{defaulted}");
}

/// `propertyNames` of `pattern`, `minLength` and `maxLength` is a check of each name: no schema is checked whole. Any
/// other schema of names is a type that each name decodes as. A map component holds it where it is used, as
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
    assert!(typed.contains("for name in") && !typed.contains("check_schema"), "{typed}");
    let other = types(json!({"enum": ["a", "b"]}));
    assert!(other.contains("is not an allowed property name") && !other.contains("check_schema"), "{other}");
}

/// One `patternProperties` entry of a closed map is a map of the pattern's schema whose names are checked against the
/// pattern. An open map or two patterns check each matched value: no schema is checked whole either way.
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
    assert!(typed.contains("for name in") && !typed.contains("check_schema"), "{typed}");
    let open = types(json!({"type": "object", "patternProperties": {"^[a-z]+$": int}}));
    assert!(!open.contains("check_schema"), "{open}");
    let two = types(json!({"type": "object", "patternProperties": {"^[a-z]+$": int, "^x": int},
        "additionalProperties": false}));
    assert!(!two.contains("check_schema"), "{two}");
}

/// A schema without `type` whose keywords each apply to one kind of value (`minimum`, `maxLength`, `maxItems`, ...) is
/// a `serde_json::Value` checked by its kind: no schema is checked whole.
#[test]
fn keywords_without_a_type_check_values_of_their_kind() {
    let any = json!({"type": "object", "properties": {"v": {"minimum": 1, "maxLength": 3, "maxItems": 2}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Loose": any}}));
    let types =
        common::file(&common::generate("typeless", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("serde_json::Value::Number") && !types.contains("check_schema"), "{types}");
}

/// `not: S` is a check that the value does not decode as the type of `S`: no schema is checked whole.
#[test]
fn not_is_a_check_against_the_type_of_its_schema() {
    let code = json!({"type": "object", "properties": {"code": {"type": "string", "not": {"type": "string", "pattern": "^x"}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Coded": code}}));
    let types = common::file(&common::generate("not", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("serde_json::from_value") && !types.contains("check_schema"), "{types}");
}

/// `dependentSchemas` over properties that the struct declares is a check that the value decodes as the type of the
/// schema when the property is present: no schema is checked whole.
#[test]
fn dependent_schemas_check_against_the_type_of_the_schema() {
    let card = json!({"type": "object", "properties": {"card": {"type": "string"}, "cvc": {"type": "string"}},
        "dependentSchemas": {"card": {"required": ["cvc"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Payment": card}}));
    let types =
        common::file(&common::generate("dependent-schemas", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is present: ") && !types.contains("\\\"dependentSchemas\\\""), "{types}");
    let voucher = json!({"type": "object", "properties": {"code": {"type": "string"}, "amount": {"type": "integer"}},
        "dependentSchemas": {"code": {"properties": {"amount": {"maximum": 100}}, "required": ["amount"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Voucher": voucher}}));
    let types =
        common::file(&common::generate("dependent-schemas2", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is present: ") && !types.contains("\\\"dependentSchemas\\\""), "{types}");
}

/// An `if` that is no tagged union checks the value against the types of its schemas: a value of the type of `if` is
/// one of the type of `then`, any other one of the type of `else`. No schema is checked whole.
#[test]
fn if_then_else_checks_against_the_types_of_its_schemas() {
    let size = json!({"type": "object", "properties": {"unit": {"type": "string"}, "value": {"type": "number"}},
        "if": {"properties": {"unit": {"type": "string", "pattern": "^cm$"}}},
        "then": {"properties": {"value": {"type": "number", "maximum": 300}}},
        "else": {"properties": {"value": {"type": "number", "maximum": 120}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Size": size}}));
    let types =
        common::file(&common::generate("if-then-else", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("`then`: ") && !types.contains("check_schema"), "{types}");
}

/// A named type checks its own `not`: a field of it takes no checked type of its own.
#[test]
fn a_field_of_a_type_that_checks_itself_takes_no_second_check() {
    let code = json!({"type": "object", "properties": {"a": {"type": "string"}}, "not": {"required": ["a"]}});
    let holder = json!({"type": "object", "properties": {"code": {"$ref": "#/components/schemas/Code"}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Code": code, "Holder": holder}}));
    let types =
        common::file(&common::generate("held-not", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("pub code: Option<Code>"), "{types}");
}

/// `contains` on an array counts the items that decode as the type of its schema, held to `minContains` and
/// `maxContains`: no schema is checked whole.
#[test]
fn contains_counts_the_items_of_its_type() {
    let tags = json!({"type": "array", "items": {"type": "string"}, "contains": {"const": "vip"}, "maxContains": 1});
    let tagged = json!({"type": "object", "properties": {"tags": tags}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Tagged": tagged}}));
    let types =
        common::file(&common::generate("contains", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("matching items, more than allowed") && !types.contains("check_schema"), "{types}");
}

/// An `anyOf` with a branch that asserts nothing holds for every value: no schema is checked whole.
#[test]
fn an_any_of_with_a_branch_of_any_value_is_typed() {
    let money = json!({"type": "object", "properties": {"amount": {"type": "integer"}}});
    let price =
        json!({"type": "object", "properties": {"price": {"anyOf": [{"$ref": "#/components/schemas/Money"}, {}]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Money": money, "Price": price}}));
    let types =
        common::file(&common::generate("any-branch", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(!types.contains("check_schema"), "{types}");
}

/// A `oneOf` whose discriminator picks one branch while two share a type counts the branches that the value decodes
/// as: no schema is checked whole.
#[test]
fn a_one_of_of_shared_types_counts_its_branches() {
    let car = json!({"type": "object", "properties": {"kind": {"type": "string"}, "wheels": {"type": "integer"}}});
    let van = json!({"type": "object", "properties": {"kind": {"type": "string"}, "doors": {"type": "integer"}}});
    let vehicle = json!({"oneOf": [{"$ref": "#/components/schemas/Car"}, {"$ref": "#/components/schemas/Van"}],
        "discriminator": {"propertyName": "kind"}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Car": car, "Van": van, "Vehicle": vehicle}}));
    let types =
        common::file(&common::generate("one-of-count", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("of `oneOf`, not exactly one") && !types.contains("check_schema"), "{types}");
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
    assert!(types.contains("runtime::check_unique") && !types.contains("check_schema"), "{types}");
}

/// `patternProperties` with several patterns, or beside declared properties, checks the value of each matched name
/// against the type of its schema: no schema is checked whole.
#[test]
fn pattern_properties_check_each_matched_value() {
    let tags = json!({"type": "object", "properties": {"id": {"type": "string"}},
        "patternProperties": {"^x_": {"type": "string"}, "^n_": {"type": "integer", "minimum": 0}},
        "additionalProperties": false});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Tags": tags}}));
    let types =
        common::file(&common::generate("patterns", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("no value is allowed here") && !types.contains("check_schema"), "{types}");
}

/// `additionalProperties` with a schema beside `patternProperties` holds for the names that no pattern matches and the
/// object does not declare: no schema is checked whole.
#[test]
fn additional_properties_beside_patterns_checks_the_other_names() {
    let tags = json!({"type": "object", "patternProperties": {"^x_": {"type": "string"}},
        "additionalProperties": {"type": "integer"}});
    let holder = json!({"type": "object", "properties": {"tags": tags}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types = common::file(
        &common::generate("additional-beside-patterns", &doc, &crate::emit::Options::default()),
        "types.rs",
    )
    .to_owned();
    assert!(types.contains("let mut matched = false;") && !types.contains("check_schema"), "{types}");
}

/// A tuple that no Rust tuple holds (`prefixItems` with fewer `minItems`, or open) checks each item against the type
/// of its schema: no schema is checked whole.
#[test]
fn a_tuple_checks_each_item_against_its_schema() {
    let pair = json!({"type": "array", "prefixItems": [{"type": "string"}, {"type": "integer"}], "items": false});
    let holder = json!({"type": "object", "properties": {"pair": pair}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types = common::file(&common::generate("tuple", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("runtime::InstancePath::Index") && !types.contains("check_schema"), "{types}");
}

/// `required` of an object without `properties` is a map that must have those names.
#[test]
fn a_map_checks_the_names_it_requires() {
    let holder = json!({"type": "object", "properties": {"tags": {"type": "object", "required": ["foo"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("map-required", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("runtime::check_required") && types.contains("\"foo\""), "{types}");
}

/// Keywords beside a `oneOf` hold for whichever branch matches: the value also decodes as their type. No schema is
/// checked whole.
#[test]
fn keywords_beside_a_union_hold_for_each_branch() {
    let a = json!({"type": "object", "properties": {"kind": {"const": "a"}, "x": {"type": "integer"}}, "required": ["kind"]});
    let b = json!({"type": "object", "properties": {"kind": {"const": "b"}, "y": {"type": "integer"}}, "required": ["kind"]});
    let item = json!({"type": "object", "maxProperties": 3,
        "oneOf": [{"$ref": "#/components/schemas/A"}, {"$ref": "#/components/schemas/B"}]});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"A": a, "B": b, "Item": item}}));
    let types =
        common::file(&common::generate("beside-union", &doc, &crate::emit::Options::default()), "types.rs").to_owned();
    assert!(types.contains("serde_json::from_value") && !types.contains("check_schema"), "{types}");
}

/// An `allOf` whose members one merged type cannot hold (two `maxLength`) is that type, and the value also decodes as
/// each member: no schema is checked whole.
#[test]
fn an_all_of_that_loses_keywords_checks_each_member() {
    let code = json!({"allOf": [{"type": "string", "maxLength": 8}, {"type": "string", "maxLength": 4}]});
    let holder = json!({"type": "object", "properties": {"code": code}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("lossy-all-of-typed", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("serde_json::from_value") && !types.contains("check_schema"), "{types}");
}

/// An `enum` or `const` of values that no Rust type holds (numbers with strings, objects) compares the value as JSON:
/// no schema is checked whole.
#[test]
fn values_that_no_type_holds_are_compared_as_json() {
    let mixed = json!({"type": "object", "properties": {"size": {"enum": [1, "large", {"w": 2}]}, "unit": {"const": {"si": true}}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Sized": mixed}}));
    let types = common::file(&common::generate("untyped-values", &doc, &crate::emit::Options::default()), "types.rs")
        .to_owned();
    assert!(types.contains("runtime::json_equal") && !types.contains("check_schema"), "{types}");
}

/// `unevaluatedProperties` refuses the names that the schema, what it applies in place and the branches that the value
/// matches leave: no schema is checked whole.
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
    assert!(types.contains("is not allowed (unevaluatedProperties)") && !types.contains("check_schema"), "{types}");
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
    assert!(types.contains("matches!(name.as_str(), \"foo\")"), "{types}");
}

/// `unevaluatedItems` refuses the items that `contains` and the branches that the value matches leave: no schema is
/// checked whole.
#[test]
fn unevaluated_items_refuses_what_nothing_evaluates() {
    let pair = json!({"type": "array", "contains": {"type": "integer"}, "unevaluatedItems": false});
    let holder = json!({"type": "object", "properties": {"pair": pair}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("unevaluated-items-typed", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is not allowed (unevaluatedItems)") && !types.contains("check_schema"), "{types}");
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
    assert!(flat.contains("&&(serde_json::from_value"), "{types}");
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
    assert!(!types.contains("check_schema"), "{types}");
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
    assert!(types.contains("Option<Vec<Self>>"), "{types}");
}

/// `propertyNames` of any schema decodes each name, as a JSON string, as its type: no schema is checked whole.
#[test]
fn property_names_of_any_schema_decode_each_name() {
    let labels = json!({"type": "object", "additionalProperties": {"type": "string"},
        "propertyNames": {"enum": ["red", "green"]}});
    let holder = json!({"type": "object", "properties": {"labels": labels}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("property-names-any", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is not an allowed property name") && !types.contains("check_schema"), "{types}");
}

/// `dependentSchemas` of a name that the struct does not declare keeps that name and checks the schema when it is
/// present: no schema is checked whole.
#[test]
fn dependent_schemas_of_an_undeclared_name_are_typed() {
    let card = json!({"type": "object", "properties": {"cvc": {"type": "string"}},
        "dependentSchemas": {"card": {"type": "object", "required": ["cvc"]}}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Payment": card}}));
    let types =
        common::file(&common::generate("dependent-undeclared", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("is present: ") && !types.contains("check_schema"), "{types}");
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
/// of their kind only: each is checked on its own, and no schema is checked whole.
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
        assert!(types.contains(check) && !types.contains("check_schema"), "{keyword}: {types}");
    }
}

/// An `if` on one property's constant without `type: object` is no tagged union: its value may be of any kind. The
/// generated code checks its `then` and `else`, so a value of the wrong branch is not taken without a word.
#[test]
fn a_conditional_without_an_object_type_is_checked() {
    let v = json!({"if": {"properties": {"kind": {"const": "n"}}, "required": ["kind"]},
        "then": {"properties": {"x": {"type": "number"}}}, "else": {"properties": {"x": {"type": "string"}}}});
    let holder = json!({"type": "object", "required": ["v"], "properties": {"v": v}});
    let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
    let types =
        common::file(&common::generate("typeless-conditional", &doc, &crate::emit::Options::default()), "types.rs")
            .to_owned();
    assert!(types.contains("`then`: ") || types.contains("check_schema"), "{types}");
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
        assert!(types.contains(check) && !types.contains("check_schema"), "{keyword}: {types}");
    }
}

/// `false` refuses any value that is present, and `items`, `prefixItems` or `additionalProperties` without `type` are
/// held by the union of their implied shape: no schema is checked whole.
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
    assert!(types.contains("no value is allowed here") && !types.contains("check_schema"), "{types}");
    // The names of the object that `named` takes match its one pattern.
    assert!(types.contains("does not match `^a`"), "{types}");
    // Each item of `list` and each value of `map` decodes as its schema: two item checks besides those of `pair`, and
    // one key check besides the pattern of `named`, whose names its pattern alone judges.
    assert_eq!(types.matches("InstancePath::Index").count(), 4, "{types}");
    assert_eq!(types.matches("InstancePath::Key").count(), 2, "{types}");
}

/// A `oneOf` or `anyOf` whose branches no union tells apart (a branch without one `type`) counts the branches that the
/// value decodes as: exactly one of `oneOf`, one at least of `anyOf`. No schema is checked whole.
#[test]
fn a_loose_union_counts_its_branches() {
    for (keyword, check) in [("oneOf", "not exactly one"), ("anyOf", "matches none of `anyOf`")] {
        let v = json!({keyword: [{"type": "integer"}, {"minimum": 2}]});
        let holder = json!({"type": "object", "required": ["v"], "properties": {"v": v}});
        let doc = common::spec("3.2.0", &json!({}), &json!({"schemas": {"Holder": holder}}));
        let files = common::generate(&format!("loose-{keyword}"), &doc, &crate::emit::Options::default());
        let types = common::file(&files, "types.rs");
        assert!(types.contains(check) && !types.contains("check_schema"), "{keyword}: {types}");
    }
}
