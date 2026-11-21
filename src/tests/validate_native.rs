//! The `validate` command checks a document under its own version with
//! pointers into it. It never reports what an upgrade would translate.

use super::common;

use serde_json::{Value, json};

fn check(test: &str, doc: &Value) -> Vec<String> {
    let d = crate::load::validate_path(&common::write(test, doc, &[]));
    d.iter().map(|x| format!("{:?} {}: {}", x.level, x.pointer, x.message)).collect()
}

#[test]
fn newer_fields_are_errors_at_their_own_pointer() {
    let mut doc =
        common::spec("3.1.0", &json!({"/x": {"query": {"responses": {"200": {"description": "ok"}}}}}), &json!({}));
    doc["tags"] = json!([{"name": "a", "parent": "b"}]);
    let out = check("native-a31", &doc);
    assert_eq!(out.len(), 2, "{out:#?}");
    assert!(out[0].contains("/tags/0/parent") && out[0].contains("OpenAPI 3.2"), "{out:#?}");
    assert!(out[1].contains("/paths/~1x/query"), "{out:#?}");
}

/// A local `$ref` that leads nowhere and a security scheme without a field that its type requires are errors.
#[test]
fn a_dangling_ref_and_an_incomplete_scheme_are_errors() {
    let doc = common::spec(
        "3.2.0",
        &json!({"/a": {"get": {"responses": {"200": {"$ref": "#/components/responses/Missing"}}}}}),
        &json!({"securitySchemes": {"key": {"type": "apiKey", "in": "header"}}}),
    );
    let out = check("native-dangling", &doc);
    assert!(out.iter().any(|m| m.contains("`#/components/responses/Missing` does not resolve")), "{out:#?}");
    assert!(out.iter().any(|m| m.contains("`name` is required for this security scheme type")), "{out:#?}");
}

#[test]
fn version_features_are_not_upgrade_noise() {
    // `nullable` is valid 3.0: nothing to report.
    let doc = common::spec("3.0.3", &json!({}), &json!({"schemas": {"A": {"type": "string", "nullable": true}}}));
    let out = check("native-a30", &doc);
    assert_eq!(out, Vec::<String>::new());
}

fn swagger(paths: &Value, extra: &Value) -> Value {
    let mut doc = json!({"swagger": "2.0", "info": {"title": "t", "version": "1"}, "paths": paths});
    for (k, v) in extra.as_object().into_iter().flatten() {
        doc[k] = v.clone();
    }
    doc
}

/// A valid 2.0 document using most of 2.0: nothing to report.
#[test]
fn a_valid_v20_document_is_clean() {
    let doc = swagger(
        &json!({"/products/{id}": {
            "parameters": [{"name": "id", "in": "path", "required": true, "type": "string"}],
            "get": {
                "operationId": "getProduct",
                "parameters": [
                    {"name": "tags", "in": "query", "type": "array", "items": {"type": "string"}, "collectionFormat": "multi"},
                    {"$ref": "#/parameters/Limit"}
                ],
                "security": [{"oauth": ["read"]}, {"key": []}],
                "responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Product"},
                    "headers": {"X-Rate": {"type": "integer"}}}, "default": {"$ref": "#/responses/Error"}}
            },
            "post": {
                "consumes": ["multipart/form-data"],
                "parameters": [{"name": "file", "in": "formData", "type": "file"}],
                "responses": {"204": {"description": "ok"}}
            }
        }}),
        &json!({
            "host": "api.example.com:8443", "basePath": "/v1", "schemes": ["https"],
            "parameters": {"Limit": {"name": "limit", "in": "query", "type": "integer"}},
            "responses": {"Error": {"description": "error"}},
            "definitions": {"Product": {"type": "object", "discriminator": "kind", "required": ["kind"],
                "properties": {"kind": {"type": "string"}}}},
            "securityDefinitions": {
                "oauth": {"type": "oauth2", "flow": "accessCode", "authorizationUrl": "https://a", "tokenUrl": "https://t", "scopes": {"read": "r"}},
                "key": {"type": "apiKey", "name": "X-Key", "in": "header"}
            },
            "tags": [{"name": "products"}]
        }),
    );
    assert_eq!(check("native-s20-ok", &doc), Vec::<String>::new());
}

/// Each case breaks one Swagger 2.0 rule. It is reported as an error at its pointer.
fn assert_rules(cases: Vec<(&str, Value, &str)>) {
    for (name, doc, want) in cases {
        let out = check(&format!("native-s20-{name}"), &doc);
        assert!(out.iter().any(|m| m.starts_with("Error") && m.contains(want)), "{name}: want `{want}` in {out:#?}");
    }
}

/// An operation with these parameters and one response.
fn op(params: &Value) -> Value {
    json!({"parameters": params, "responses": {"200": {"description": "ok"}}})
}

