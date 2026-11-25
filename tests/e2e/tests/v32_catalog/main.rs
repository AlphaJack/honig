//! The recipes of the book on the `catalog` fixture. Each module is a page. The book includes its anchors.

#![cfg(feature = "axum")]

#[path = "../support/mod.rs"]
mod support;

mod adapters;
mod auth;
mod layers;
mod observability;
mod recipes;
mod schemas;
mod websocket;

use std::sync::{Mutex, MutexGuard};

use e2e::v32_catalog::api::{self, operations as ops, types};

/// Answers a request that the server refuses itself in the catalog's own error body, `Error {code, message}`. No
/// standard defines its fields, so this maps them: the status goes in `code`, the detail in `message`. A body that a
/// handler or layer set itself goes out as it is.
// ANCHOR: error_response
pub fn error_body(rejection: api::Rejection) -> api::Response {
    let code = i32::from(rejection.status().as_u16());
    rejection.json(&types::Error { code, message: rejection.message() })
}
// ANCHOR_END: error_response

/// The catalog in memory. The ID of a product is its index.
#[derive(Default)]
pub struct Catalog(Mutex<Vec<types::Product>>);

impl Catalog {
    fn products(&self) -> Result<MutexGuard<'_, Vec<types::Product>>, api::ApiError> {
        self.0
            .lock()
            .map_err(|_| api::ApiError::new(honig::http::StatusCode::INTERNAL_SERVER_ERROR, "catalog lock poisoned"))
    }
}

impl api::Api for Catalog {
    type ApiKey = ();
    fn error_response(rejection: api::Rejection) -> api::Response {
        error_body(rejection)
    }

    /// The catalog's one key is `secret`. Any other key is nobody.
    async fn verify_api_key(&self, credential: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok((credential == "secret").then_some(()))
    }

    async fn list_products(
        &self,
        _: ops::list_products::Params,
        _: &api::Context<'_, ops::list_products::Security<Self>>,
    ) -> Result<ops::list_products::Response, api::ApiError> {
        Ok(ops::list_products::Response::Ok { body: self.products()?.clone(), x_next: None })
    }

    async fn create_product(
        &self,
        params: ops::create_product::Params,
        _: &api::Context<'_, ops::create_product::Security<Self>>,
    ) -> Result<ops::create_product::Response, api::ApiError> {
        self.products()?.push(params.body.clone());
        Ok(ops::create_product::Response::Created(params.body))
    }

    async fn get_product(
        &self,
        params: ops::get_product::Params,
        _: &api::Context<'_, ops::get_product::Security<Self>>,
    ) -> Result<ops::get_product::Response, api::ApiError> {
        let found = usize::try_from(params.product_id).ok().and_then(|i| self.products().ok()?.get(i).cloned());
        Ok(match found {
            Some(product) => ops::get_product::Response::Ok(product),
            None => ops::get_product::Response::NotFound(types::Error::new(404, "no such product".to_owned())),
        })
    }
}

/// A desk lamp: the product every recipe creates.
fn lamp() -> types::Product {
    types::Product::Physical(types::Physical::new("Desk lamp".to_owned()))
}
