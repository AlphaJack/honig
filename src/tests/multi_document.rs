//! A description in several documents. Names used in a referenced document (security schemes, discriminator
//! mappings, link `operationId`s) resolve from the entry document as 3.2 RECOMMENDS ("Resolving Implicit
//! Connections"). This holds even when the referenced document declares the same names itself.

use std::assert_matches;

use super::common;

use crate::ir::http::LinkTarget;
use crate::ir::security::SchemeKind;
use crate::ir::types::{Kind, Ty};
use serde_json::json;

#[test]
fn implicit_names_in_a_referenced_document_resolve_from_the_entry_document() {
    let entry = json!({
        "openapi": "3.2.0",
        "info": {"title": "entry", "version": "1"},
        "paths": {
            "/products": {"$ref": "shared.json#/paths/~1products"},
            "/products/{id}": {"get": {
                "operationId": "getProduct",
                "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "string"}}],
                "responses": {"200": {"description": "ok"}}
            }}
        },
        "components": {
            "securitySchemes": {"key": {"type": "apiKey", "in": "header", "name": "X-Key"}},
            "schemas": {
                "Gadget": {"type": "object", "required": ["kind"], "properties": {"kind": {"const": "gadget"}, "purrs": {"type": "boolean"}}},
                "Book": {"type": "object", "required": ["kind"], "properties": {"kind": {"const": "book"}, "hardcover": {"type": "boolean"}}}
            }
        }
    });
    // The referenced document is an OpenAPI document of its own with a `key` scheme and a `Gadget` of its own.
    let shared = json!({
        "openapi": "3.2.0",
        "info": {"title": "shared", "version": "1"},
        "paths": {"/products": {"get": {
            "operationId": "listProducts",
            "security": [{"key": []}],
            "responses": {"200": {
                "description": "ok",
                "content": {"application/json": {"schema": {
                    "oneOf": [{"$ref": "openapi.json#/components/schemas/Gadget"}, {"$ref": "openapi.json#/components/schemas/Book"}],
                    "discriminator": {"propertyName": "kind", "mapping": {"gadget": "Gadget", "book": "Book"}}
                }}},
                "links": {"first": {"operationId": "getProduct", "parameters": {"id": "$response.body#/0/kind"}}}
            }}
        }}},
        "components": {
            "securitySchemes": {"key": {"type": "http", "scheme": "bearer"}},
            "schemas": {"Gadget": {"type": "string"}}
        }
    });
    let (api, diags) = common::build("multi-document", &entry, &[("shared.json", &shared)]);
    assert!(diags.is_empty(), "{diags:#?}");
    let list = api
        .http
        .operations
        .iter()
        .find(|o| crate::emit::rust::names::snake(&o.name) == "list_products")
        .expect("listProducts from the referenced document");
    // `key` is the API key of the entry document and not the bearer scheme of the referenced document.
    assert_eq!(list.security, [vec![crate::ir::security::Requirement { scheme: "key".into(), scopes: vec![] }]]);
    let key = api.schemes.iter().find(|s| s.name == "key").expect("the scheme");
    assert_matches!(&key.kind, SchemeKind::ApiKey { location, param } if location == "header" && param == "X-Key");
    // `mapping: {gadget: Gadget}` names the `Gadget` object of the entry document and not the string of the referenced document.
    let Ty::Named(body) = &list.responses[0].contents[0].ty else { panic!("a named union") };
    let Kind::Union(u) = &api.types.get(*body).kind else { panic!("a union") };
    let gadget = u.variants.iter().find(|v| v.tags == ["gadget"]).expect("the `gadget` tag");
    let Ty::Named(gadget_ty) = &gadget.ty else { panic!("a named variant") };
    let Kind::Struct(s) = &api.types.get(*gadget_ty).kind else { panic!("the entry document's object") };
    assert!(s.fields.iter().any(|f| f.wire == "purrs"), "{s:?}");
    // The link names an operation of the entry document.
    let link = &list.responses[0].links[0];
    assert_eq!(link.target, LinkTarget::Id("getProduct".into()));
    assert!(api.http.operations.iter().any(|o| crate::emit::rust::names::snake(&o.name) == "get_product"));
}
