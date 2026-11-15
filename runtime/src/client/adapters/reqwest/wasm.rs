//! reqwest from a browser. `fetch` takes no streamed request body. So its pieces are read first. Responses stream.

use crate::{Body, Error};

/// Returns the whole body in one buffer.
pub(crate) async fn body(body: Body) -> Result<reqwest::Body, Error> {
    Ok(body.collect().await?.into())
}

/// Returns the response body a chunk at a time as `fetch` reads it. The `Response` of wasm32 has no `chunk`.
pub(crate) fn chunks(resp: reqwest::Response) -> impl futures_core::Stream<Item = Result<bytes::Bytes, Error>> {
    futures_util::StreamExt::map(resp.bytes_stream(), |b| b.map_err(Error::transport))
}
