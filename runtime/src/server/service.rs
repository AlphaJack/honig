//! A request goes through these steps. When a step refuses the request it answers in the error schema of the
//! description:
//!
//! 1. **Routing**: an undeclared path is 404. An undeclared method is 405. A `HEAD` takes the `GET` of its path and
//!    gets its headers without the body.
//! 2. **Credentials**: the `security` of the operation. If none of its requirements are present then the answer is
//!    401. Otherwise the `verify_` method of the `Api` for each scheme checks its credential, and the server checks
//!    the scopes that it grants (403). The principal goes in the extensions.
//! 3. **Your layers** as an `http::Request<Body>` in the order that you add them. The first one is the outermost.
//!    Its extensions hold the generated `Operation`, the `RequestEvent`, the credentials ([`Secured`]) and the
//!    principal. The body is not read yet. So a refused request costs no body read. [`read_body`] reads it for a
//!    layer that needs it.
//! 4. **The body**: read whole up to `max_body_bytes` (413). An operation that streams its body skips this step.
//! 5. **Decoding and validation** (400 and 415). Then your handler runs and its response is encoded.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;

use tower_layer::{Identity, Layer, Stack};
use tower_service::Service;

use crate::server::adapter::unread;
use crate::server::conditional::Conditions;
use crate::server::operation::{Op, ProblemSchemas};
use crate::server::range::Ranges;
use crate::server::serve::{Call, Config, challenge, for_method};
use crate::trace_context::Remote;
use crate::{
    ApiError, Body, Reason, Rejection, Request, RequestEvent, Response, Verified, auth::Requirement, auth::Scheme,
    server::auth::SatisfiedSecurity,
};

/// A boxed future the server's services return. It is the one `futures_util` names.
pub use futures_util::future::BoxFuture;

/// What generated code implements for an API or for one of its webhooks. That is routing and the operations.
pub trait Operations: Send + Sync + 'static {
    /// Who is calling as the handlers see it. That is the `Principal` of your `Api`.
    type Principal: Clone + Send + Sync + 'static;

    /// The generated `Operation`: one variant per operation.
    type Operation: crate::Operation;

    /// Returns the path templates for a framework that routes first (axum). Each one alone and under each base path.
    fn paths(&self) -> &'static [&'static str];

    /// Returns the operation that a request reaches from its method and target (path and query). Otherwise 404 or 405.
    ///
    /// # Errors
    /// No operation takes the request.
    fn route(&self, method: &str, target: &str) -> Result<Self::Operation, Reason>;

    /// Returns the error schemas that the description declares for a request that no operation takes.
    fn errors(&self) -> ProblemSchemas;

    /// Returns how the API answers a request that the server refuses: the `error_response` of the generated `Api`.
    fn error_response(&self) -> fn(Rejection<Self::Operation>) -> Response;

    /// Checks the credential that a request presented for `scheme` with the `verify_` method of the `Api` for it.
    /// `None`: the credential is not valid.
    fn verify<'r>(
        &'r self,
        scheme: &'static str,
        credential: &'r str,
        request: &'r Request,
    ) -> BoxFuture<'r, Result<Option<Verified<Self::Principal>>, ApiError>>;

    /// Serves a request that [`Operations::route`] found an operation for. `call` holds the principal that the
    /// `verify_` methods found if there is one. Each operation takes what its `security` asks for.
    fn call<'r>(
        &'r self,
        request: Request,
        call: Call<'r, Option<Self::Principal>, Self::Operation>,
    ) -> BoxFuture<'r, Response>;
}

/// The credentials a request presented.
///
/// A request in your layers and a handler's [`Context`](crate::Context) both hold them. The generated
/// `server::credentials` reads them typed.
pub trait Secured {
    /// Returns credentials of every satisfied security requirement with their scheme. The generated
    /// `server::credentials` reads them typed.
    #[doc(hidden)]
    fn credentials(&self) -> &[(Scheme, String)];

    /// Returns the credentials the request presented by the kind of their scheme, for a layer meant for any API. A
    /// layer for one API matches the generated `server::credentials` instead: one variant per scheme of the
    /// description.
    fn presented(&self) -> Vec<crate::Presented<'_>> {
        self.credentials().iter().map(|(scheme, value)| crate::Presented::of(*scheme, value)).collect()
    }
}

