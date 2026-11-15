//! `$dynamicRef` generics become concrete types per binding site.

use e2e::v32_generics::api::{self, operations as ops, types};
use serde_json::json;

#[test]
fn bound_generics_are_monomorphized() {
    let page = types::ProductPage { items: vec![types::Product { name: "Lamp".to_owned() }], next: None };
    let first: &types::Product = &page.items[0];
    assert_eq!(first.name, "Lamp");
    let tree = types::StringTree {
        value: "root".to_owned(),
        children: Some(vec![types::StringTree { value: "leaf".to_owned(), children: None }]),
    };
    let leaf: &String = &tree.children.as_ref().expect("children")[0].value;
    assert_eq!(leaf, "leaf");
}

#[test]
fn unbound_generic_keeps_its_default() {
    let page: types::Page = serde_json::from_value(json!({"items":[1,"two",{"three":3}]})).expect("anything goes");
    assert_eq!(page.items.len(), 3);
}

/// Answers each operation with a fixed value of its generic type.
struct Fixed;

fn page() -> types::ProductPage {
    types::ProductPage { items: vec![types::Product { name: "Lamp".to_owned() }], next: Some("2".to_owned()) }
}

fn tree() -> types::StringTree {
    types::StringTree {
        value: "root".to_owned(),
        children: Some(vec![types::StringTree { value: "leaf".to_owned(), children: Some(vec![]) }]),
    }
}

fn any() -> types::Page {
    serde_json::from_value(json!({"items": [1, "two", {"three": 3}]})).expect("anything goes")
}

impl api::Api for Fixed {
    type Principal = ();

    async fn list_products(
        &self,
        _: ops::list_products::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::list_products::Response, api::ApiError> {
        Ok(ops::list_products::Response::Ok(page()))
    }

    async fn tag_tree(
        &self,
        _: ops::tag_tree::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::tag_tree::Response, api::ApiError> {
        Ok(ops::tag_tree::Response::Ok(tree()))
    }

    async fn any_page(
        &self,
        _: ops::any_page::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::any_page::Response, api::ApiError> {
        Ok(ops::any_page::Response::Ok(any()))
    }
}

/// Each monomorphized type goes out from the server and comes back from the client as it was.
#[test]
fn generic_bodies_round_trip() {
    let service = api::Server::new(Fixed).into_service();
    let client = api::Client::new(service);
    assert_eq!(
        api::block_on(client.list_products(ops::list_products::Params::new())).ok(),
        Some(ops::list_products::Response::Ok(page()))
    );
    assert_eq!(
        api::block_on(client.tag_tree(ops::tag_tree::Params::new())).ok(),
        Some(ops::tag_tree::Response::Ok(tree()))
    );
    assert_eq!(
        api::block_on(client.any_page(ops::any_page::Params::new())).ok(),
        Some(ops::any_page::Response::Ok(any()))
    );
}
