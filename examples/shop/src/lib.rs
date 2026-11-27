//! The catalog of a shop: the module that `cargo honig generate` writes from `openapi.yaml` and the handlers of its
//! operations.

pub mod api;
/// A client for the book supplier: a second module from `supplier.yaml`.
pub mod supplier;

use std::sync::{Mutex, MutexGuard};

use api::{operations as ops, types};

/// The API key that the shop takes. A real one reads it from its configuration.
pub const API_KEY: &str = "secret";

// ANCHOR: catalog
/// The catalog in memory. The ID of a product is its index. A deleted product leaves its place empty.
#[derive(Default)]
pub struct Catalog(Mutex<Vec<Option<types::Product>>>);

impl Catalog {
    fn products(&self) -> Result<MutexGuard<'_, Vec<Option<types::Product>>>, api::ApiError> {
        self.0
            .lock()
            .map_err(|_| api::ApiError::new(honig::http::StatusCode::INTERNAL_SERVER_ERROR, "catalog lock poisoned"))
    }
}

impl api::Api for Catalog {
    /// Who a valid key is: the shop has one caller.
    type ApiKey = ();

    /// Checks the `X-API-Key` of a request: the shop's key is someone, any other key is nobody (401).
    async fn verify_api_key(&self, key: &str, _: &api::Request) -> Result<Option<()>, api::ApiError> {
        Ok((key == API_KEY).then_some(()))
    }

    // ANCHOR: error_response
    /// A request that the server refuses itself (no route, bad input, a handler `Err`) in the shop's `Error`: the
    /// status goes in `code`, the detail in `message`.
    fn error_response(rejection: api::Rejection) -> api::Response {
        let code = i32::from(rejection.status().as_u16());
        rejection.json(&types::Error::new(code, rejection.message()))
    }
    // ANCHOR_END: error_response

    async fn list_products(
        &self,
        params: ops::list_products::Params,
        _: &api::Context<'_, ops::list_products::Security<Self>>,
    ) -> Result<ops::list_products::Response, api::ApiError> {
        let limit = params.limit.and_then(|n| usize::try_from(n.get()).ok()).unwrap_or(usize::MAX);
        let body = self.products()?.iter().flatten().take(limit).cloned().collect();
        Ok(ops::list_products::Response::Ok { body, x_next: None })
    }

    async fn create_product(
        &self,
        params: ops::create_product::Params,
        _: &api::Context<'_, ops::create_product::Security<Self>>,
    ) -> Result<ops::create_product::Response, api::ApiError> {
        self.products()?.push(Some(params.body.clone()));
        Ok(ops::create_product::Response::Created(params.body))
    }

    async fn get_product(
        &self,
        params: ops::get_product::Params,
        _: &api::Context<'_, ops::get_product::Security<Self>>,
    ) -> Result<ops::get_product::Response, api::ApiError> {
        let found = usize::try_from(params.product_id).ok().and_then(|i| self.products().ok()?.get(i).cloned());
        Ok(match found.flatten() {
            Some(product) => ops::get_product::Response::Ok(product),
            None => ops::get_product::Response::NotFound(types::Error::new(404, "no such product".to_owned())),
        })
    }

    async fn delete_product(
        &self,
        params: ops::delete_product::Params,
        _: &api::Context<'_, ops::delete_product::Security<Self>>,
    ) -> Result<ops::delete_product::Response, api::ApiError> {
        let mut products = self.products()?;
        let gone = usize::try_from(params.product_id).ok().and_then(|i| products.get_mut(i)?.take());
        Ok(match gone {
            Some(_) => ops::delete_product::Response::NoContent,
            None => ops::delete_product::Response::NotFound(types::Error::new(404, "no such product".to_owned())),
        })
    }

    /// The catalog in the media type that the client prefers: JSON, or CSV with the kind and the name of each product.
    async fn export_products(
        &self,
        params: ops::export_products::Params,
        _: &api::Context<'_, ops::export_products::Security<Self>>,
    ) -> Result<ops::export_products::Response, api::ApiError> {
        let products: Vec<types::Product> = self.products()?.iter().flatten().cloned().collect();
        let body = match params.media {
            ops::export_products::Media::Json => ops::export_products::OkBody::Json(products),
            ops::export_products::Media::Csv => {
                let row = |kind: &str, name: &str| format!("{kind},\"{}\"\n", name.replace('"', "\"\""));
                let rows = products.iter().map(|product| match product {
                    types::Product::Physical(p) => row("physical", &p.name),
                    types::Product::Digital(d) => row("digital", &d.name),
                });
                ops::export_products::OkBody::Csv(std::iter::once("kind,name\n".to_owned()).chain(rows).collect())
            }
        };
        Ok(ops::export_products::Response::Ok(body))
    }

    /// The shop keeps no history of its changes: the feed has ended.
    async fn stream_product_events(
        &self,
        _: ops::stream_product_events::Params,
        _: &api::Context<'_, ops::stream_product_events::Security<Self>>,
    ) -> Result<ops::stream_product_events::Response, api::ApiError> {
        Ok(ops::stream_product_events::Response::NoContent)
    }
}
// ANCHOR_END: catalog