impl<C, Id> Secured for crate::Context<'_, C, Id> {
    fn credentials(&self) -> &[(Scheme, String)] {
        &self.auth
    }
}

impl Secured for Request {
    fn credentials(&self) -> &[(Scheme, String)] {
        self.extensions().get::<MatchedOperation>().map_or(&[], |r| &r.satisfied.auth)
    }
}

/// What the server found out about a request before your layers, in its extensions.
///
/// A layer for any API identifies the operation here, as axum's `MatchedPath`. A layer for one API reads the generated
/// `Operation` instead. The credentials are read through [`Secured`].
#[derive(Debug, Clone)]
pub struct MatchedOperation {
    name: &'static str,
    satisfied: SatisfiedSecurity,
    body_limit: usize,
}

impl MatchedOperation {
    /// Returns the operation as generated code spells it (`get_order`).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Returns the body of a request in a layer, read whole on the first call.
///
/// It reads up to `max_body_bytes` and answers `413` past it. The operation then decodes the same bytes and reads
/// nothing again. A layer that verifies a signature over the body
/// (Matrix's `X-Matrix`, webhook HMACs such as `X-Hub-Signature-256`) reads it here. A layer that does not call it
/// costs no body read.
///
/// # Errors
/// The body is larger than `max_body_bytes` (413) or cannot be read (400). Return the error from your layer.
pub async fn read_body(req: &mut Request) -> Result<&[u8], ApiError> {
    let limit = req.extensions().get::<MatchedOperation>().map_or(crate::reject::BODY_LIMIT, |r| r.body_limit);
    if let Some(unread) = req.body().chunks().take_source() {
        match unread.read_all(limit).await {
            Ok(bytes) => *req.body_mut() = Body::new(bytes),
            Err(e) => return Err(Reason::unread_body(e, limit).into()),
        }
    }
    Ok(req.body().bytes())
}

/// What the router and the operations share.
struct Shared<O: Operations> {
    ops: O,
    config: Config<O::Operation>,
}

/// A server for the operations `O` with the tower layers `L` between routing and each operation.
///
/// The generated `api::Server::new(api)` starts one. Layers run in the order that they are added. The first one is the
/// outermost as with the `ServiceBuilder` of tower. Each one is a tower `Service` of `http::Request<Body>`. It
/// answers with a [`Response`] or with an error that converts into an [`ApiError`]. That error goes out in the error
/// schema that the description declares. Generic tower layers (`Timeout`, `LoadShed` and `ConcurrencyLimit`) fit
/// too. Their `BoxError` becomes a `503`.
pub struct Server<O: Operations, L = Identity> {
    ops: O,
    layer: L,
    config: Config<O::Operation>,
}

impl<O: Operations, L> std::fmt::Debug for Server<O, L> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("config", &self.config).finish_non_exhaustive()
    }
}

impl<O: Operations> Server<O> {
    /// Creates a server for `ops` without layers: requests reach each operation as they are routed. The generated
    /// `api::Server::new` takes your `Arc<impl Api>` here.
    pub fn new(ops: impl Into<O>) -> Self {
        let ops = ops.into();
        let config = Config { errors: Arc::new(ops.error_response()), ..Config::default() };
        Self { ops, layer: Identity::new(), config }
    }
}

impl<O: Operations, L> Server<O, L> {
    /// Adds a tower layer between routing and the operations. It goes inside the layers added before. The first one
    /// added is the outermost as with the `ServiceBuilder` of tower. Pass one `ServiceBuilder` to read the order top
    /// to bottom.
    ///
    /// A layer here and one on the axum `Router` that `into_axum` returns differ:
    ///
    /// - An `operation_layer` runs once routing found the operation and its credentials passed. Its extensions hold
    ///   the operation, the credentials and the principal. Its errors go out in the error
    ///   schema of the description. It fits per-principal limits, permissions, signatures and timeouts.
    /// - An axum `Router::layer` runs on every request, also to unknown paths. It knows no operation yet, and its
    ///   errors go out with the body that it writes. It fits CORS, compression and request IDs.
    pub fn operation_layer<T>(self, layer: T) -> Server<O, Stack<T, L>> {
        let Self { ops, layer: outer, config } = self;
        Server { ops, layer: Stack::new(layer, outer), config }
    }