#[test]
fn v20_document_rules_are_checked() {
    assert_rules(vec![
        ("info", json!({"swagger": "2.0", "paths": {}}), "`info` is required"),
        ("host", swagger(&json!({}), &json!({"host": "https://x.com/v1"})), "/host: `host` must be the host only"),
        ("basePath", swagger(&json!({}), &json!({"basePath": "v1"})), "/basePath: `basePath` must start with `/`"),
        ("schemes", swagger(&json!({}), &json!({"schemes": ["ftp"]})), "/schemes/0: scheme"),
        ("unknown", swagger(&json!({}), &json!({"servers": []})), "/servers: unknown field `servers`"),
        (
            "responses",
            swagger(&json!({"/a": {"get": {}}}), &json!({})),
            "/paths/~1a/get: `responses` needs at least one",
        ),
        (
            "description",
            swagger(&json!({"/a": {"get": {"responses": {"200": {}}}}}), &json!({})),
            "/paths/~1a/get/responses/200: `description` is required",
        ),
        (
            "status",
            swagger(&json!({"/a": {"get": {"responses": {"2XX": {"description": "x"}}}}}), &json!({})),
            "`2XX` is not a status code",
        ),
    ]);
}

#[test]
fn v20_parameter_rules_are_checked() {
    assert_rules(vec![
        (
            "path-required",
            swagger(
                &json!({"/a/{id}": {"get": op(&json!([{"name": "id", "in": "path", "type": "string"}]))}}),
                &json!({}),
            ),
            "/parameters/0/required: path parameters must have `required: true`",
        ),
        (
            "path-missing",
            swagger(&json!({"/a/{id}": {"get": op(&json!([]))}}), &json!({})),
            "path variable `id` has no `in: path` parameter",
        ),
        (
            "path-extra",
            swagger(
                &json!({"/a": {"get": op(&json!([{"name": "id", "in": "path", "required": true, "type": "string"}]))}}),
                &json!({}),
            ),
            "path parameter `id` does not appear in the path",
        ),
        (
            "in",
            swagger(&json!({"/a": {"get": op(&json!([{"name": "q", "in": "cookie", "type": "string"}]))}}), &json!({})),
            "/parameters/0/in: `in` must be",
        ),
        (
            "type",
            swagger(&json!({"/a": {"get": op(&json!([{"name": "q", "in": "query"}]))}}), &json!({})),
            "/parameters/0/type: `type` must be",
        ),
        (
            "items",
            swagger(
                &json!({"/a": {"get": op(&json!([{"name": "q", "in": "query", "type": "array", "items": {}}]))}}),
                &json!({}),
            ),
            "/parameters/0/items/type: `type` must be",
        ),
    ]);
}

#[test]
fn v20_file_and_body_parameter_rules_are_checked() {
    assert_rules(vec![
        (
            "file",
            swagger(&json!({"/a": {"post": op(&json!([{"name": "f", "in": "query", "type": "file"}]))}}), &json!({})),
            "`type: file` is only for `formData` parameters",
        ),
        (
            "file-consumes",
            swagger(
                &json!({"/a": {"post": op(&json!([{"name": "f", "in": "formData", "type": "file"}]))}}),
                &json!({}),
            ),
            "a `type: file` parameter needs `consumes`",
        ),
        (
            "multi",
            swagger(
                &json!({"/a": {"get": op(&json!([{"name": "h", "in": "header", "type": "array", "items": {"type": "string"}, "collectionFormat": "multi"}]))}}),
                &json!({}),
            ),
            "`multi` is only for `query` and `formData`",
        ),
        (
            "body-schema",
            swagger(&json!({"/a": {"post": op(&json!([{"name": "b", "in": "body"}]))}}), &json!({})),
            "a `body` parameter needs `schema`",
        ),
        (
            "two-bodies",
            swagger(
                &json!({"/a": {"post": op(&json!([{"name": "a", "in": "body", "schema": {}}, {"name": "b", "in": "body", "schema": {}}]))}}),
                &json!({}),
            ),
            "at most one `body` parameter",
        ),
        (
            "body-form",
            swagger(
                &json!({"/a": {"post": op(&json!([{"name": "a", "in": "body", "schema": {}}, {"name": "f", "in": "formData", "type": "string"}]))}}),
                &json!({}),
            ),
            "`body` and `formData` parameters cannot be combined",
        ),
        (
            "duplicate-param",
            swagger(
                &json!({"/a": {"get": op(&json!([{"name": "q", "in": "query", "type": "string"}, {"name": "q", "in": "query", "type": "string"}]))}}),
                &json!({}),
            ),
            "duplicate parameter",
        ),
    ]);
}

