//! Spec rules whose violation is silent. The generated client and server
//! agree with each other and compile but differ
//! from what the `OpenAPI` specification says. Each test cites the rule.
//! Fast because it generates in memory and compiles nothing.

use super::common;

use crate::emit::Options;
use serde_json::{Value, json};

fn doc(paths: &Value, components: &Value) -> Value {
    common::spec("3.2.0", paths, components)
}

fn server_of(test: &str, d: &Value) -> String {
    common::file(&common::generate(test, d, &Options::default()), "server.rs").to_owned()
}

fn operations_of(test: &str, d: &Value) -> String {
    common::file(&common::generate(test, d, &Options::default()), "operations.rs").to_owned()
}

/// Generated text with all whitespace and trailing commas removed. So checks
/// do not depend on where the printer breaks lines.
fn squash(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    s.replace(",]", "]").replace(",)", ")").replace(",}", "}")
}

/// The `security` of every `::honig::__private::Op` of a generated server without whitespace. A shared `ops::SECURITY_<n>`
/// is shown as `operations.rs` declares it.
fn security_consts(files: &[(String, String)]) -> Vec<String> {
    let (server, ops) = (squash(common::file(files, "server.rs")), squash(common::file(files, "operations.rs")));
    let value = |name: &str| {
        let decl = format!("pubconst{name}:&[&[::honig::__private::Requirement]]=");
        ops.split_once(&decl).and_then(|(_, rest)| rest.split_once(';')).expect("declared").0.to_owned()
    };
    // `security` is the first member that an `Op` sets; one without it has none.
    server
        .split("::honig::__private::Op=")
        .skip(1)
        .map(|rest| {
            rest.strip_prefix("::honig::__private::Op{security:")
                .and_then(|r| r.split_once(','))
                .map_or("&[]", |(s, _)| s)
        })
        .map(|s| s.strip_prefix("ops::").map_or_else(|| s.to_owned(), value))
        .collect()
}

/// Does a `security` value let a request without credentials through?
/// That is the empty list or an empty alternative.
fn is_public(security: &str) -> bool {
    security == "&[]" || security.starts_with("&[&[]") || security.contains(",&[],") || security.contains(",&[]]")
}

fn bearer() -> Value {
    json!({"securitySchemes": {"local": {"type": "http", "scheme": "bearer"}}})
}

fn secured_op(security: &Value) -> Value {
    json!({"/a": {"get": {"operationId": "a", "security": security, "responses": {"204": {"description": "ok"}}}}})
}

// Security Requirement Object (3.2): keys are component names or URIs.

