//! The catalog of a shop: the module that `cargo honig generate` writes from `openapi.yaml` and the handlers of its
//! operations.

pub mod api;
/// A client for the book supplier: a second module from `supplier.yaml`.
pub mod supplier;

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use api::{operations as ops, types};

/// The API key of the staff when the shop is given no other: see [`Catalog::with_api_key`].
pub const API_KEY: &str = "secret";
/// The API key of a partner.
pub const PARTNER_KEY: &str = "partner";

// ANCHOR: server
// ANCHOR: catalog
/// Who a valid key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// Changes the catalog.
    Staff,
    /// Only looks at the catalog.
    Partner,
}

/// The catalog in memory. The ID of a product is its index. A deleted product leaves its place empty.
#[derive(Default)]
pub struct Catalog {
    api_key: Option<String>,
    products: Mutex<Vec<Option<types::Product>>>,
    photos: Mutex<BTreeMap<i64, Photo>>,
}

/// A photo as it was uploaded. Each upload is a new version, which the `ETag` names.
struct Photo {
    bytes: Vec<u8>,
    media: Option<String>,
    version: u32,
}

impl Photo {
    fn etag(&self) -> String {
        format!("\"v{}\"", self.version)
    }
}

/// The value of `mutex`, or a `500` when a handler panicked while it held it.
fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, api::ApiError> {
    mutex
        .lock()
        .map_err(|_| api::ApiError::new(honig::http::StatusCode::INTERNAL_SERVER_ERROR, "catalog lock poisoned"))
}

impl Catalog {
    /// Sets the API key of the staff. Without it the key is [`API_KEY`].
    #[must_use]
    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = Some(key);
        self
    }

    fn products(&self) -> Result<MutexGuard<'_, Vec<Option<types::Product>>>, api::ApiError> {
        lock(&self.products)
    }
}

impl api::Api for Catalog {
    type ApiKey = Caller;

    /// Checks the `X-API-Key` of a request: who the key belongs to, or nobody (401).
    async fn verify_api_key(&self, key: &str, _: &api::Request) -> Result<Option<Caller>, api::ApiError> {
        Ok(if key == self.api_key.as_deref().unwrap_or(API_KEY) {
            Some(Caller::Staff)
        } else {
            (key == PARTNER_KEY).then_some(Caller::Partner)
        })
    }

    // ANCHOR: error_response
    /// A request that the server refuses itself (no route, bad input, a handler `Err`) in the shop's `Error`: the
    /// detail goes in `message`.
    fn error_response(rejection: api::Rejection) -> api::Response {
        rejection.json(&types::Error::new(rejection.message()))
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
        ctx: &api::Context<'_, ops::create_product::Security<Self>>,
    ) -> Result<ops::create_product::Response, api::ApiError> {
        let ops::create_product::Security::ApiKey(caller) = ctx.security;
        if caller != Caller::Staff {
            let error = types::Error::new("only staff members can add products".to_owned());
            return Ok(ops::create_product::Response::Forbidden(error));
        }
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
            None => ops::get_product::Response::NotFound(types::Error::new("no such product".to_owned())),
        })
    }
    // ANCHOR_END: catalog

    async fn delete_product(
        &self,
        params: ops::delete_product::Params,
        _: &api::Context<'_, ops::delete_product::Security<Self>>,
    ) -> Result<ops::delete_product::Response, api::ApiError> {
        let mut products = self.products()?;
        let gone = usize::try_from(params.product_id).ok().and_then(|i| products.get_mut(i)?.take());
        Ok(match gone {
            Some(_) => ops::delete_product::Response::NoContent,
            None => ops::delete_product::Response::NotFound(types::Error::new("no such product".to_owned())),
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

    // ANCHOR: download
    /// The photo in the image format it was uploaded in. `image/*` declares a range: the blob says which image it is.
    async fn get_product_photo(
        &self,
        params: ops::get_product_photo::Params,
        _: &api::Context<'_, ops::get_product_photo::Security<Self>>,
    ) -> Result<ops::get_product_photo::Response, api::ApiError> {
        let photos = lock(&self.photos)?;
        let Some(photo) = photos.get(&params.product_id) else {
            return Ok(ops::get_product_photo::Response::NotFound);
        };
        let body = api::Blob::from(photo.bytes.clone());
        let body = match &photo.media {
            Some(media) => body.with_media(media.clone()),
            None => body,
        };
        Ok(ops::get_product_photo::Response::Ok { body, e_tag: Some(photo.etag()) })
    }
    // ANCHOR_END: download

    // ANCHOR: upload
    /// Stores a new version of the photo. A client that sends `If-Match` replaces only the version it read.
    async fn put_product_photo(
        &self,
        params: ops::put_product_photo::Params,
        ctx: &api::Context<'_, ops::put_product_photo::Security<Self>>,
    ) -> Result<ops::put_product_photo::Response, api::ApiError> {
        let index = usize::try_from(params.product_id).ok();
        if !index.is_some_and(|i| self.products().is_ok_and(|p| p.get(i).is_some_and(Option::is_some))) {
            return Ok(ops::put_product_photo::Response::NotFound);
        }
        let media = params.body.media().map(str::to_owned);
        // The chunks as they arrive. One past `max_body_bytes` is an `Err`: `?` answers `413`.
        let bytes = params.body.collect().await?;
        let mut photos = lock(&self.photos)?;
        let current = photos.get(&params.product_id);
        let (version, etag) = (current.map_or(1, |p| p.version + 1), current.map(Photo::etag));
        // 412 when `If-Match` names another version. `None`: no photo yet, which `If-Match: *` refuses.
        ctx.preconditions(
            etag.as_deref().map(|etag| honig::CacheValidators { etag: Some(etag), last_modified: None }),
        )?;
        photos.insert(params.product_id, Photo { bytes, media, version });
        Ok(ops::put_product_photo::Response::NoContent)
    }
    // ANCHOR_END: upload

    /// The shop keeps no history of its changes: the feed has ended.
    async fn stream_product_events(
        &self,
        _: ops::stream_product_events::Params,
        _: &api::Context<'_, ops::stream_product_events::Security<Self>>,
    ) -> Result<ops::stream_product_events::Response, api::ApiError> {
        Ok(ops::stream_product_events::Response::NoContent)
    }
    // ANCHOR: catalog
}
// ANCHOR_END: catalog
// ANCHOR_END: server