#[test]
fn v20_reference_and_security_rules_are_checked() {
    assert_rules(vec![
        (
            "operationId",
            swagger(
                &json!({"/a": {"get": {"operationId": "x", "responses": {"200": {"description": "ok"}}}}, "/b": {"get": {"operationId": "x", "responses": {"200": {"description": "ok"}}}}}),
                &json!({}),
            ),
            "operationId `x` is also used",
        ),
        (
            "ref",
            swagger(
                &json!({"/a": {"get": {"responses": {"200": {"description": "ok", "schema": {"$ref": "#/definitions/Nope"}}}}}}),
                &json!({}),
            ),
            "`#/definitions/Nope` does not resolve",
        ),
        (
            "scheme-undefined",
            swagger(&json!({}), &json!({"security": [{"nope": []}]})),
            "security scheme `nope` is not defined",
        ),
        (
            "scheme-scopes",
            swagger(&json!({}), &json!({"securityDefinitions": {"b": {"type": "basic"}}, "security": [{"b": ["x"]}]})),
            "`b` is not `oauth2`: its scope list must be empty",
        ),
        (
            "oauth-urls",
            swagger(
                &json!({}),
                &json!({"securityDefinitions": {"o": {"type": "oauth2", "flow": "accessCode", "scopes": {}}}}),
            ),
            "this flow needs `authorizationUrl`",
        ),
        (
            "apikey-in",
            swagger(
                &json!({}),
                &json!({"securityDefinitions": {"k": {"type": "apiKey", "name": "k", "in": "cookie"}}}),
            ),
            "needs `in`: `query` or `header`",
        ),
        ("tags", swagger(&json!({}), &json!({"tags": [{"name": "a"}, {"name": "a"}]})), "duplicate tag `a`"),
    ]);
}

/// A MUST that real documents break widely (a third of the discriminators of APIs.guru).
/// It is reported but not as an error.
#[test]
fn a_v20_discriminator_outside_required_is_a_warning() {
    let doc = swagger(
        &json!({}),
        &json!({"definitions": {"Product": {"type": "object", "discriminator": "kind", "properties": {"kind": {"type": "string"}}}}}),
    );
    let out = check("native-s20-disc", &doc);
    assert_eq!(out.len(), 1, "{out:#?}");
    assert!(out[0].starts_with("Warning /definitions/Product/discriminator:"), "{out:#?}");
}

/// A cookie list behind a long chain of `$ref`s is still a list: every link is followed until one repeats.
#[test]
fn a_cookie_list_behind_a_long_ref_chain_is_refused() {
    let mut schemas = serde_json::Map::new();
    for i in 0..10 {
        schemas.insert(format!("L{i}"), json!({"$ref": format!("#/components/schemas/L{}", i + 1)}));
    }
    schemas.insert("L10".into(), json!({"type": "array", "items": {"type": "integer"}}));
    let paths = json!({"/d": {"get": {
        "parameters": [{"name": "ids", "in": "cookie", "explode": false, "schema": {"$ref": "#/components/schemas/L0"}}],
        "responses": {"204": {"description": "ok"}}
    }}});
    let out = check("cookie-ref-chain", &common::spec("3.2.0", &paths, &json!({"schemas": schemas})));
    assert_eq!(out.len(), 1, "{out:#?}");
    assert!(out[0].contains("commas"), "{out:#?}");
}

/// 3.2.1: `explode: false` is invalid with `in: cookie` for arrays and objects
/// (their items join with commas). A 3.2 document is refused. An earlier one
/// whose text allowed it gets a warning. A scalar or an exploded list is fine.
#[test]
fn a_cookie_list_without_explode_is_refused_in_v32_and_warned_before() {
    let cookie = |schema: Value, explode: bool| {
        json!({"/d": {"get": {
            "parameters": [{"name": "ids", "in": "cookie", "explode": explode, "schema": schema}],
            "responses": {"204": {"description": "ok"}}
        }}})
    };
    let list = json!({"type": "array", "items": {"type": "integer"}});
    let components = json!({"schemas": {"Prefs": {"type": "object"}}});
    let at = "/paths/~1d/get/parameters/0/explode";
    for (version, level) in [("3.2.0", "Error"), ("3.2.1", "Error"), ("3.1.1", "Warning")] {
        let out = check(
            &format!("cookie-explode-{version}"),
            &common::spec(version, &cookie(list.clone(), false), &components),
        );
        assert_eq!(out.len(), 1, "{version}: {out:#?}");
        assert!(out[0].starts_with(&format!("{level} {at}")) && out[0].contains("commas"), "{version}: {out:#?}");
    }
    let by_ref = common::spec("3.2.0", &cookie(json!({"$ref": "#/components/schemas/Prefs"}), false), &components);
    assert_eq!(check("cookie-explode-ref", &by_ref).len(), 1, "an object behind a `$ref`");
    for (name, schema, explode) in [("scalar", json!({"type": "string"}), false), ("exploded", list, true)] {
        let doc = common::spec("3.2.0", &cookie(schema, explode), &components);
        assert_eq!(check(&format!("cookie-explode-{name}"), &doc), Vec::<String>::new(), "{name}");
    }
}

#[test]
fn query_parameter_names_are_case_sensitive_and_header_names_are_not() {
    let doc = |a: &str, b: &str, at: &str| {
        let d = json!({"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "paths": {"/b": {"get": {
            "parameters": [{"name": a, "in": at, "schema": {"type": "string"}}, {"name": b, "in": at, "schema": {"type": "string"}}],
            "responses": {"200": {"description": "ok"}}}}}});
        crate::load::load_str(&d.to_string()).diagnostics.iter().any(|x| x.message.contains("duplicate parameter"))
    };
    // S3's `marker` and `Marker` are two query parameters.
    assert!(!doc("marker", "Marker", "query"));
    assert!(doc("X-Trace", "x-trace", "header"));
}
