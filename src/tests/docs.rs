//! Documentation-only fields reach the generated rustdoc: `info.contact`,
//! `info.license`, `info.termsOfService` and every `externalDocs`.

use super::common;

use serde_json::{Value, json};

fn generated(test: &str) -> Vec<(String, String)> {
    common::generate(test, &spec(), &crate::emit::Options::default())
}

use common::file;

fn spec() -> Value {
    json!({
        "openapi": "3.2.0",
        "info": {
            "title": "Products",
            "version": "1",
            "termsOfService": "https://example.com/terms",
            "contact": {"name": "Product team", "email": "products@example.com"},
            "license": {"name": "MIT", "identifier": "MIT"},
        },
        "externalDocs": {"url": "https://example.com/guide"},
        "tags": [{
            "name": "products",
            "description": "Product care",
            "externalDocs": {"description": "Tag guide", "url": "https://example.com/tags"},
        }],
        "paths": {"/products": {"get": {
            "operationId": "listProducts",
            "tags": ["products"],
            "summary": "List products",
            "externalDocs": {"description": "Paging", "url": "https://example.com/paging"},
            "responses": {"200": {
                "description": "ok",
                "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Product"}}},
            }},
        }}},
        "components": {"schemas": {"Product": {
            "type": "object",
            "externalDocs": {"url": "https://example.com/product"},
            "properties": {"name": {
                "type": "string",
                "externalDocs": {"description": "Naming", "url": "https://example.com/names"},
            }},
        }}},
    })
}

#[test]
fn info_fields_document_the_module() {
    let files = generated("docs-info");
    let m = file(&files, "mod.rs");
    assert!(m.contains("Terms of service: <https://example.com/terms>"), "{m}");
    assert!(m.contains("Contact: Product team, products@example.com"), "{m}");
    assert!(m.contains("License: MIT, MIT"), "{m}");
    assert!(m.contains("See also: <https://example.com/guide>"), "{m}");
}

#[test]
fn external_docs_document_types_fields_operations_and_tags() {
    let files = generated("docs-external");
    let types = file(&files, "types.rs");
    assert!(types.contains("See also: <https://example.com/product>"), "type: {types}");
    assert!(types.contains("See also: Naming (<https://example.com/names>)"), "field: {types}");
    let client = file(&files, "client.rs");
    assert!(client.contains("List products"), "summary kept: {client}");
    assert!(client.contains("See also: Paging (<https://example.com/paging>)"), "operation: {client}");
    let ops = file(&files, "operations.rs");
    assert!(ops.contains("See also: Tag guide (<https://example.com/tags>)"), "tag: {ops}");
}

/// The own `description` of a Reference Object replaces that of its target in the rustdoc (3.1+: it "SHOULD override
/// that of the referenced component"). This happens here and only here. The component itself keeps its text.
#[test]
fn a_reference_description_overrides_its_target() {
    let doc = common::spec(
        "3.2.0",
        &json!({
            "/a": {"get": {"operationId": "getA",
                "parameters": [{"$ref": "#/components/parameters/Limit", "description": "Limit for this call"}],
                "responses": {"200": {"$ref": "#/components/responses/Ok", "description": "Found the a"}}}},
            "/b": {"get": {"operationId": "getB",
                "parameters": [{"$ref": "#/components/parameters/Limit"}],
                "responses": {"200": {"$ref": "#/components/responses/Ok"}}}}
        }),
        &json!({
            "parameters": {"Limit": {"name": "limit", "in": "query", "description": "Page size", "schema": {"type": "integer"}}},
            "responses": {"Ok": {"description": "Fine"}}
        }),
    );
    let files = common::generate("ref-description", &doc, &crate::emit::Options::default());
    let ops = file(&files, "operations.rs");
    let (a, b) = ops.split_at(ops.find("pub mod get_b").expect("getB is generated"));
    assert!(a.contains("/// Limit for this call") && a.contains("/// Found the a"), "{a}");
    assert!(b.contains("/// Page size") && b.contains("/// Fine"), "{b}");
}