    /// Sets the largest request body in bytes that the server reads. Larger ones get `413`. Default: 16 MiB.
    ///
    /// The limit is a setting of the server and not a layer outside it, such as `RequestBodyLimitLayer` of
    /// tower-http:
    ///
    /// - The server reads the body itself to decode it. It holds the body in memory. So it needs a limit even with
    ///   no layer around it.
    /// - A streamed [`Blob`](crate::Blob) fails the chunk that goes past the limit. The handler gets that error, and `?`
    ///   answers `413` in the error schema of the description.
    /// - [`read_body`] in a layer reads up to the same limit. A layer that checks a signature over the body and the
    ///   operation that decodes it agree on one number.
    ///
    /// A layer outside can only lower the limit. It refuses a request early when its `Content-Length` is too large.
    /// Its answer then has its own body: map it through [`ErrorResponses::response`]. A body without `Content-Length` fails
    /// while the server reads it. The server sees a broken body and answers `400`, not `413`.
    #[must_use]
    pub fn max_body_bytes(mut self, limit: usize) -> Self {
        self.config.max_body_bytes = limit;
        self
    }

    /// Sets the response for a request that the server answers without the response of your handler. The cause is no
    /// route, bad input, missing credentials, the `Err` of a layer, validation, a handler `Err` or an encoding failure.
    /// Default: the `error_response` of the generated `Api`. This one replaces it for this server alone.
    #[must_use]
    pub fn error_response(
        mut self,
        render: impl Fn(Rejection<O::Operation>) -> Response + Send + Sync + 'static,
    ) -> Self {
        self.config.errors = Arc::new(render);
        self
    }

    /// Sets the sink that receives the wide event ([`RequestEvent`]) of each request once the response is ready. By
    /// default the event is logged with `tracing` when the server lists the `tracing` adapter, and dropped otherwise.
    #[must_use]
    pub fn on_event(mut self, sink: impl Fn(&RequestEvent<O::Operation>) + Send + Sync + 'static) -> Self {
        self.config.on_event = Arc::new(sink);
        self
    }

    /// Returns the error responses of the description for a layer outside the server. Such a layer is a timeout, a body
    /// limit or an IP flood guard. It then answers in the same error schema as the operations.
    #[must_use]
    pub fn errors(&self) -> ErrorResponses<O::Operation> {
        ErrorResponses { config: self.config.clone(), schemas: self.ops.errors() }
    }

    /// Returns the server as a tower `Service` over `http` requests. hyper serves it through
    /// `hyper_util::service::TowerToHyperService`.
    pub fn into_service(self) -> ApiService<O, L::Service>
    where
        L: Layer<Endpoint<O>>,
        L::Service: Service<Request, Response = Response> + Clone + Send + 'static,
        <L::Service as Service<Request>>::Error: Into<ApiError>,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        let shared = Arc::new(Shared { ops: self.ops, config: self.config });
        let stack = self.layer.layer(Endpoint { shared: Arc::clone(&shared) });
        ApiService { shared, stack }
    }

    /// Returns an axum `Router` with a route per path template. Merge it next to your own routes or `nest` it under a
    /// prefix. Paths that it does not declare fall through to the fallback of your router.
    #[cfg(feature = "axum")]
    pub fn into_axum(self) -> axum::Router
    where
        L: Layer<Endpoint<O>>,
        L::Service: Service<Request, Response = Response> + Clone + Send + Sync + 'static,
        <L::Service as Service<Request>>::Error: Into<ApiError>,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        let service = self.into_service();
        service
            .shared
            .ops
            .paths()
            .iter()
            .fold(axum::Router::new(), |router, path| router.route_service(path, service.clone()))
    }
}

