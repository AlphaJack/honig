//! `reqwest-middleware` stacks as a [`ReqwestTransport`] client.

use std::future::Future;

use crate::{ClientError, Execute, ReqwestTransport, transport::MaybeSend};

/// `reqwest-middleware` transport: `Client::new(api::ReqwestMiddlewareTransport::with_client(stack, "https://api.example.com"))`.
///
/// It has no `new` of its own: a second `new` on `ReqwestTransport` would make `ReqwestTransport::new(url)` ambiguous.
pub type ReqwestMiddlewareTransport = ReqwestTransport<reqwest_middleware::ClientWithMiddleware>;

impl Execute for reqwest_middleware::ClientWithMiddleware {
    fn execute(
        &self,
        req: reqwest::Request,
    ) -> impl Future<Output = Result<reqwest::Response, ClientError>> + MaybeSend {
        let client = self.clone();
        async move { Self::execute(&client, req).await.map_err(ClientError::transport) }
    }
}
