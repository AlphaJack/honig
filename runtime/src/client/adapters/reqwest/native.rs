//! A request body as reqwest sends it natively: streamed piece by piece.

use crate::{Body, ClientError};

/// Returns the body as it is: bytes held whole go out with their length.
#[expect(clippy::unused_async, reason = "the same signature as wasm32's, which reads the pieces first")]
pub(crate) async fn body(body: Body) -> Result<reqwest::Body, ClientError> {
    Ok(body.into())
}

/// Bytes held whole go out with their length. A body that streams goes out chunk by chunk.
impl From<Body> for reqwest::Body {
    fn from(body: Body) -> Self {
        if body.is_streamed() { Self::wrap(body) } else { body.into_parts().0.into() }
    }
}

/// Returns the response body a chunk at a time as it arrives.
pub(crate) fn chunks(resp: reqwest::Response) -> impl futures_core::Stream<Item = Result<bytes::Bytes, ClientError>> {
    futures_util::stream::unfold(resp, |mut resp| async move {
        let chunk = resp.chunk().await.map_err(ClientError::transport).transpose()?;
        Some((chunk, resp))
    })
}