/// The error responses that the description declares. They render as the server renders its own.
/// [`Server::errors`] returns this.
///
/// A layer outside the server answers with a status and its own body. Examples are `TimeoutLayer` and
/// `RequestBodyLimitLayer` of tower-http and `tower_governor`. Map such a response through [`ErrorResponses::response`].
/// Clients then decode it as their typed error.
#[derive(Clone, Debug)]
pub struct ErrorResponses<Id> {
    config: Config<Id>,
    schemas: ProblemSchemas,
}

impl<Id: crate::Operation> ErrorResponses<Id> {
    /// Returns the response for `status` in the error schema of the description for a request that no operation took.
    /// That is its `default` or `4XX` response. Otherwise it is RFC 9457 `problem+json`. It goes through your
    /// `error_response` option.
    #[must_use]
    pub fn response(&self, status: http::StatusCode, detail: impl Into<String>) -> Response {
        self.config.reject(self.schemas, None, Reason::Denied(ApiError::new(status, detail)), http::HeaderMap::new())
    }
}

/// The operation at the end of your layers.
///
/// It reads the body, decodes, validates, runs your handler and encodes its response. It takes the principal that the
/// server put in the request's extensions.
pub struct Endpoint<O: Operations> {
    shared: Arc<Shared<O>>,
}

impl<O: Operations> Clone for Endpoint<O> {
    fn clone(&self) -> Self {
        Self { shared: Arc::clone(&self.shared) }
    }
}

impl<O: Operations> std::fmt::Debug for Endpoint<O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Endpoint")
    }
}

impl<O: Operations> Service<Request> for Endpoint<O> {
    type Response = Response;
    type Error = ApiError;
    type Future = BoxFuture<'static, Result<Response, ApiError>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> Poll<Result<(), ApiError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let shared = Arc::clone(&self.shared);
        let extensions = req.extensions_mut();
        let routed =
            extensions.get::<O::Operation>().copied().zip(extensions.get::<RequestEvent<O::Operation>>().cloned());
        let satisfied =
            extensions.remove::<MatchedOperation>().map_or_else(SatisfiedSecurity::default, |r| r.satisfied);
        let principal = extensions.remove::<O::Principal>();
        Box::pin(async move {
            let Some((operation, event)) = routed else {
                return Err(ApiError::new(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    "a request that the server did not route",
                ));
            };
            let call = Call { config: &shared.config, operation, satisfied, principal, event: &event };
            Ok(shared.ops.call(req, call).await)
        })
    }
}

/// The server as a tower `Service`: what [`Server::into_service`] returns.
pub struct ApiService<O: Operations, S> {
    shared: Arc<Shared<O>>,
    stack: S,
}

impl<O: Operations, S: Clone> Clone for ApiService<O, S> {
    fn clone(&self) -> Self {
        Self { shared: Arc::clone(&self.shared), stack: self.stack.clone() }
    }
}

impl<O: Operations, S> std::fmt::Debug for ApiService<O, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiService").field("config", &self.shared.config).finish_non_exhaustive()
    }
}

impl<O, S> ApiService<O, S>
where
    O: Operations,
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Error: Into<ApiError>,
    S::Future: Send + 'static,
{
    /// Serves a request in the types of the runtime without a framework. A generated client also sends to the
    /// service itself: `Client::new(server.into_service())` runs both in memory.
    pub fn handle(&self, req: Request) -> BoxFuture<'static, Response> {
        let (head, body) = req.into_parts();
        Box::pin(respond(Arc::clone(&self.shared), self.stack.clone(), head, body))
    }
}

impl<O, S, B> Service<http::Request<B>> for ApiService<O, S>
where
    O: Operations,
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Error: Into<ApiError>,
    S::Future: Send + 'static,
    B: http_body::Body<Data = bytes::Bytes> + Send + Unpin + 'static,
    B::Error: std::fmt::Display,
{
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let (shared, stack) = (Arc::clone(&self.shared), self.stack.clone());
        Box::pin(async move {
            let (head, body) = req.into_parts();
            // A request without a body (a `GET`) gets no slot for one.
            let body = if http_body::Body::is_end_stream(&body) {
                Body::default()
            } else {
                Body::stream(unread(body, &head.headers, shared.config.max_body_bytes))
            };
            Ok(respond(shared, stack, head, body).await)
        })
    }
}

