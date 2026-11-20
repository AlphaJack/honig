//! The operations of a generated API as the server routes and serves them. The wide event and the steps of serving
//! both refer to them.

use crate::{ErrorSchema, Requirement};

/// An operation of a generated API.
///
/// The generated `Operation` enum implements it: one variant per operation. Your code matches on that enum. A layer
/// meant for any API is generic over this trait and identifies the operation in its logs and metrics with
/// [`Operation::name`].
pub trait Operation: Copy + Eq + std::fmt::Debug + Send + Sync + 'static {
    /// Returns how the server routes and serves the operation.
    #[doc(hidden)]
    fn op(self) -> &'static Op;

    /// Returns the operation's name in the generated code (`get_order`).
    fn name(self) -> &'static str {
        self.op().name
    }

    /// Returns the path template (`/orders/{orderId}`) or the name of the webhook.
    fn route(self) -> &'static str {
        self.op().route
    }
}

/// The error schema the description declares for an operation (`None`: no route) and status.
pub type ErrorSchemas = fn(Option<&str>, u16) -> Option<&'static ErrorSchema>;

/// One operation as the server routes and serves it.
#[derive(Debug)]
pub struct Op {
    /// The operation's name in the generated code (`get_order`).
    pub name: &'static str,
    /// HTTP method in upper case.
    pub method: &'static str,
    /// The path template or the name of the webhook.
    pub route: &'static str,
    /// The span name OpenTelemetry's HTTP conventions give it (`GET /orders/{orderId}`).
    pub span: &'static str,
    /// Security requirements (OR of ANDs); empty when the operation needs none.
    pub security: &'static [&'static [Requirement]],
    /// A binary or sequential body moves into the parameters and does not stay in
    /// [`Context::request`](crate::Context::request). It reaches the handler as it arrives
    /// ([`Blob::is_streamed`](crate::Blob::is_streamed) and [`Items::Stream`](crate::Items::Stream)). The server reads
    /// every other body whole first.
    pub takes_body: bool,
    /// The description's error schemas.
    pub errors: ErrorSchemas,
    /// The media types of its success responses in the order of the description. `Accept` chooses among them (406
    /// when it takes none). Empty when they have no body.
    pub produces: &'static [&'static str],
    /// The server answers `Range` with one byte range of the success response. This is a `GET` with a binary success
    /// response that declares no `206` and no `Range` parameter of its own.
    pub ranges: bool,
}
