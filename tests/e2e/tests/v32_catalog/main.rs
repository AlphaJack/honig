//! The recipes of the book on the `catalog` fixture. Each module is a page. The book includes its anchors.

#![cfg(feature = "axum")]

#[path = "../support/mod.rs"]
mod support;

mod adapters;
mod auth;
mod client;
mod lambda;
mod layers;
mod observability;
mod recipes;
mod schemas;
mod websocket;

use std::sync::{Mutex, MutexGuard};

use e2e::v32_catalog::api::{self, operations as ops, types};

/// Answers a request that the server refuses itself in the catalog's own error body, `Error {message}`. No standard
/// defines its field, so this fills it: the detail goes in `message`. A body that a handler or layer set itself goes
/// out as it is.
// ANCHOR: error_response
pub fn error_body(rejection: api::Rejection) -> api::Response {
    rejection.json(&types::Error { message: rejection.message() })
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
        let mut products = self.products()?;
        // The ID is the index: the request has none, the product that the catalog stores has it.
        let mut product = serde_json::to_value(&params.body).unwrap_or_default();
        product["id"] = products.len().into();
        let product: types::Product = serde_json::from_value(product)
            .map_err(|e| api::ApiError::new(honig::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        products.push(product.clone());
        Ok(ops::create_product::Response::Created(product))
    }

    async fn get_product(
        &self,
        params: ops::get_product::Params,
        _: &api::Context<'_, ops::get_product::Security<Self>>,
    ) -> Result<ops::get_product::Response, api::ApiError> {
        let found = usize::try_from(params.product_id).ok().and_then(|i| self.products().ok()?.get(i).cloned());
        Ok(match found {
            Some(product) => ops::get_product::Response::Ok(product),
            None => ops::get_product::Response::NotFound(types::Error::new("no such product".to_owned())),
        })
    }

    /// Every product as JSON. The recipes ask for no CSV: it is `501`.
    async fn export_products(
        &self,
        params: ops::export_products::Params,
        _: &api::Context<'_, ops::export_products::Security<Self>>,
    ) -> Result<ops::export_products::Response, api::ApiError> {
        match params.media {
            ops::export_products::Media::Json => {
                Ok(ops::export_products::Response::Ok(ops::export_products::OkBody::Json(self.products()?.clone())))
            }
            ops::export_products::Media::Csv => {
                Err(api::ApiError::new(honig::http::StatusCode::NOT_IMPLEMENTED, "no CSV in the recipes"))
            }
        }
    }

    /// The recipes delete nothing: every product is `404`.
    async fn delete_product(
        &self,
        _: ops::delete_product::Params,
        _: &api::Context<'_, ops::delete_product::Security<Self>>,
    ) -> Result<ops::delete_product::Response, api::ApiError> {
        Ok(ops::delete_product::Response::NotFound(types::Error::new("no such product".to_owned())))
    }

    /// The recipes keep no photos.
    async fn get_product_photo(
        &self,
        _: ops::get_product_photo::Params,
        _: &api::Context<'_, ops::get_product_photo::Security<Self>>,
    ) -> Result<ops::get_product_photo::Response, api::ApiError> {
        Ok(ops::get_product_photo::Response::NotFound)
    }

    /// The recipes keep no photos.
    async fn put_product_photo(
        &self,
        _: ops::put_product_photo::Params,
        _: &api::Context<'_, ops::put_product_photo::Security<Self>>,
    ) -> Result<ops::put_product_photo::Response, api::ApiError> {
        Ok(ops::put_product_photo::Response::NotFound)
    }

    /// The recipes keep no history: the feed has ended.
    async fn stream_product_events(
        &self,
        _: ops::stream_product_events::Params,
        _: &api::Context<'_, ops::stream_product_events::Security<Self>>,
    ) -> Result<ops::stream_product_events::Response, api::ApiError> {
        Ok(ops::stream_product_events::Response::NoContent)
    }
}

/// A desk lamp: the product every recipe creates.
fn lamp() -> types::ProductRequest {
    types::ProductRequest::Physical(types::PhysicalRequest::new("Desk lamp".to_owned()))
}

/// The desk lamp as the catalog stores it: its first product, with the ID 0.
fn stored_lamp() -> types::Product {
    types::Product::Physical(types::Physical::new(0, "Desk lamp".to_owned()))
}
