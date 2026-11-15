//! `readOnly` / `writeOnly` properties give separate request and response
//! types. So the required properties of each direction are required in Rust.

use super::common;

use crate::ir::types::{Kind, Ty};
use serde_json::json;

fn fields(api: &crate::ir::Api, name: &str) -> Vec<(String, bool)> {
    let def = api
        .types
        .defs
        .iter()
        .find(|d| d.name == crate::ir::names::Name::new(name))
        .unwrap_or_else(|| panic!("no {name}"));
    let Kind::Struct(s) = &def.kind else { panic!("{name} is not a struct") };
    s.fields.iter().map(|f| (f.wire.clone(), f.required)).collect()
}

#[test]
fn each_direction_gets_its_own_type() {
    let doc = common::spec(
        "3.1.0",
        &json!({"/logins": {"post": {
            "requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Session"}}}},
            "responses": {"201": {"description": "ok", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Session"}}}}},
        }}}),
        &json!({"schemas": {
            "Login": {"type": "object", "required": ["user", "password", "token"], "properties": {
                "user": {"type": "string"},
                "password": {"type": "string", "writeOnly": true},
                "token": {"type": "string", "readOnly": true},
            }},
            // Reaches `Login`: it splits too.
            "Session": {"type": "object", "required": ["login"], "properties": {"login": {"$ref": "#/components/schemas/Login"}}},
        }}),
    );
    let (api, _) = common::build("directions", &doc, &[]);
    assert_eq!(fields(&api, "Login"), [("user".to_owned(), true), ("token".to_owned(), true)]);
    assert_eq!(fields(&api, "LoginRequest"), [("user".to_owned(), true), ("password".to_owned(), true)]);
    let request = api
        .types
        .defs
        .iter()
        .position(|d| d.name == crate::ir::names::Name::new("SessionRequest"))
        .expect("SessionRequest");
    let op = &api.http.operations[0];
    assert_eq!(op.body.as_ref().expect("body").contents[0].ty, Ty::Named(crate::ir::types::TypeId(request)));
    let response =
        api.types.defs.iter().position(|d| d.name == crate::ir::names::Name::new("Session")).expect("Session");
    assert_eq!(op.responses[0].contents[0].ty, Ty::Named(crate::ir::types::TypeId(response)));
}
