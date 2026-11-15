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

/// The catalog in memory. The ID of a product is its index.
#[derive(Default)]
pub struct Catalog(Mutex<Vec<types::Product>>);

impl Catalog {
    fn products(&self) -> Result<MutexGuard<'_, Vec<types::Product>>, api::ApiError> {
        self.0
            .lock()
            .map_err(|_| api::ApiError::new(api::http::StatusCode::INTERNAL_SERVER_ERROR, "catalog lock poisoned"))
    }
}

impl api::Api for Catalog {
    type Principal = ();

    async fn list_products(
        &self,
        _: ops::list_products::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::list_products::Response, api::ApiError> {
        Ok(ops::list_products::Response::Ok { body: self.products()?.clone(), x_next: None })
    }

    async fn create_product(
        &self,
        params: ops::create_product::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::create_product::Response, api::ApiError> {
        self.products()?.push(params.body.clone());
        Ok(ops::create_product::Response::Created(params.body))
    }

    async fn get_product(
        &self,
        params: ops::get_product::Params,
        _: &api::Context<'_>,
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
    types::Product::Physical(types::Physical::new(types::PhysicalKind, "Desk lamp".to_owned()))
}