#[test]
fn security_scheme_named_by_local_uri_is_enforced() {
    for key in ["#/components/securitySchemes/local", "https://example.com/api#/components/securitySchemes/local"] {
        let mut d = doc(&secured_op(&json!([{key: []}])), &bearer());
        d["$self"] = json!("https://example.com/api");
        let consts = security_consts(&common::generate("sec-local-uri", &d, &Options::default()));
        assert_eq!(consts, [r#"&[&[(::honig::__private::Scheme::Bearer("local"),&[])]]"#], "{key}");
    }
}

/// A requirement that lists a scheme by its name and by its URI has one entry too many: renaming one would replace the
/// scopes of the other.
#[test]
fn a_scheme_listed_by_name_and_by_uri_fails_generation() {
    let components = json!({"securitySchemes": {"oauth": {"type": "oauth2", "flows": {"implicit": {
        "authorizationUrl": "https://example.com/auth", "scopes": {"read": "r", "write": "w"}}}}}});
    let uri = "#/components/securitySchemes/oauth";
    for (i, security) in
        [json!([{"oauth": ["write"], uri: []}]), json!([{uri: [], "oauth": ["write"]}])].iter().enumerate()
    {
        let path = common::write(&format!("sec-twice-{i}"), &doc(&secured_op(security), &components), &[]);
        let err = crate::generate::generate(&path, &Options::default()).expect_err("must refuse");
        let at: Vec<&str> = err.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(at.len(), 1, "one error and no unresolved key left behind: {security}: {:#?}", err.0);
        assert!(at[0].contains("one scheme with different scopes"), "{security}: {:#?}", err.0);
    }
}

#[test]
fn security_scheme_named_by_external_uri_is_bundled_and_enforced() {
    let other =
        json!({"components": {"securitySchemes": {"partner": {"type": "apiKey", "in": "header", "name": "X-Key"}}}});
    let d = doc(&secured_op(&json!([{"other.json#/components/securitySchemes/partner": []}])), &json!({}));
    let path = common::write("sec-external-uri", &d, &[("other.json", &other)]);
    let files = crate::generate::generate(&path, &Options::default()).expect("generates").files;
    let consts = security_consts(&files);
    assert_eq!(
        consts,
        [r#"&[&[(::honig::__private::Scheme::ApiKey("partner",::honig::__private::KeyIn::Header,"X-Key"),&[])]]"#]
    );
}

#[test]
fn unresolvable_security_scheme_fails_generation() {
    let d = doc(&secured_op(&json!([{"missing.json#/components/securitySchemes/x": []}])), &bearer());
    let path = common::write("sec-unresolvable", &d, &[]);
    let err = crate::generate::generate(&path, &Options::default()).expect_err("must refuse");
    assert!(err.iter().any(|d| d.message.contains("does not resolve")), "{:#?}", err.0);
    let client_only = Options { server: false, tests: false, ..Options::default() };
    crate::generate::generate(&path, &client_only).expect_err("a client would not send the credential");
}

/// Every combination of known names, URIs, unknown names, AND, OR and
/// `{}`. An operation is public only when the description makes it public.
#[test]
fn no_security_shape_turns_a_secured_operation_public() {
    let names = ["local", "#/components/securitySchemes/local", "ghost", "#/components/securitySchemes/ghost"];
    let mut shapes: Vec<Value> = vec![];
    for a in names {
        shapes.push(json!([{a: []}]));
        shapes.push(json!([{a: []}, {}]));
        for b in names {
            shapes.push(json!([{a: [], b: []}]));
            shapes.push(json!([{a: []}, {b: []}]));
        }
    }
    for (i, security) in shapes.iter().enumerate() {
        let d = doc(&secured_op(security), &bearer());
        let path = common::write(&format!("sec-shape-{i}"), &d, &[]);
        let optional = security.as_array().unwrap().iter().any(|r| r.as_object().unwrap().is_empty());
        match crate::generate::generate(&path, &Options::default()) {
            Ok(g) => {
                let consts = security_consts(&g.files);
                assert_eq!(consts.len(), 1);
                assert_eq!(is_public(&consts[0]), optional, "{security} generated {}", consts[0]);
            }
            Err(e) => assert!(security.to_string().contains("ghost"), "{security} refused: {:#?}", e.0),
        }
    }
}

/// A `security` that is not a list cannot be told apart from a missing one: a bare `security:` in YAML would make an
/// operation public under a root requirement.
#[test]
fn a_security_that_is_not_a_list_fails_generation() {
    let op =
        json!({"/a": {"get": {"operationId": "a", "security": null, "responses": {"204": {"description": "ok"}}}}});
    let mut v32 = doc(&op, &bearer());
    v32["security"] = json!([{"local": []}]);
    let v20 = json!({"swagger": "2.0", "info": {"title": "t", "version": "1"}, "paths": op,
        "securityDefinitions": {"local": {"type": "apiKey", "in": "header", "name": "K"}}, "security": [{"local": []}]});
    for (name, d) in [("sec-null-32", v32), ("sec-null-20", v20)] {
        let path = common::write(name, &d, &[]);
        let err = crate::generate::generate(&path, &Options::default()).expect_err(name);
        assert!(err.iter().any(|d| d.pointer == "/paths/~1a/get/security"), "{name}: {:#?}", err.0);
    }
}

/// Two webhooks or callbacks with one name and method are routed to the first: a public one in front of a secured one
/// would answer for both.
#[test]
fn two_callbacks_of_one_name_and_method_fail_generation() {
    let callback = |security: Value| json!({"done": {"{$request.body#/url}": {"post": {"security": security, "responses": {"204": {"description": "ok"}}}}}});
    let op = |id: &str, security: Value| json!({"operationId": id, "callbacks": callback(security), "responses": {"204": {"description": "ok"}}});
    let paths = json!({"/a": {"get": op("a", json!([]))}, "/b": {"get": op("b", json!([{"local": []}]))}});
    let path = common::write("sec-callbacks", &doc(&paths, &bearer()), &[]);
    let err = crate::generate::generate(&path, &Options::default()).expect_err("must refuse");
    assert!(err.iter().any(|d| d.message.contains("also described by")), "{:#?}", err.0);
    let client = Options { server: false, tests: false, ..Options::default() };
    crate::generate::generate(&path, &client).expect_err("a client fails too");
}

/// A description with an unknown field generates nothing: `securty` on an operation, so the operation would be
/// served without the requirement that its author meant.
#[test]
fn a_description_with_an_unknown_field_fails_generation() {
    let get = |extra: (&str, Value)| {
        let mut op = json!({"operationId": "a", "responses": {"204": {"description": "ok"}}});
        op[extra.0] = extra.1;
        json!({"/a": {"get": op}})
    };
    let local = json!([{"local": []}]);
    let mut root = doc(&get(("securty", local.clone())), &bearer());
    root["security"] = local.clone();
    let mut capital = doc(&get(("x-ok", json!(1))), &bearer());
    capital["Security"] = local.clone();
    let mut item = doc(&get(("x-ok", json!(1))), &bearer());
    item["paths"]["/a"]["security"] = local;
    for (i, (d, ptr)) in
        [(root, "/paths/~1a/get/securty"), (capital, "/Security"), (item, "/paths/~1a/security")].iter().enumerate()
    {
        let path = common::write(&format!("sec-error-{i}"), d, &[]);
        let err = crate::generate::generate(&path, &Options::default()).expect_err(ptr);
        assert!(err.iter().any(|d| d.pointer == *ptr), "{ptr}: {:#?}", err.0);
        let client = Options { server: false, tests: false, ..Options::default() };
        crate::generate::generate(&path, &client).expect_err("a client fails too");
    }
}

/// Two names that are one item in Rust fail generation: the later would be numbered by its place in the document, so a
/// reorder of the paths would make `delete_user` delete from the archive.
#[test]
fn two_names_of_one_rust_item_fail_generation() {
    let ok = json!({"204": {"description": "ok"}});
    let params = json!([{"in": "path", "name": "id", "required": true, "schema": {"type": "string"}}]);
    let operations = json!({
        "/live/users/{id}": {"delete": {"operationId": "delete-user", "parameters": params, "responses": ok}},
        "/archive/users/{id}": {"delete": {"operationId": "deleteUser", "parameters": params, "responses": ok}}
    });
    let schema = json!({"type": "object"});
    let components = json!({"schemas": {"billing_address": schema, "BillingAddress": schema}});
    for (name, d) in
        [("same-item-operations", doc(&operations, &json!({}))), ("same-item-components", doc(&json!({}), &components))]
    {
        let path = common::write(name, &d, &[]);
        for options in [Options::default(), Options { server: false, tests: false, ..Options::default() }] {
            let err = crate::generate::generate(&path, &options).expect_err(name);
            assert!(err.iter().any(|d| d.message.contains("map to the same item in Rust")), "{name}: {:#?}", err.0);
        }
    }
}

/// OAS 3.2 Paths Object: "Templated paths with the same hierarchy but different templated names MUST NOT exist as they
/// are identical." Both would map one URL to two operations and the first would answer every request, also for the
/// secured one: `generate` fails for a client and a server. A template that repeats a variable fails the same way.
#[test]
fn two_templated_paths_that_are_the_same_fail_generation() {
    let op = |id: &str, var: &str, security: &Value| {
        json!({"get": {"operationId": id, "security": security,
            "parameters": [{"in": "path", "name": var, "required": true, "schema": {"type": "string"}}],
            "responses": {"204": {"description": "ok"}}}})
    };
    let same = json!({"/a/{x}": op("get_a_x", "x", &json!([])), "/a/{y}": op("get_a_y", "y", &json!([{"local": []}]))});
    let repeated = json!({"/a/{x}/b/{x}": op("get_a_x_b", "x", &json!([]))});
    for (name, paths, pointer) in
        [("same-template", same, "/paths/~1a~1{y}"), ("repeated-variable", repeated, "/paths/~1a~1{x}~1b~1{x}")]
    {
        let path = common::write(name, &doc(&paths, &bearer()), &[]);
        for options in [Options::default(), Options { server: false, tests: false, ..Options::default() }] {
            let err = crate::generate::generate(&path, &options).expect_err(name);
            assert!(err.iter().any(|d| d.pointer == pointer), "{name}: {:#?}", err.0);
        }
    }
}

/// Any error of the description fails `generate`, for a client and a server: code from it would differ from what it
/// says. A path variable that no parameter declares is one.
#[test]
fn any_error_fails_generation() {
    let paths = json!({"/a/{x}": {"get": {"operationId": "a", "responses": {"204": {"description": "ok"}}}}});
    let path = common::write("any-error", &doc(&paths, &json!({})), &[]);
    for options in [Options::default(), Options { server: false, tests: false, ..Options::default() }] {
        let err = crate::generate::generate(&path, &options).expect_err("an error stops generation");
        assert!(err.iter().any(|d| d.pointer == "/paths/~1a~1{x}/get"), "{:#?}", err.0);
    }
}

/// `generate` reports every error in one run: a check that finds one does not hide the others.
#[test]
fn every_error_is_reported_in_one_run() {
    let op =
        json!({"/a": {"get": {"operationId": "a", "security": null, "responses": {"204": {"description": "ok"}}}}});
    let components = json!({"schemas": {"Range": {"type": "integer", "minimum": 5, "maximum": 1}}});
    let path = common::write("every-error", &doc(&op, &components), &[]);
    let err = crate::generate::generate(&path, &Options::default()).expect_err("two errors");
    let at = |p: &str| err.iter().any(|d| d.pointer.starts_with(p));
    assert!(at("/paths/~1a/get/security") && at("/components/schemas/Range"), "{:#?}", err.0);
}

/// A keyword that generated code would drop fails generation at its pointer: a `$ref` that does not resolve, an
/// unknown `type` and a path parameter missing from its path.
#[test]
fn a_keyword_that_would_lose_its_meaning_fails_generation() {
    let schema = |s: Value| doc(&json!({}), &json!({"schemas": {"S": s}}));
    let stray = json!({"/a": {"get": {"operationId": "a", "responses": {"204": {"description": "ok"}},
        "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}]}}});
    for (d, at) in [
        (
            schema(json!({"properties": {"a": {"$ref": "#/components/schemas/Gone"}}})),
            "/components/schemas/S/properties/a",
        ),
        (schema(json!({"type": "text"})), "/components/schemas/S/type"),
        (doc(&stray, &json!({})), "/paths/~1a/get"),
    ] {
        let path = common::write("lost-meaning", &d, &[]);
        let err = crate::generate::generate(&path, &Options::default()).expect_err(at);
        assert!(err.iter().any(|d| d.pointer.starts_with(at)), "{at}: {:#?}", err.0);
    }
    // A 3.1 `nullable` is an unknown keyword, valid and ignored: a warning says that it allows no `null`.
    let mut nullable = schema(json!({"type": "string", "nullable": true}));
    nullable["openapi"] = json!("3.1.0");
    let made = crate::generate::generate(&common::write("nullable-31", &nullable, &[]), &Options::default());
    let made = made.expect("a valid description generates");
    assert!(made.diagnostics.iter().any(|d| d.pointer == "/components/schemas/S/nullable"), "{:#?}", made.diagnostics);
}

/// An `anyOf` of two open objects that no keyword tells apart decodes `{amount: 5, last4: "4242"}` as the first and
/// drops `last4`: a warning at the `anyOf`. A required property of one that the other lacks separates them.
#[test]
fn an_anyof_whose_objects_cannot_be_told_apart_is_a_warning() {
    let card = |required: Value| json!({"type": "object", "required": required, "properties": {"amount": {"type": "integer"}, "last4": {"type": "string"}}});
    let bank = json!({"type": "object", "required": ["amount"], "properties": {"amount": {"type": "integer"}, "iban": {"type": "string"}}});
    let payment = json!({"anyOf": [{"$ref": "#/components/schemas/Card"}, {"$ref": "#/components/schemas/Bank"}]});
    for (required, fine) in [(json!(["amount"]), false), (json!(["amount", "last4"]), true)] {
        let d = doc(&json!({}), &json!({"schemas": {"Payment": payment, "Card": card(required), "Bank": bank}}));
        let path = common::write("anyof-apart", &d, &[]);
        let made = crate::generate::generate(&path, &Options::default()).expect("a valid description generates");
        let warned = made.diagnostics.iter().any(|d| d.pointer == "/components/schemas/Payment/anyOf");
        assert_eq!(warned, !fine, "a required last4 tells them apart: {:#?}", made.diagnostics);
    }
}

/// Discriminated branches that only an unpinned tag tells apart both match `{kind: "book"}`, which `oneOf` then
/// refuses: a warning says so. A required property of one keeps them apart, and the unpinned tag is still a warning.
#[test]
fn discriminated_branches_that_only_an_unpinned_tag_separates_are_a_warning() {
    let book = json!({"type": "object", "required": ["kind"], "properties": {"kind": {"type": "string"}}});
    let gadget = |required: Value| {
        json!({"type": "object", "required": required,
            "properties": {"kind": {"type": "string"}, "batteries": {"type": "boolean"}}})
    };
    let items = json!({"oneOf": [{"$ref": "#/components/schemas/Book"}, {"$ref": "#/components/schemas/Gadget"}],
        "discriminator": {"propertyName": "kind"}});
    for (gadget, fine) in [(gadget(json!(["kind"])), false), (gadget(json!(["kind", "batteries"])), true)] {
        let d = doc(&json!({}), &json!({"schemas": {"Item": items, "Book": book, "Gadget": gadget}}));
        let path = common::write("unpinned-tag", &d, &[]);
        let made = crate::generate::generate(&path, &Options::default()).expect("a valid description generates");
        let only_tag = made.diagnostics.iter().any(|d| d.message.contains("differ only by `kind`"));
        assert_eq!(only_tag, !fine, "a required `batteries` tells them apart: {:#?}", made.diagnostics);
        assert!(made.diagnostics.iter().any(|d| d.pointer == "/components/schemas/Item/oneOf"), "a warning either way");
    }
}

/// JSON Schema 2020-12 section 8.2.2: an anchor belongs to its schema resource. Two resources may each declare
/// `$anchor: x`, and `#x` in each finds its own. Two `$anchor: x` in one resource are an error: neither would win.
#[test]
fn anchors_belong_to_their_resource() {
    let resource = |id: &str, t: &str| {
        json!({"$id": format!("https://example.com/{id}"), "type": "object", "required": ["v"],
            "properties": {"v": {"$ref": "#x"}}, "$defs": {"X": {"$anchor": "x", "type": t}}})
    };
    let d = doc(&json!({}), &json!({"schemas": {"A": resource("a", "string"), "B": resource("b", "integer")}}));
    let types = common::file(&common::generate("anchors-own", &d, &Options::default()), "types.rs").to_owned();
    let field = |name: &str| {
        let (_, rest) = types.split_once(&format!("pub struct {name} {{")).expect(name);
        rest.split_once('}').expect("its end").0.split_whitespace().collect::<String>()
    };
    assert!(field("A").contains("pubv:String") && field("B").contains("pubv:i64"), "{types}");
    let twice = json!({"$defs": {"P": {"$anchor": "y", "type": "string"}, "Q": {"$anchor": "y", "type": "integer"}}});
    let path = common::write("anchors-twice", &doc(&json!({}), &json!({"schemas": {"C": twice}})), &[]);
    let err = crate::generate::generate(&path, &Options::default()).expect_err("two anchors y");
    assert!(err.iter().any(|d| d.message.contains("`y`")), "{:#?}", err.0);
}

/// A limit beside `x-rust-type` runs on the JSON value before it converts to your type: a field of that type decodes
/// through the generated check.
#[test]
fn a_limit_beside_a_type_of_yours_is_checked() {
    let schemas = json!({"schemas": {"Cents": {"type": "integer", "maximum": 100, "x-rust-type": "i64"},
        "Wallet": {"type": "object", "properties": {"cents": {"$ref": "#/components/schemas/Cents"}}}}});
    let made =
        crate::generate::generate(&common::write("yours-limit", &doc(&json!({}), &schemas), &[]), &Options::default());
    let types = common::file(&made.expect("a valid description generates").files, "types.rs")
        .split_whitespace()
        .collect::<String>();
    assert!(types.contains("impl::honig::__private::formats::CheckforCentsLimits"), "{types}");
    assert!(types.contains("::honig::__private::formats::Json<super::types::CentsLimits>"), "{types}");
}

/// Each limit beside `x-rust-type` is checked, not only `maximum`: lengths, `pattern`, every bound, `multipleOf`, item
/// and property counts and `uniqueItems`. A limit left out would let a value of yours through unchecked.
#[test]
fn every_limit_beside_a_type_of_yours_is_checked() {
    let cases = [
        (json!({"type": "string", "minLength": 2}), "check_len(", "Some(2),None,\"characters\""),
        (json!({"type": "string", "maxLength": 9}), "check_len(", "None,Some(9),\"characters\""),
        (json!({"type": "string", "pattern": "^a"}), "check_pattern(", "Pattern::new(\"^a\",)"),
        (json!({"type": "integer", "minimum": 1}), "check_decimal_range(", "min:Some(\"1\")"),
        (json!({"type": "integer", "exclusiveMinimum": 1}), "check_decimal_range(", "exclusive_min:Some(\"1\")"),
        (json!({"type": "integer", "exclusiveMaximum": 9}), "check_decimal_range(", "exclusive_max:Some(\"9\")"),
        (json!({"type": "integer", "multipleOf": 5}), "check_multiple_text(", "\"5\""),
        (json!({"type": "array", "items": {"type": "string"}, "minItems": 1}), "check_len(", "Some(1),None,\"items\""),
        (json!({"type": "array", "items": {"type": "string"}, "maxItems": 3}), "check_len(", "None,Some(3),\"items\""),
        (json!({"type": "array", "items": {"type": "string"}, "uniqueItems": true}), "check_unique(", "check_unique("),
        (json!({"type": "object", "minProperties": 1}), "check_len(", "Some(1),None,\"properties\""),
        (json!({"type": "object", "maxProperties": 3}), "check_len(", "None,Some(3),\"properties\""),
    ];
    for (mut schema, check, argument) in cases {
        schema["x-rust-type"] = json!("crate::Mine");
        let holder = json!({"type": "object", "properties": {"m": {"$ref": "#/components/schemas/Mine"}}});
        let schemas = json!({"schemas": {"Mine": schema.clone(), "Holder": holder}});
        let made = crate::generate::generate(
            &common::write("yours-limits", &doc(&json!({}), &schemas), &[]),
            &Options::default(),
        );
        let types = common::file(&made.expect("a valid description generates").files, "types.rs").to_owned();
        let limits = types.split_once("for MineLimits").map(|(_, rest)| rest).unwrap_or_default();
        let limits: String = limits.split_once("\n}\n").map_or(limits, |(body, _)| body).split_whitespace().collect();
        assert!(limits.contains(check) && limits.contains(argument), "{schema}: {types}");
    }
}

/// A link expression `$response.body#/idd` that reaches no property of the response body is always absent: a warning
/// at the link. A pointer to a declared property needs none.
#[test]
fn a_link_pointer_that_reaches_nothing_is_a_warning() {
    let param = json!({"in": "path", "name": "id", "required": true, "schema": {"type": "string"}});
    let paths = |pointer: &str| {
        json!({"/start": {"get": {"operationId": "start", "responses": {"200": {"description": "ok",
            "content": {"application/json": {"schema": {"type": "object", "properties": {"id": {"type": "string"}}}}},
            "links": {"next": {"operationId": "get_item", "parameters": {"id": format!("$response.body#{pointer}")}}}}}}},
            "/items/{id}": {"get": {"operationId": "get_item", "parameters": [param],
            "responses": {"204": {"description": "ok"}}}}})
    };
    let made = |pointer: &str| {
        let path = common::write("link-pointer", &doc(&paths(pointer), &json!({})), &[]);
        crate::generate::generate(&path, &Options::default())
    };
    let warned = |pointer: &str| {
        let made = made(pointer).expect("a valid description generates");
        made.diagnostics.iter().any(|d| d.pointer.ends_with("/links/next/parameters/id"))
    };
    assert!(!warned("/id"), "a declared property");
    assert!(warned("/idd"), "no property idd");
}

/// A length on a `format: byte` field generates a check of its base64 text.
#[test]
fn a_length_on_base64_bytes_is_checked() {
    let d = doc(&json!({}), &json!({"schemas": {"Token": {"type": "string", "format": "byte", "maxLength": 8}}}));
    let made = crate::generate::generate(&common::write("byte-length", &d, &[]), &Options::default());
    let types = common::file(&made.expect("a valid description generates").files, "types.rs").to_owned();
    assert!(types.contains("::honig::__private::check_len("), "{types}");
}

/// A valid schema whose keywords no generated check enforces still generates: a warning names what goes unchecked. A
/// keyword of another kind than the `type` passes every value and needs no check at all.
#[test]
fn a_schema_that_no_check_enforces_is_a_warning() {
    let generated = |s: Value| {
        let d = doc(&json!({}), &json!({"schemas": {"S": s}}));
        let made = crate::generate::generate(&common::write("unchecked-schema", &d, &[]), &Options::default());
        made.expect("a valid description generates").diagnostics
    };
    let vacuous = generated(json!({"type": "string", "patternProperties": {"^a": {}}}));
    assert!(vacuous.is_empty(), "{vacuous:#?}");
    let dynamic = generated(json!({"$dynamicAnchor": "m", "type": "object", "allOf": [{"$dynamicRef": "#m"}],
        "properties": {"x": {}}, "unevaluatedProperties": false}));
    assert!(
        dynamic.iter().any(|d| d.message.contains("does not check this schema: unevaluatedProperties")),
        "{dynamic:#?}"
    );
}

/// Media types that give one variant name are numbered in the sorted order of the types: a reorder of `content`
/// changes no name.
#[test]
fn a_reorder_of_media_types_changes_no_variant() {
    let variants = |flip: bool| {
        let (plain, charset) = (json!({"schema": {"type": "string"}}), json!({"schema": {"type": "string"}}));
        let content = if flip {
            json!({"text/plain; charset=utf-8": charset, "text/plain": plain})
        } else {
            json!({"text/plain": plain, "text/plain; charset=utf-8": charset})
        };
        let paths = json!({"/a": {"post": {"operationId": "a", "requestBody": {"content": content},
            "responses": {"204": {"description": "ok"}}}}});
        let (api, diags) = common::build(if flip { "media-b" } else { "media-a" }, &doc(&paths, &json!({})), &[]);
        assert!(diags.is_empty(), "{diags:?}");
        let body = api.http.operations[0].body.as_ref().expect("a body");
        let mut out: Vec<(String, String)> =
            body.contents.iter().map(|c| (c.media.clone(), c.variant.words().join("_"))).collect();
        out.sort();
        out
    };
    assert_eq!(variants(false), variants(true));
}

/// Two scopes that are one variant of the scope enum fail generation: numbered by their place in the sorted list, an
/// added scope would rename the others and a `verify_` method would grant another scope than it names.
#[test]
fn two_scopes_of_one_variant_fail_generation() {
    let components = json!({"securitySchemes": {"oauth": {"type": "oauth2", "flows": {"implicit": {
        "authorizationUrl": "https://example.com/auth", "scopes": {"read:user": "a", "read_user": "b"}}}}}});
    let path =
        common::write("sec-scope-variants", &doc(&secured_op(&json!([{"oauth": ["read:user"]}])), &components), &[]);
    let err = crate::generate::generate(&path, &Options::default()).expect_err("must refuse");
    let at = "/components/securitySchemes/oauth/flows/implicit/scopes/read_user";
    assert!(err.iter().any(|d| d.pointer == at), "the second scope: {:#?}", err.0);
}

/// A schema that no value of its type meets generates a type that refuses every value: `generate` fails and points at
/// the schema.
#[test]
fn a_schema_that_no_value_meets_fails_generation() {
    let schemas = [
        ("Price", json!({"type": "integer", "minimum": 10, "maximum": 5})),
        ("Name", json!({"type": "string", "minLength": 5, "maxLength": 2})),
        ("Tags", json!({"type": "array", "items": {"type": "string"}, "minItems": 3, "maxItems": 1})),
    ];
    for (name, schema) in schemas {
        let path = common::write(&format!("empty-{name}"), &doc(&json!({}), &json!({"schemas": {name: schema}})), &[]);
        let err = crate::generate::generate(&path, &Options::default()).expect_err(name);
        let at = format!("/components/schemas/{name}");
        assert!(err.iter().any(|d| d.pointer == at), "{name}: {:#?}", err.0);
    }
}

/// `x-enum-varnames` pairs names with values by position: a list of another length gives a value the name of
/// another, and an edit of the `enum` alone shifts every meaning.
#[test]
fn enum_names_of_another_length_fail_generation() {
    let enums =
        |names: Value| json!({"schemas": {"Level": {"type": "integer", "enum": [1, 2, 3], "x-enum-varnames": names}}});
    for (i, names) in [json!(["Low", "High"]), json!(["Low", "Mid", "High", "Max"]), json!(["Low", 2, "High"])]
        .into_iter()
        .enumerate()
    {
        let path = common::write(&format!("enum-names-{i}"), &doc(&json!({}), &enums(names)), &[]);
        let err = crate::generate::generate(&path, &Options::default()).expect_err("must fail");
        assert!(err.iter().any(|d| d.pointer == "/components/schemas/Level/x-enum-varnames"), "{:#?}", err.0);
    }
    let ok = common::write("enum-names-ok", &doc(&json!({}), &enums(json!(["Low", "Mid", "High"]))), &[]);
    crate::generate::generate(&ok, &Options::default()).expect("one name for each value generates");
}

/// A name in `operations` that no operation has fails generation: it would give an empty module. The count is of the
/// operations that stay.
#[test]
fn an_unknown_operation_id_fails_generation() {
    let ok = json!({"204": {"description": "ok"}});
    let paths = json!({"/a": {"get": {"operationId": "getA", "responses": ok}},
        "/b": {"get": {"operationId": "getB", "responses": ok}}});
    let path = common::write("unknown-operation", &doc(&paths, &json!({})), &[]);
    let only = |names: &[&str]| Options {
        operations: Some(names.iter().map(|n| (*n).to_owned()).collect()),
        ..Options::default()
    };
    let err = crate::generate::generate(&path, &only(&["getA", "getZ"])).expect_err("must fail");
    assert!(err.iter().any(|d| d.message.contains("`getZ`")), "{:#?}", err.0);
    let listed = crate::generate::generate(&path, &only(&["getA"])).expect("generates");
    assert_eq!(listed.operations, 1, "the count is after the filter");
    let all = crate::generate::generate(&path, &Options::default()).expect("generates");
    assert_eq!(all.operations, 2);
}

/// A key of `derives` that names no generated type fails generation: after a rename of a schema the derive (a marker
/// such as a zeroize derive) would vanish without a word. `*` and a type's name stay valid.
#[test]
fn a_derives_key_that_names_no_type_fails_generation() {
    let schema = json!({"type": "object", "properties": {"id": {"type": "string"}}});
    let path = common::write("unknown-derives", &doc(&json!({}), &json!({"schemas": {"Account": schema}})), &[]);
    let with = |key: &str| {
        let mut opts = Options::default();
        opts.rust.derives = vec![(key.to_owned(), vec!["Hash".to_owned()])];
        opts
    };
    let err = crate::generate::generate(&path, &with("Acount")).expect_err("must fail");
    assert!(err.iter().any(|d| d.message.contains("`Acount`")), "{:#?}", err.0);
    crate::generate::generate(&path, &with("Account")).expect("a type's name");
    crate::generate::generate(&path, &with("*")).expect("every type");
}

/// An unquoted YAML `007` under `type: string` is the number 7: the type falls back to a JSON value and refuses
/// `"007"`. `generate` fails and points at the value.
#[test]
fn an_enum_value_outside_the_type_fails_generation() {
    let dir = std::env::temp_dir().join(format!("honig-enum-type-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let spec = dir.join("openapi.yaml");
    std::fs::write(
        &spec,
        "openapi: 3.1.0\ninfo: {title: t, version: '1'}\npaths: {}\ncomponents:\n  schemas:\n    Code:\n      type: string\n      enum: [007, 1.10]\n",
    )
    .unwrap();
    let err = crate::generate::generate(&spec, &Options::default()).expect_err("must fail");
    assert!(err.iter().any(|d| d.pointer == "/components/schemas/Code/enum"), "{:#?}", err.0);
    std::fs::remove_dir_all(&dir).ok();
}

/// A scheme that the generator would read as another fails generation: an API key `in` another place than `header`,
/// `query` or `cookie` meant the header, and a missing `scheme` or `flows` meant a bearer token.
#[test]
fn a_security_scheme_that_is_read_as_another_fails_generation() {
    let schemes = [
        json!({"type": "apiKey", "name": "K", "in": "body"}),
        json!({"type": "apiKey", "name": "K"}),
        json!({"type": "http"}),
        json!({"type": "oauth2"}),
    ];
    for (i, scheme) in schemes.into_iter().enumerate() {
        let components = json!({"securitySchemes": {"local": scheme}});
        let path =
            common::write(&format!("scheme-read-as-{i}"), &doc(&secured_op(&json!([{"local": []}])), &components), &[]);
        let err = crate::generate::generate(&path, &Options::default()).expect_err("must refuse");
        assert!(err.iter().any(|d| d.pointer == "/components/securitySchemes/local"), "{i}: {:#?}", err.0);
        let client = Options { server: false, tests: false, ..Options::default() };
        crate::generate::generate(&path, &client).expect_err("a client fails too");
    }
}

/// A link that sets a parameter its target does not have would have the value skipped: `generate` fails and points at
/// the key. `name` and `in.name` both name a parameter.
#[test]
#[expect(clippy::literal_string_with_formatting_args, reason = "a path template of the description")]
fn a_link_parameter_that_the_target_lacks_fails_generation() {
    let id = json!({"in": "path", "name": "id", "required": true, "schema": {"type": "string"}});
    let link = |key: &str| json!({"next": {"operationId": "get_item", "parameters": {key: "$response.body#/id"}}});
    let paths = |key: &str| {
        json!({"/start": {"get": {"operationId": "start",
            "responses": {"200": {"description": "ok", "links": link(key)}}}},
            "/items/{id}": {"get": {"operationId": "get_item", "parameters": [id],
            "responses": {"204": {"description": "ok"}}}}})
    };
    for (key, fine) in [("id", true), ("path.id", true), ("idd", false)] {
        let path = common::write(&format!("link-key-{key}"), &doc(&paths(key), &json!({})), &[]);
        let made = crate::generate::generate(&path, &Options::default());
        if fine {
            made.expect(key);
        } else {
            let err = made.expect_err(key);
            assert!(err.iter().any(|d| d.pointer.ends_with("/links/next/parameters/idd")), "{:#?}", err.0);
        }
    }
}

/// A field whose `default` is no string, number or boolean has none: that is a warning, not silence. A literal one
/// stays quiet.
#[test]
fn a_default_that_no_value_writes_is_a_warning() {
    let holder = json!({"type": "object", "properties": {
        "options": {"type": "object", "properties": {"a": {"type": "string"}}, "default": {"a": "x"}},
        "limit": {"type": "integer", "default": 10}}});
    let path = common::write("dropped-default", &doc(&json!({}), &json!({"schemas": {"Holder": holder}})), &[]);
    let made = crate::generate::generate(&path, &Options::default()).expect("generates");
    let warned: Vec<&str> = made.diagnostics.iter().map(|d| d.message.as_str()).collect();
    let at = |p: &str| made.diagnostics.iter().any(|d| d.pointer == p && d.level == crate::diag::Level::Warning);
    assert!(at("/components/schemas/Holder/properties/options/default"), "{warned:?}");
    assert!(!at("/components/schemas/Holder/properties/limit/default"), "{warned:?}");
}

/// A keyword that no check runs is a warning, not silence: an upload is bytes. A `text/plain` body takes the type of its
/// schema, which checks it, so it needs none.
#[test]
fn a_limit_on_a_binary_body_is_a_warning_and_on_a_text_body_a_check() {
    let body = |media: &str, schema: Value| {
        json!({"/a": {"post": {"operationId": "a", "requestBody": {"content": {media: {"schema": schema}}},
            "responses": {"204": {"description": "ok"}}}}})
    };
    let text = body("text/plain", json!({"type": "string", "maxLength": 140}));
    let upload = body("application/octet-stream", json!({"type": "string", "format": "binary", "maxLength": 1000}));
    for (name, paths, warns) in [("text", text, false), ("upload", upload, true)] {
        let path = common::write(&format!("unchecked-{name}"), &doc(&paths, &json!({})), &[]);
        let made = crate::generate::generate(&path, &Options::default()).expect("generates");
        let warned = made
            .diagnostics
            .iter()
            .any(|d| d.pointer.ends_with("/schema/maxLength") && d.message.contains("not checked"));
        assert_eq!(warned, warns, "{name}: {:#?}", made.diagnostics);
    }
}

/// A `text/plain` body with a string `format` takes the type that checks it. A schema of another type is not checked
/// against the text: a warning says so.
#[test]
fn a_text_body_is_checked_or_warned_about() {
    let body = |schema: Value| {
        json!({"/a": {"post": {"operationId": "a", "requestBody": {"content": {"text/plain": {"schema": schema}}},
            "responses": {"204": {"description": "ok"}}}}})
    };
    let uuid = doc(&body(json!({"type": "string", "format": "uuid"})), &json!({}));
    let files = common::generate("text-uuid", &uuid, &Options::default());
    assert!(common::file(&files, "operations.rs").contains("UuidStr"), "the body is a checked uuid");
    let path = common::write("text-integer", &doc(&body(json!({"type": "integer", "minimum": 5})), &json!({})), &[]);
    let made = crate::generate::generate(&path, &Options::default()).expect("generates");
    let warned = made.diagnostics.iter().filter(|d| d.pointer.ends_with("/schema/type")).count();
    assert_eq!(warned, 1, "{:#?}", made.diagnostics);
}

/// A `2XX` response is a success as `200` is: its required header draws no warning of an error response, and the
/// server keeps the default `error_response`.
#[test]
fn a_2xx_response_is_a_success() {
    for status in ["200", "2XX"] {
        let paths = json!({"/a": {"get": {"operationId": "a", "responses": {status: {"description": "ok",
            "headers": {"X-Id": {"required": true, "schema": {"type": "string"}}}}}}}});
        let path = common::write(&format!("success-{status}"), &doc(&paths, &json!({})), &[]);
        let made = crate::generate::generate(&path, &Options::default()).expect("generates");
        assert!(
            made.diagnostics.iter().all(|d| !d.message.contains("error response")),
            "{status}: {:#?}",
            made.diagnostics
        );
        let server = common::file(
            &common::generate(&format!("success-server-{status}"), &doc(&paths, &json!({})), &Options::default()),
            "server.rs",
        )
        .to_owned();
        assert!(server.contains("rejection.response()"), "{status}: the default `error_response`");
    }
}

/// A scheme that refers to a schema, to itself or to nothing defines no check: a server would skip it.
#[test]
fn a_scheme_reference_to_no_scheme_fails_generation() {
    let refs = ["#/components/schemas/S", "#/components/securitySchemes/local", "#/components/securitySchemes/nope"];
    for r in refs {
        let components = json!({"schemas": {"S": {"type": "string"}}, "securitySchemes": {"local": {"$ref": r}}});
        let path = common::write("sec-scheme-ref", &doc(&secured_op(&json!([{"local": []}])), &components), &[]);
        crate::generate::generate(&path, &Options::default()).expect_err(r);
    }
    let components = json!({"securitySchemes": {"local": {"$ref": "#/components/securitySchemes/real"},
        "real": {"type": "http", "scheme": "bearer"}}});
    let d = doc(&secured_op(&json!([{"local": []}])), &components);
    let consts = security_consts(&common::generate("sec-scheme-ref-real", &d, &Options::default()));
    assert_eq!(consts.len(), 1);
    assert!(!is_public(&consts[0]), "{}", consts[0]);
}

// `content` (3.2 Request Body / Response): "For requests that match multiple
// keys, only the most specific key is applicable."

/// Media types in the order the generated code checks them.
fn checked_media(code: &str, candidates: &[&str]) -> Vec<String> {
    let s = squash(code);
    let mut found: Vec<(usize, String)> = candidates
        .iter()
        .filter_map(|m| s.find(&format!("media_matches(req.content_type(),\"{m}\")")).map(|i| (i, (*m).to_owned())))
        .collect();
    found.sort();
    found.into_iter().map(|(_, m)| m).collect()
}

#[test]
fn most_specific_request_media_type_is_tried_first_in_any_order() {
    let keys = ["*/*", "text/*", "text/plain", "text/plain; charset=utf-8"];
    let orders = [[0, 1, 2, 3], [3, 2, 1, 0], [1, 0, 3, 2], [2, 0, 3, 1]];
    for order in orders {
        let content: serde_json::Map<String, Value> =
            order.iter().map(|&i| (keys[i].to_owned(), json!({"schema": {"type": "string"}}))).collect();
        let d = doc(
            &json!({"/u": {"post": {
                "operationId": "u",
                "requestBody": {"required": true, "content": content},
                "responses": {"204": {"description": "ok"}}
            }}}),
            &json!({}),
        );
        let seen = checked_media(&server_of("media-order", &d), &keys);
        let mut sorted = seen.clone();
        sorted.sort_by_key(|m| keys.iter().position(|k| k == m).map(std::cmp::Reverse));
        assert_eq!(seen, sorted, "document order {order:?}");
    }
}

// `pattern` (JSON Schema 2020-12 section 6.3.3): checked by honig's `regress` feature; one it cannot compile
// is an error, and generation refuses the document.

#[test]
fn a_pattern_regress_cannot_compile_is_reported() {
    let (_, w) = common::build(
        "pattern-bad",
        &common::spec(
            "3.1.0",
            &json!({}),
            &json!({"schemas": {
                "Bad": {"type": "string", "pattern": "[z-a]"},
                "Map": {"type": "object", "patternProperties": {"(unclosed": {"type": "string"}}},
                "Good": {"type": "string", "pattern": "^\\p{Lu}[a-z]*$"}
            }}),
        ),
        &[],
    );
    assert!(w.iter().any(|m| m.starts_with("/components/schemas/Bad/pattern:") && m.contains("[z-a]")), "{w:#?}");
    assert!(w.iter().any(|m| m.contains("/patternProperties/(unclosed:")), "{w:#?}");
    assert!(!w.iter().any(|m| m.contains("Good")), "a Unicode property escape is valid ECMA-262: {w:#?}");
}

/// A type whose `pattern` does not compile would accept every value: generation refuses the document and points to it.
#[test]
fn a_pattern_that_does_not_compile_fails_generation() {
    let doc = |pattern: &str| {
        common::spec("3.1.0", &json!({}), &json!({"schemas": {"Code": {"type": "string", "pattern": pattern}}}))
    };
    let path = common::write("pattern-refused", &doc("[z-a]"), &[]);
    let Err(d) = crate::generate::generate(&path, &Options::default()) else {
        panic!("a pattern that does not compile must fail generation")
    };
    assert!(d.has_errors(), "{d:?}");
    assert!(d.iter().any(|e| e.pointer == "/components/schemas/Code/pattern" && e.message.contains("[z-a]")), "{d:?}");
    let ok = common::write("pattern-accepted", &doc("^[A-Z]+$"), &[]);
    assert!(crate::generate::generate(&ok, &Options::default()).is_ok());
}

/// 3.2 section 4.12.5 Style Values: "Combinations not represented in this table are
/// not permitted."
#[test]
fn a_style_outside_its_locations_is_an_error() {
    let param = |name: &str, at: &str, style: &str| json!({"name": name, "in": at, "style": style, "schema": {"type": "array", "items": {"type": "string"}}});
    let (_, w) = common::build(
        "style-location",
        &doc(
            &json!({"/a/{p}": {"get": {
                "operationId": "a",
                "parameters": [
                    {"name": "p", "in": "path", "required": true, "style": "label", "schema": {"type": "string"}},
                    param("m", "query", "matrix"),
                    param("d", "header", "deepObject"),
                    param("f", "query", "form"),
                    param("c", "cookie", "cookie")
                ],
                "responses": {"204": {"description": "ok"}}
            }}}),
            &json!({}),
        ),
        &[],
    );
    let bad: Vec<&String> = w.iter().filter(|m| m.contains("is not allowed for `in:")).collect();
    assert_eq!(bad.len(), 2, "{w:#?}");
    assert!(
        bad.iter().any(|m| m.contains("/parameters/1/style:") && m.contains("\"matrix\"") && m.contains("in: query"))
    );
    assert!(bad.iter().any(|m| m.contains("/parameters/2/style:") && m.contains("\"deepObject\"")));
}

// Parameter and Header Object `content` (3.2): "the media type ... and the
// schema" describe the value whatever the media type.

#[test]
fn a_text_plain_response_header_keeps_its_schema() {
    let (api, _) = common::build(
        "header-text",
        &doc(
            &json!({"/a": {"get": {"operationId": "a", "responses": {"200": {
                "description": "ok",
                "headers": {
                    "X-Count": {"required": true, "content": {"text/plain": {"schema": {"type": "integer"}}}},
                    "X-Meta": {"content": {"application/json": {"schema": {"type": "object", "properties": {"n": {"type": "integer"}}}}}}
                }
            }}}}}),
            &json!({}),
        ),
        &[],
    );
    let headers = &api.http.operations[0].responses[0].headers;
    let count = headers.iter().find(|h| h.wire == "X-Count").expect("X-Count");
    assert_eq!(count.ty, crate::ir::types::Ty::I64, "not untyped JSON");
    assert!(!count.json, "text/plain carries the value's own text");
    assert!(headers.iter().find(|h| h.wire == "X-Meta").expect("X-Meta").json);
}

/// 3.2 `components.mediaTypes`: a `content` entry may be a `$ref`.
#[test]
fn a_content_entry_may_reference_a_reusable_media_type() {
    let (api, w) = common::build(
        "media-ref",
        &doc(
            &json!({"/a": {"get": {
                "operationId": "a",
                "parameters": [{"name": "f", "in": "query", "content": {"application/json": {"$ref": "#/components/mediaTypes/Filter"}}}],
                "responses": {"200": {
                    "description": "ok",
                    "headers": {"X-N": {"required": true, "content": {"text/plain": {"$ref": "#/components/mediaTypes/Count"}}}}
                }}
            }}}),
            &json!({"mediaTypes": {
                "Filter": {"schema": {"type": "string", "maxLength": 4}},
                "Count": {"schema": {"type": "integer"}}
            }}),
        ),
        &[],
    );
    assert!(w.iter().all(|m| !m.contains("does not resolve")), "{w:#?}");
    let p = &api.http.operations[0].params[0];
    let crate::ir::types::Ty::Option(named) = &p.ty else { panic!("optional: {:?}", p.ty) };
    let crate::ir::types::Ty::Named(id) = named.as_ref() else { panic!("a checked type: {named:?}") };
    let crate::ir::types::Kind::Checked(c) = &api.types.get(*id).kind else { panic!("checked") };
    assert_eq!((&c.base, c.constraints.max_length), (&crate::ir::types::Ty::String, Some(4)));
    assert_eq!(api.http.operations[0].responses[0].headers[0].ty, crate::ir::types::Ty::I64);
}

#[test]
fn a_content_parameter_is_validated_by_its_media_type_schema() {
    let (api, _) = common::build(
        "param-content",
        &doc(
            &json!({"/a": {"get": {
                "operationId": "a",
                "parameters": [{"name": "filter", "in": "query", "content": {"application/json": {"schema": {"type": "string", "maxLength": 8}}}}],
                "responses": {"204": {"description": "ok"}}
            }}}),
            &json!({}),
        ),
        &[],
    );
    assert_eq!(api.http.operations[0].params[0].constraints.max_length, Some(8));
}

// JSON Schema drafts the generator claims to support (06, 07, 2019-09).

fn types_of(test: &str, schemas: &Value) -> (String, Vec<String>) {
    let d = common::spec("3.1.0", &json!({}), &json!({"schemas": schemas}));
    let path = common::write(test, &d, &[]);
    let g = crate::generate::generate(&path, &Options::default()).expect("generates");
    let warnings = g.diagnostics.iter().map(|d| format!("{}: {}", d.pointer, d.message)).collect();
    (squash(common::file(&g.files, "types.rs")), warnings)
}

#[test]
fn draft_07_tuple_with_additional_items_false_is_a_closed_tuple() {
    let (types, _) = types_of(
        "draft07-tuple",
        &json!({"T": {
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "array",
            "items": [{"type": "string"}, {"type": "integer"}],
            "additionalItems": false,
            "minItems": 2
        }}),
    );
    assert!(types.contains("pubtypeT=(String,i64);"), "{types}");
}

#[test]
fn draft_2019_09_recursive_ref_is_recursive() {
    let (types, warnings) = types_of(
        "recursive-ref",
        &json!({"Tree": {
            "$schema": "https://json-schema.org/draft/2019-09/schema",
            "$recursiveAnchor": true,
            "type": "object",
            "properties": {"kids": {"type": "array", "items": {"$recursiveRef": "#"}}}
        }}),
    );
    assert!(types.contains("#[optional]pubkids:Vec<Self>"), "{types}");
    assert!(warnings.is_empty(), "{warnings:#?}");
}

/// 2019-09 section 8.2.4.2: the outermost `$recursiveAnchor` of the dynamic scope
/// wins. So the recursion of an extension stays in the extension.
#[test]
fn draft_2019_09_recursive_ref_follows_the_extending_schema() {
    let (types, warnings) = types_of(
        "recursive-extend",
        &json!({
            "Tree": {
                "$schema": "https://json-schema.org/draft/2019-09/schema",
                "$recursiveAnchor": true,
                "type": "object",
                "properties": {"kids": {"type": "array", "items": {"$recursiveRef": "#"}}}
            },
            "LabeledTree": {
                "$recursiveAnchor": true,
                "$ref": "#/components/schemas/Tree",
                "properties": {"label": {"type": "string"}},
                "required": ["label"]
            },
            "Forest": {"type": "array", "items": {"$ref": "#/components/schemas/Tree"}}
        }),
    );
    assert!(warnings.is_empty(), "{warnings:#?}");
    let body = |name: &str| {
        types.split(&format!("pubstruct{name}{{")).nth(1).and_then(|r| r.split_once('}')).map(|(b, _)| b.to_owned())
    };
    let tree = body("Tree").expect("Tree");
    assert!(tree.contains("#[optional]pubkids:Vec<Self>"), "{tree}");
    let labeled = body("LabeledTree").expect("LabeledTree");
    assert!(labeled.contains("#[optional]pubkids:Vec<Self>"), "an extension's children are extensions: {labeled}");
    assert!(labeled.contains("publabel:String"), "{labeled}");
    assert!(types.contains("pubtypeForest=Vec<Tree>;"), "no anchor outside: static: {types}");
}

/// Only a schema whose `$recursiveRef` lands on an anchor resolves dynamically: a `$ref` from an anchored schema to
/// any other stays a reference.
#[test]
fn draft_2019_09_a_static_target_stays_a_reference() {
    let (types, _) = types_of(
        "recursive-static",
        &json!({
            "Tree": {
                "$schema": "https://json-schema.org/draft/2019-09/schema",
                "$recursiveAnchor": true,
                "type": "object",
                "properties": {"meta": {"$ref": "#/components/schemas/Meta"}, "node": {"$ref": "#/components/schemas/Node"}}
            },
            "Meta": {"type": "object", "properties": {"at": {"type": "string"}}},
            "Node": {"$id": "https://t.test/node", "type": "object", "properties": {"next": {"$recursiveRef": "#"}}}
        }),
    );
    assert!(types.contains("pubmeta:Meta") && types.contains("pubnode:Node"), "{types}");
}

/// An extension that has an `allOf` of its own keeps it beside the copy of what it extends.
#[test]
fn draft_2019_09_recursive_extension_keeps_its_own_all_of() {
    let (types, _) = types_of(
        "recursive-extend-all-of",
        &json!({
            "Tree": {
                "$schema": "https://json-schema.org/draft/2019-09/schema",
                "$recursiveAnchor": true,
                "type": "object",
                "properties": {"kids": {"type": "array", "items": {"$recursiveRef": "#"}}}
            },
            "LabeledTree": {
                "$recursiveAnchor": true,
                "$ref": "#/components/schemas/Tree",
                "allOf": [{"properties": {"color": {"type": "string"}}}],
                "properties": {"label": {"type": "string"}}
            }
        }),
    );
    let labeled = types.split("pubstructLabeledTree{").nth(1).and_then(|r| r.split_once('}')).map(|(b, _)| b);
    let labeled = labeled.unwrap_or_else(|| panic!("{types}"));
    assert!(labeled.contains("pubcolor:String") && labeled.contains("pubkids:Vec<Self>"), "{labeled}");
}

#[test]
fn draft_04_boolean_exclusive_bounds_still_bound_in_v31() {
    let mut diags = crate::diag::Diagnostics::default();
    let up = crate::upgrade::to_v32(
        common::spec(
            "3.1.0",
            &json!({}),
            &json!({"schemas": {"N": {"type": "integer", "minimum": 1, "exclusiveMinimum": true}}}),
        ),
        &mut diags,
    )
    .expect("upgrades");
    assert_eq!(up["components"]["schemas"]["N"], json!({"type": "integer", "exclusiveMinimum": 1}));
}

// Runtime expressions (3.2 "Runtime Expressions"): every form of the ABNF.

/// The grammar of the spec: `$url`, `$method`, `$statusCode` and `$request.` /
/// `$response.` with `header.`, `query.`, `path.` or `body`.
const EXPRESSIONS: &[&str] = &[
    "$url",
    "$method",
    "$statusCode",
    "$request.header.X-Trace",
    "$request.query.q",
    "$request.path.id",
    "$request.body",
    "$request.body#/user/uuid",
    "$response.header.Location",
    "$response.body",
    "$response.body#/id",
];

fn linked(params: &Value) -> Value {
    doc(
        &json!({
            "/things/{id}": {
                "post": {
                    "operationId": "make",
                    "parameters": [
                        {"name": "id", "in": "path", "required": true, "schema": {"type": "string"}},
                        {"name": "q", "in": "query", "schema": {"type": "string"}},
                        {"name": "X-Trace", "in": "header", "schema": {"type": "string"}}
                    ],
                    "requestBody": {"content": {"application/json": {"schema": {"type": "object"}}}},
                    "responses": {"201": {"description": "made", "links": {"Next": {"operationId": "take", "parameters": params}}}}
                }
            },
            "/take": {"get": {
                "operationId": "take",
                "parameters": [{"name": "v", "in": "query", "schema": {"type": "string"}}],
                "responses": {"204": {"description": "ok"}}
            }}
        }),
        &json!({}),
    )
}

#[test]
fn every_runtime_expression_form_reaches_a_value_source() {
    for e in EXPRESSIONS {
        let ops = squash(&operations_of("expr-forms", &linked(&json!({"v": e}))));
        let init =
            ops.split("Self{v:").nth(1).map(|r| r.split_once("},").map_or(r, |(head, _)| head)).unwrap_or_default();
        assert!(!init.starts_with("None"), "{e} is never evaluated: {init}");
        let static_source = e.starts_with("$request.path.")
            || e.starts_with("$request.query.")
            || e.starts_with("$request.header.")
            || e.starts_with("$response.body");
        if !static_source {
            assert!(init.contains(&format!("exchange.eval(\"{e}\")")), "{e}: {init}");
            assert!(ops.contains("pubfnfrom_exchange("), "{e} needs the exchange");
        }
    }
}

#[test]
fn embedded_expressions_in_a_template_are_each_evaluated() {
    let ops = squash(&operations_of("expr-template", &linked(&json!({"v": "/x/{$request.path.id}?s={$statusCode}"}))));
    assert!(ops.contains(r#"::honig::__private::template(&["/x/","?s=",""],vec!["#), "{ops}");
    assert!(ops.contains(r#"exchange.eval("$statusCode")"#), "{ops}");
}

#[test]
fn callback_url_expression_is_evaluated() {
    let d = doc(
        &json!({"/subscribe": {"post": {
            "operationId": "subscribe",
            "parameters": [{"name": "tenant", "in": "query", "schema": {"type": "string"}}],
            "requestBody": {"content": {"application/json": {"schema": {"type": "object"}}}},
            "responses": {"201": {"description": "ok"}},
            "callbacks": {"onEvent": {"{$request.body#/callbackUrl}/t/{$request.query.tenant}": {"post": {
                "operationId": "onEvent",
                "responses": {"204": {"description": "ok"}}
            }}}}
        }}}),
        &json!({}),
    );
    let ops = squash(&operations_of("callback-url", &d));
    assert!(ops.contains("pubmodcallbacks{"), "{ops}");
    assert!(
        ops.contains("pubfnon_event_url(request:&super::Params,exchange:&::honig::Exchange<'_>)->Option<String>"),
        "{ops}"
    );
    assert!(ops.contains(r#"exchange.eval("$request.body#/callbackUrl")"#), "{ops}");
    assert!(ops.contains("request.tenant"), "{ops}");
}

#[test]
fn template_splitting_keeps_other_braces_literal() {
    // Through a link: the literal parts appear verbatim.
    let ops = squash(&operations_of("expr-braces", &linked(&json!({"v": "{a}{$method}{"}))));
    assert!(ops.contains(r#"::honig::__private::template(&["{a}","{"],vec![exchange.eval("$method")])"#), "{ops}");
}

// Security Requirement Object (3.1+): "For other security scheme types, the
// array MAY contain a list of role names which are required for the
// execution". Several requirements are alternatives (OR).

fn fine_grained() -> Value {
    doc(
        &json!({"/issues": {
            "get": {
                "operationId": "listIssues",
                "security": [{"pat": ["issues:read"]}, {"pat": ["issues:write"]}],
                "responses": {"204": {"description": "ok"}}
            },
            "post": {
                "operationId": "createIssue",
                "security": [{"oauth": ["repo"]}],
                "responses": {"204": {"description": "ok"}}
            }
        }}),
        &json!({"securitySchemes": {
            "pat": {"type": "http", "scheme": "bearer"},
            "oauth": {"type": "oauth2", "flows": {"clientCredentials": {
                "tokenUrl": "https://example.com/token",
                "scopes": {"repo": "Full control of repositories", "read:org": "Read org membership"}
            }}}
        }}),
    )
}

/// Returns the variants of the enum `name` in the squashed server.
fn variants_of<'a>(server: &'a str, name: &str) -> Vec<&'a str> {
    let Some((decl, _)) = server.split(&format!("pubenum{name}{{")).nth(1).and_then(|r| r.split_once('}')) else {
        panic!("no {name}");
    };
    decl.split(',').filter(|v| !v.is_empty()).map(|v| v.rsplit("*/").next().unwrap_or(v)).collect()
}

/// Each scheme has its own scope enum: a token of one scheme cannot grant a scope of another.
#[test]
fn every_scope_and_role_is_a_variant_of_its_schemes_enum() {
    let server = squash(&server_of("scope-enum", &fine_grained()));
    let ops = squash(&operations_of("scope-enum", &fine_grained()));
    let pat = variants_of(&ops, "PatScope");
    let oauth = variants_of(&ops, "OauthScope");
    assert!(pat.iter().any(|v| v.ends_with("IssuesRead")) && pat.iter().any(|v| v.ends_with("IssuesWrite")), "{pat:?}");
    assert!(!pat.iter().any(|v| v.ends_with("Repo")), "no scope of `oauth` in `PatScope`: {pat:?}");
    assert!(oauth.iter().any(|v| v.ends_with("Repo")) && oauth.iter().any(|v| v.ends_with("ReadOrg")), "{oauth:?}");
    assert!(!oauth.iter().any(|v| v.ends_with("IssuesRead")), "{oauth:?}");
    assert!(ops.contains("///Fullcontrolofrepositories"), "OAuth2 descriptions become docs");
    for (name, v) in [("issues:read", "IssuesRead"), ("issues:write", "IssuesWrite"), ("read:org", "ReadOrg")] {
        assert!(ops.contains(&format!("Self::{v}=>\"{name}\"")), "{v} as_str");
        assert!(ops.contains(&format!("\"{name}\"=>Some(Self::{v})")), "{v} from_name");
    }
    assert!(!ops.contains("pubenumScope{"), "no enum that merges the schemes");
    assert!(server.contains("Option<::honig::Granted<Self::Pat,PatScope>>"), "verify_pat grants `PatScope`");
    assert!(server.contains("Option<::honig::Granted<Self::Oauth,OauthScope>>"), "verify_oauth grants `OauthScope`");
}

#[test]
fn or_of_roles_on_one_scheme_keeps_both_alternatives() {
    let consts = security_consts(&common::generate("scope-or", &fine_grained(), &Options::default()));
    assert!(
        consts.contains(
            &r#"&[&[(::honig::__private::Scheme::Bearer("pat"),&["issues:read"])],&[(::honig::__private::Scheme::Bearer("pat"),&["issues:write"])]]"#
                .to_owned()
        ),
        "{consts:#?}"
    );
}

#[test]
fn no_scopes_means_no_scope_enum() {
    let server = squash(&server_of("scope-none", &doc(&secured_op(&json!([{"local": []}])), &bearer())));
    assert!(!server.contains("pubenumLocalScope"), "an empty enum would be uninhabited noise");
    assert!(server.contains("Option<Self::Local>"), "a scheme without scopes returns its value alone");
}

/// JSON Schema 2020-12 section 6.1.3: a `const` property is valid only at its value. The error body that the server
/// writes for its own refusals gives each required `const` its value, also one named like a field it fills (`type`).
/// An optional one stays out.
#[test]
fn a_required_const_of_an_error_schema_gets_its_value() {
    let d = doc(
        &json!({"/orders": {"post": {
            "operationId": "createOrder",
            "responses": {
                "201": {"description": "Created"},
                "4XX": {"description": "Refused", "content": {"application/problem+json": {
                    "schema": {"$ref": "#/components/schemas/Refusal"}}}}
            }
        }}}),
        &json!({"schemas": {"Refusal": {
            "type": "object",
            "required": ["detail", "object", "type"],
            "properties": {
                "detail": {"type": "string"},
                "object": {"type": "string", "const": "error"},
                "type": {"type": "string", "const": "https://shop.example.com/errors/refused"},
                "version": {"type": "integer", "const": 2}
            }
        }}}),
    );
    let server = squash(&server_of("error-const", &d));
    let fields = server.split_once("fields:&[").and_then(|(_, rest)| rest.split_once(']')).expect("fields").0;
    assert_eq!(
        fields,
        r#"("detail",::honig::ProblemMember::Detail),("object",::honig::ProblemMember::Const("\"error\"")),("type",::honig::ProblemMember::Const("\"https://shop.example.com/errors/refused\""))"#,
    );
}

/// An error body whose fields no standard names (a token endpoint's `{error, error_description}` with a required enum)
/// makes the generated `Api` require `error_response`, which maps it. `generate` reports nothing.
#[test]
fn an_unfillable_error_body_requires_error_response() {
    let d = doc(
        &json!({"/token": {"post": {
            "operationId": "token",
            "responses": {
                "200": {"description": "Tokens"},
                "400": {"description": "Refused", "content": {"application/json": {"schema": {
                    "type": "object",
                    "required": ["error", "error_description"],
                    "properties": {
                        "error": {"type": "string", "enum": ["invalid_grant", "invalid_request"]},
                        "error_description": {"type": "string"}
                    }
                }}}}
            }
        }}}),
        &json!({}),
    );
    let spec = common::write("error-unfillable", &d, &[]);
    let generated = crate::generate::generate(&spec, &Options::default()).expect("generates");
    assert!(generated.diagnostics.is_empty(), "{:?}", generated.diagnostics);
    let server = squash(common::file(&generated.files, "server.rs"));
    assert!(
        server.contains("fnerror_response(rejection:::honig::Rejection<Operation>)->::honig::Response;"),
        "{server}"
    );
}

/// `allOf: [$ref: Base, {type: object, properties: ...}]` merges into one struct that contains every member: its fields
/// are checked as typed. Two members that both declare `name` merge too, and the
/// body also decodes as each member: one struct would keep only one of their schemas.
#[test]
fn an_all_of_that_one_struct_cannot_hold_checks_each_member() {
    let body = json!({"/products": {"post": {
        "operationId": "createProduct",
        "requestBody": {"required": true, "content": {"application/json": {
            "schema": {"$ref": "#/components/schemas/Product"}}}},
        "responses": {"201": {"description": "Created"}}
    }}});
    let generated = |test: &str, second: Value| {
        let schemas = json!({"schemas": {
            "Base": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string", "maxLength": 9}}},
            "Product": {"allOf": [{"$ref": "#/components/schemas/Base"}, second]}
        }});
        let files = common::generate(test, &doc(&body, &schemas), &Options::default());
        [common::file(&files, "types.rs"), common::file(&files, "operations.rs")].concat()
    };
    let typed = generated("all-of-typed", json!({"type": "object", "properties": {"sku": {"type": "string"}}}));
    let lossy = generated("all-of-lossy", json!({"type": "object", "properties": {"name": {"maxLength": 5}}}));
    assert!(!typed.contains(".decode_ref::<") && lossy.contains(".decode_ref::<"), "{typed}\n{lossy}");
}

/// An error body of RFC 9457 `problem+json` is filled by the RFC's member names: the generated `Api` gives
/// `error_response` a default, and no impl needs to write it.
#[test]
fn a_problem_json_error_body_needs_no_error_response() {
    let d = doc(
        &json!({"/orders": {"post": {
            "operationId": "createOrder",
            "responses": {
                "201": {"description": "Created"},
                "4XX": {"description": "Refused", "content": {"application/problem+json": {"schema": {
                    "type": "object",
                    "properties": {"title": {"type": "string"}, "status": {"type": "integer"}, "detail": {"type": "string"}}
                }}}}
            }
        }}}),
        &json!({}),
    );
    let server = squash(&server_of("error-problem", &d));
    assert!(
        server.contains(
            "fnerror_response(rejection:::honig::Rejection<Operation>)->::honig::Response{rejection.response()}"
        ),
        "{server}"
    );
}

/// Two scheme names that differ only in case or separators map to one Rust item. Numbering them would follow the
/// document order, and a reorder would swap their validators: generation fails and names both.
#[test]
fn two_schemes_with_one_rust_name_are_an_error() {
    let key = |name: &str| json!({"type": "apiKey", "in": "header", "name": name});
    let (_, w) = common::build(
        "scheme-collision",
        &doc(&json!({}), &json!({"securitySchemes": {"api-key": key("X-A"), "api_key": key("X-B")}})),
        &[],
    );
    assert!(
        w.iter().any(|m| m.starts_with("/components/securitySchemes/api_key:")
            && m.contains("`api-key` and `api_key` map to the same item")),
        "{w:#?}"
    );
}

/// The client picks one of several operation servers by name. One with only a `url` is named after it: a reorder
/// keeps each name with its server. Two of one name fail generation, since a position would follow the order.
#[test]
fn an_operation_server_without_a_name_is_named_by_its_url() {
    let names = |servers: Value| {
        let paths = json!({"/a": {"get": {"operationId": "a", "servers": servers,
            "responses": {"204": {"description": "ok"}}}}});
        let (api, w) = common::build("operation-servers", &doc(&paths, &json!({})), &[]);
        let names: Vec<Option<String>> = api.http.operations[0].servers.iter().map(|s| s.name.clone()).collect();
        (names, w)
    };
    let (one, two) = (json!({"url": "https://one.example"}), json!({"url": "https://two.example/v1"}));
    let (forward, w) = names(json!([one, two]));
    assert_eq!(forward, [Some("one example".to_owned()), Some("two example v1".to_owned())], "{w:#?}");
    assert!(w.iter().all(|m| !m.contains("several servers")), "{w:#?}");
    let (back, _) = names(json!([two, one]));
    assert_eq!(back, [Some("two example v1".to_owned()), Some("one example".to_owned())]);
    let (_, w) = names(json!([one, one]));
    assert!(w.iter().any(|m| m.contains("several servers need a distinct `name`")), "{w:#?}");
}

/// Each diagnostic points at the keyword that causes it, and says what is wrong.
#[test]
fn each_diagnostic_points_at_its_keyword() {
    let mut failed: Vec<&str> = vec![];
    let body = |media: Value| {
        json!({"/a": {"post": {"operationId": "a", "requestBody": {"content": {"application/jsonl": media}},
            "responses": {"204": {"description": "ok"}}}}})
    };
    // A problem of `itemSchema` is at `itemSchema`.
    let (_, w) = common::build(
        "item-schema",
        &doc(&body(json!({"itemSchema": {"$ref": "#/components/schemas/Nope"}})), &json!({})),
        &[],
    );
    failed.extend(
        (!(w.iter().any(|m| m.contains("/itemSchema/$ref:")) && w.iter().all(|m| !m.contains("/schema/$ref:"))))
            .then_some("itemSchema"),
    );
    // A boolean `exclusiveMinimum` in 3.1 is converted with a word.
    let exclusive = json!({"schemas": {"N": {"type": "integer", "minimum": 1, "exclusiveMinimum": true}}});
    let (_, w) = common::build("exclusive-bool", &common::spec("3.1.0", &json!({}), &exclusive), &[]);
    failed.extend(
        (!(w.iter().any(|m| m.starts_with("/components/schemas/N/exclusiveMinimum:")))).then_some("exclusiveMinimum"),
    );
    // A reference of a scheme that no build fetches says so, not that a feature is missing.
    let ftp = json!({"schemas": {"F": {"$ref": "ftp://example.com/f.yaml"}}});
    let (_, w) = common::build("ftp-ref", &doc(&json!({}), &ftp), &[]);
    failed.extend(
        (!(w.iter().any(|m| m.contains("is not supported")) && w.iter().all(|m| !m.contains("remote-refs"))))
            .then_some("scheme"),
    );
    // A schema that refers back to itself outside `components/schemas` is unchecked, with a word.
    let x = "#/components/schemas/A/properties/x";
    let looped =
        json!({"schemas": {"A": {"type": "object", "properties": {"x": {"type": "array", "items": {"$ref": x}}}}}});
    let (_, w) = common::build("pointer-cycle", &doc(&json!({}), &looped), &[]);
    failed.extend((!(w.iter().any(|m| m.contains("refers back to itself")))).then_some("cycle"));
    // An `if` without `type` only answers whether a value matches: no warning, and none at a pointer outside the
    // description.
    let cond = json!({"schemas": {"C": {"oneOf": [{"type": "object"}, {"type": "string"}],
        "if": {"properties": {"a": {"const": 1}}, "minProperties": 0}, "then": {"required": ["b"]}}}});
    let (_, w) = common::build("if-no-type", &doc(&json!({}), &cond), &[]);
    failed.extend((!(w.iter().all(|m| !m.contains("x-honig-not")))).then_some("queued if"));
    assert!(failed.is_empty(), "{failed:?}");
}

/// A schema under `dependentSchemas`, `unevaluatedProperties`, `unevaluatedItems` or `contentSchema` is validated as
/// any other: its reference that does not resolve is an error at its pointer.
#[test]
fn every_subschema_is_validated() {
    let nope = json!({"$ref": "#/components/schemas/Nope"});
    for key in ["dependentSchemas", "unevaluatedProperties", "unevaluatedItems", "contentSchema"] {
        let sub = if key == "dependentSchemas" { json!({"a": nope}) } else { nope.clone() };
        let schema = json!({"schemas": {"S": {"type": "object", key: sub}}});
        let (_, w) = common::build("validated-sub", &doc(&json!({}), &schema), &[]);
        let at = if key == "dependentSchemas" { format!("/{key}/a/$ref:") } else { format!("/{key}/$ref:") };
        assert!(w.iter().any(|m| m.starts_with("/components/schemas/S") && m.contains(&at)), "{key}: {w:#?}");
    }
}

/// An `Accept`, `Content-Type` or required `Authorization` header parameter is ignored (OAS 3.2 section 4.12.2): one
/// warning each says so, and generation goes on.
#[test]
fn an_ignored_header_parameter_is_a_warning() {
    let param = |name: &str| json!({"name": name, "in": "header", "required": true, "schema": {"type": "string"}});
    let params = [param("Accept"), param("Content-Type"), param("Authorization")];
    let paths = json!({"/a": {"get": {"operationId": "a", "parameters": params,
        "responses": {"204": {"description": "ok"}}}}});
    let path = common::write("ignored-headers", &doc(&paths, &json!({})), &[]);
    let made = crate::generate::generate(&path, &Options::default()).expect("generates");
    for i in ["0", "1", "2"] {
        let at = format!("/paths/~1a/get/parameters/{i}");
        let warned = made.diagnostics.iter().filter(|d| d.pointer == at && d.message.contains("is ignored")).count();
        assert_eq!(warned, 1, "{at}: {:#?}", made.diagnostics);
    }
}

/// `x-enum-varnames` as a list names the values by position, so a reordered `enum` renames them: it warns. An object
/// from value to name does not.
#[test]
fn enum_names_by_position_warn() {
    let schemas =
        |names: Value| json!({"schemas": {"E": {"type": "integer", "enum": [1, 2], "x-enum-varnames": names}}});
    let warned = |names: Value| {
        let (_, w) = common::build("enum-names", &doc(&json!({}), &schemas(names)), &[]);
        w.iter().any(|m| m.starts_with("/components/schemas/E/x-enum-varnames:"))
    };
    assert!(warned(json!(["One", "Two"])), "a list");
    assert!(!warned(json!({"1": "One", "2": "Two"})), "an object");
}
