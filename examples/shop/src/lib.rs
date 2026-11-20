//! The catalog of a shop: the module that `cargo honig generate` writes from `openapi.yaml` and the handlers of its
//! operations.

pub mod api;
/// A client for the book supplier: a second module from `supplier.yaml`.
pub mod supplier;

use std::sync::{Mutex, MutexGuard};

use api::{operations as ops, types};

// ANCHOR: catalog
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

    // ANCHOR: error_response
    /// A request that the server refuses itself (no route, bad input, a handler `Err`) in the shop's `Error`: the
    /// status goes in `code`, the detail in `message`.
    fn error_response(rejection: api::Rejection) -> api::Response {
        use api::Message as _;
        if let Some(own) = rejection.reason.own() {
            return own;
        }
        let status = rejection.status();
        let message = rejection.reason.detail().unwrap_or_else(|| status.to_string());
        let body = types::Error::new(i32::from(status.as_u16()), message);
        let mut response = rejection.reason.head();
        response.set_body("application/json", serde_json::to_vec(&body).unwrap_or_default());
        response
    }
    // ANCHOR_END: error_response

    async fn list_products(
        &self,
        params: ops::list_products::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::list_products::Response, api::ApiError> {
        let limit = params.limit.and_then(|n| usize::try_from(n.get()).ok()).unwrap_or(usize::MAX);
        let body = self.products()?.iter().take(limit).cloned().collect();
        Ok(ops::list_products::Response::Ok { body, x_next: None })
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
// ANCHOR_END: catalog