/// Returns route, check credentials and run the layers and the operation. Then give the wide event to `on_event`.
async fn respond<O, S>(shared: Arc<Shared<O>>, mut stack: S, head: http::request::Parts, body: Body) -> Response
where
    O: Operations,
    S: Service<Request, Response = Response>,
    S::Error: Into<ApiError>,
{
    let started = std::time::Instant::now();
    let config = &shared.config;
    let (operation, op, satisfied, method) = {
        let target = head.uri.path_and_query().map_or("/", http::uri::PathAndQuery::as_str);
        let operation = match shared.ops.route(head.method.as_str(), target) {
            Ok(operation) => operation,
            Err(rejection) => return config.unrouted(shared.ops.errors(), &head, rejection),
        };
        let op = crate::Operation::op(operation);
        let method = crate::server::serve::routed_method(head.method.as_str(), op);
        let raw_query = head.uri.query().unwrap_or_default();
        let satisfied =
            crate::server::auth::check_auth(&head.headers, &crate::params::parse_query(raw_query), op.security);
        match satisfied {
            Some(satisfied) => (operation, op, satisfied, method),
            None => return refused(config, operation, method, Reason::Unauthorized, head.headers.clone(), started),
        }
    };
    // The head moves into the layers. A refusal renders by `Accept` alone (a shared buffer and not a copy).
    let accept = head.headers.get(http::header::ACCEPT).cloned();
    let conditions = Conditions::of(&head);
    let ranges = Ranges::of(&head, op.ranges);
    let remote = Remote::of(&head.headers);
    let mut req = Request::from_parts(head, body);
    let principal = match authorize(&shared.ops, &req, op.security, &satisfied.auth).await {
        Ok(principal) => principal,
        Err(reason) => return refused(config, operation, method, reason, req.headers().clone(), started),
    };
    let event = RequestEvent::new(Some(operation), method);
    event.security(&satisfied);
    let extensions = req.extensions_mut();
    if let Some(principal) = principal {
        extensions.insert(principal);
    }
    extensions.insert(operation);
    extensions.insert(event.clone());
    extensions.insert(MatchedOperation { name: op.name, satisfied, body_limit: config.max_body_bytes });
    let through = async {
        std::future::poll_fn(|cx| stack.poll_ready(cx)).await.map_err(Into::into)?;
        stack.call(req).await.map_err(Into::into)
    };
    let mut resp = match traced(op, remote, through).await {
        Ok(resp) => ranges.answer(conditions.answer(resp)),
        Err(e) => {
            let reason = Reason::Denied(e);
            event.fail(&reason);
            let mut headers = http::HeaderMap::new();
            if let Some(accept) = accept {
                headers.insert(http::header::ACCEPT, accept);
            }
            config.reject(op.errors, Some(operation), reason, headers)
        }
    };
    // Every 401 carries a challenge (RFC 9110 section 15.5.2): a refusal of the server, of a layer or of a handler.
    challenge(&mut resp, op.security);
    let resp = for_method(method, resp);
    event.finish(&resp, started);
    (config.on_event)(&event);
    resp
}

/// Returns the answer to a request that its credentials did not let through, and gives its event to `on_event`.
fn refused<Id: crate::Operation>(
    config: &Config<Id>,
    operation: Id,
    method: &str,
    reason: Reason,
    headers: http::HeaderMap,
    started: std::time::Instant,
) -> Response {
    let op = operation.op();
    let event = RequestEvent::new(Some(operation), op.method);
    event.fail(&reason);
    let mut resp = config.reject(op.errors, Some(operation), reason, headers);
    challenge(&mut resp, op.security);
    let resp = for_method(method, resp);
    event.finish(&resp, started);
    (config.on_event)(&event);
    resp
}

/// Returns who is calling: the principal of the first alternative of `requirements` whose every scheme the `Api`
/// accepts and whose scopes its credentials grant. The principal is the one of its first scheme. `None` when no
/// alternative passes but an empty one (`{}`) lets an anonymous request in. A credential that the `Api` refuses never
/// falls back to anonymous.
///
/// # Errors
/// - 401 when no alternative passes.
/// - 403 when one would pass but its credentials lack a scope.
/// - The `Err` of a `verify_` method as it is.
async fn authorize<O: Operations>(
    ops: &O,
    req: &Request,
    requirements: &[&[Requirement]],
    presented: &[(Scheme, String)],
) -> Result<Option<O::Principal>, Reason> {
    if requirements.is_empty() {
        return Ok(None);
    }
    let mut verified: Vec<(&'static str, Option<Verified<O::Principal>>)> = Vec::new();
    let (mut refused, mut lacking) = (false, false);
    'alternative: for alt in requirements.iter().filter(|alt| !alt.is_empty()) {
        let mut principal = None;
        for (scheme, required) in *alt {
            let name = scheme.name();
            let Some((_, credential)) = presented.iter().find(|(s, _)| s.name() == name) else {
                continue 'alternative;
            };
            let at = match verified.iter().position(|(n, _)| *n == name) {
                Some(at) => at,
                None => {
                    let verdict = ops.verify(name, credential, req).await.map_err(Reason::Denied)?;
                    verified.push((name, verdict));
                    verified.len() - 1
                }
            };
            match &verified[at].1 {
                None => {
                    refused = true;
                    continue 'alternative;
                }
                Some(v) if !required.iter().all(|s| v.scopes.contains(s)) => {
                    lacking = true;
                    continue 'alternative;
                }
                Some(v) => {
                    principal.get_or_insert_with(|| v.principal.clone());
                }
            }
        }
        return Ok(principal);
    }
    if lacking {
        let insufficient =
            ApiError::new(http::StatusCode::FORBIDDEN, "the credentials lack a scope that the operation requires")
                .with_header(
                    http::header::WWW_AUTHENTICATE,
                    http::HeaderValue::from_static("Bearer error=\"insufficient_scope\""),
                );
        return Err(Reason::Denied(insufficient));
    }
    if !refused && requirements.iter().any(|alt| alt.is_empty()) {
        return Ok(None);
    }
    Err(Reason::Unauthorized)
}

/// Returns one span per request. Its name and attributes follow the HTTP server conventions of OpenTelemetry.
/// `tracing-opentelemetry` exports it as it is. Your layers, your handler and their logs run inside it.
#[cfg(feature = "tracing")]
async fn traced(
    op: &Op,
    remote: Remote,
    run: impl Future<Output = Result<Response, ApiError>>,
) -> Result<Response, ApiError> {
    let span = tracing::info_span!(
        "request",
        "otel.name" = op.span,
        "otel.kind" = "server",
        "http.request.method" = op.method,
        "http.route" = op.route,
        "http.response.status_code" = tracing::field::Empty,
        "otel.status_code" = tracing::field::Empty,
        operation = op.name,
    );
    remote.adopt(&span);
    let resp = tracing::Instrument::instrument(run, span.clone()).await;
    let status = resp.as_ref().map_or_else(|e| e.status, http::Response::status).as_u16();
    span.record("http.response.status_code", status);
    if status >= http::StatusCode::INTERNAL_SERVER_ERROR.as_u16() {
        span.record("otel.status_code", "ERROR");
    }
    resp
}

#[cfg(not(feature = "tracing"))]
async fn traced(
    _: &Op,
    _: Remote,
    run: impl Future<Output = Result<Response, ApiError>>,
) -> Result<Response, ApiError> {
    run.await
}

#[cfg(test)]
mod tests {
    use crate::ApiError;

    #[test]
    fn a_layers_boxed_error_is_503_unless_it_is_an_api_error() {
        let own: Box<dyn std::error::Error + Send + Sync> =
            Box::new(ApiError::new(http::StatusCode::TOO_MANY_REQUESTS, "slow down"));
        assert_eq!(ApiError::from(own).status, 429, "a later layer's refusal passes through");
        let elapsed: Box<dyn std::error::Error + Send + Sync> = "request timed out".into();
        let e = ApiError::from(elapsed);
        assert_eq!((e.status.as_u16(), e.message.as_str()), (503, "request timed out"));
    }
}
