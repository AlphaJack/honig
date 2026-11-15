//! A request goes through these steps. When a step refuses the request it answers in the error schema of the
//! description:
//!
//! 1. **Routing**: an undeclared path is 404. An undeclared method is 405. A `HEAD` takes the `GET` of its path and
//!    gets its headers without the body.
//! 2. **Credentials**: the `security` of the operation. If none of its requirements are present then the answer is
//!    401.
//! 3. **Your layers** as an `http::Request<Body>` in the order that you add them. The first one is the outermost.
//!    Its extensions hold the generated `Operation`, the `RequestEvent` and the credentials ([`Secured`]). The body
//!    is not read yet. So a refused request costs no body read. [`read_body`] reads it for a layer that needs it.
//! 4. **The body**: read whole up to `max_body_bytes` (413). An operation that streams its body skips this step.
//! 5. **Decoding and validation** (400 and 415). Then your handler runs and its response is encoded.
//!
//! [`Server::authenticate`] finds who is calling and puts the principal in the extensions. When your `Api` names a
//! `Principal` the server builds into a service only once it does.

use std::convert::Infallible;
use std::future::Future;
use std::sync::Arc;
use std::task::Poll;

use tower_layer::{Identity, Layer, Stack};
use tower_service::Service;

use crate::server::adapter::unread;
use crate::server::conditional::Conditions;
use crate::server::operation::{ErrorSchemas, Op};
use crate::server::range::Ranges;
use crate::server::serve::{Call, Config, challenge, for_method};
use crate::trace_context::Remote;
use crate::{ApiError, Body, Reason, Rejection, Request, RequestEvent, Response, SatisfiedSecurity};

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
    fn errors(&self) -> ErrorSchemas;

    /// Serves a request that [`Operations::route`] found an operation for. `call` holds the principal that
    /// [`Server::authenticate`] found if there is one. Each operation takes what its `security` asks for.
    fn call<'r>(
        &'r self,
        request: Request,
        call: Call<'r, Option<Self::Principal>, Self::Operation>,
    ) -> BoxFuture<'r, Response>;
}

/// Whether the server finds who is calling: [`Anonymous`] until [`Server::authenticate`], then [`Authenticated`].
///
/// An `Api` whose `Principal` is `()` builds into a service as it is. Any other `Principal` needs
/// [`Server::authenticate`]: without it `into_service` does not compile. Each operation then takes what its
/// `security` asks for:
///
/// | The operation's `security` | `ctx.principal` | When `authenticate` found nobody |
/// |---|---|---|
/// | none (`security: []`) | `()` | the handler runs |
/// | an empty alternative (`[{}, {apiKey: []}]`) | `Option<Principal>` | the handler runs with `None` |
/// | credentials required | `Principal` | 401 in the error schema |
#[diagnostic::on_unimplemented(
    message = "the `Api` names a `Principal` that no step finds",
    label = "call `.authenticate(..)` on the server before this",
    note = "`authenticate` finds who is calling and gives the principal to the operations"
)]
pub trait Authentication<P>: Send + Sync + 'static {
    /// Returns the principal of a request that no step authenticated: `()` when the `Api` names none.
    #[doc(hidden)]
    fn anonymous() -> Option<P>;
}

/// A server that has not been told how to find who is calling.
#[derive(Debug, Clone, Copy, Default)]
pub struct Anonymous;

/// A server whose [`Server::authenticate`] finds who is calling.
#[derive(Debug, Clone, Copy, Default)]
pub struct Authenticated;

impl Authentication<()> for Anonymous {
    fn anonymous() -> Option<()> {
        Some(())
    }
}

impl<P> Authentication<P> for Authenticated {
    fn anonymous() -> Option<P> {
        None
    }
}

/// The credentials a request presented and the scopes its operation requires.
///
/// A request in your layers and a handler's [`Context`](crate::Context) both hold them. The generated
/// `server::credentials` and `server::scopes_held` read them typed.
pub trait Secured {
    /// Returns credentials of every satisfied security requirement with their scheme. The generated
    /// `server::credentials` reads them typed.
    #[doc(hidden)]
    fn credentials(&self) -> &[(crate::Scheme, String)];

    /// Returns scopes of every satisfied requirement (OR of ANDs) in document order. The generated
    /// `server::scopes_held` and `server::scope_alternatives` read them typed.
    #[doc(hidden)]
    fn requirements(&self) -> &[Vec<&'static str>];

    /// Does a satisfied security requirement rely on `mutualTLS`? A layer that sees so checks the client certificate
    /// and then adds [`ClientCertificateVerified`] to the extensions.
    fn relies_on_mutual_tls(&self) -> bool {
        false
    }

    /// Returns the credentials the request presented by the kind of their scheme, for a layer meant for any API. A
    /// layer for one API matches the generated `server::credentials` instead: one variant per scheme of the
    /// description.
    fn presented(&self) -> Vec<crate::Presented<'_>> {
        self.credentials().iter().map(|(scheme, value)| crate::Presented::of(*scheme, value)).collect()
    }

    /// Does the credential hold the scopes that the operation requires? `held` answers for one scope (from your token
    /// introspection or JWT claims). True when every scope of at least one satisfied requirement is held. Also true
    /// when the operation requires none.
    fn scopes_held(&self, held: impl Fn(&str) -> bool) -> bool {
        let requirements = self.requirements();
        requirements.is_empty() || requirements.iter().any(|alt| alt.iter().all(|s| held(s)))
    }
}

impl<C, Id> Secured for crate::Context<'_, C, Id> {
    fn credentials(&self) -> &[(crate::Scheme, String)] {
        &self.auth
    }

    fn requirements(&self) -> &[Vec<&'static str>] {
        &self.requirements
    }
}

impl Secured for Request {
    fn credentials(&self) -> &[(crate::Scheme, String)] {
        self.extensions().get::<MatchedOperation>().map_or(&[], |r| &r.satisfied.auth)
    }

    fn requirements(&self) -> &[Vec<&'static str>] {
        self.extensions().get::<MatchedOperation>().map_or(&[], |r| &r.satisfied.requirements)
    }

    fn relies_on_mutual_tls(&self) -> bool {
        self.extensions().get::<MatchedOperation>().is_some_and(|r| r.satisfied.mutual_tls)
    }
}

/// The mark of a client certificate that a layer checked.
///
/// A `mutualTLS` security requirement is satisfied only once a layer has checked the client certificate that your
/// TLS terminator verified and put this in the request's extensions. Until then the operation answers 401.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientCertificateVerified;

/// What the server found out about a request before your layers, in its extensions.
///
/// A layer for any API names the operation here, as axum's `MatchedPath`. A layer for one API reads the generated
/// `Operation` instead. The credentials are read through [`Secured`].
#[derive(Debug, Clone)]
pub struct MatchedOperation {
    name: &'static str,
    satisfied: SatisfiedSecurity,
    body_limit: usize,
}

impl MatchedOperation {
    /// Returns the operation as generated code names it (`get_order`).
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
    let limit = req.extensions().get::<MatchedOperation>().map_or(crate::BODY_LIMIT, |r| r.body_limit);
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
    /// The principal of a request that no step authenticated. See [`Authentication::anonymous`].
    anonymous: fn() -> Option<O::Principal>,
}

/// A server for the operations `O` with the tower layers `L` between routing and each operation. `A` is whether it
/// finds who is calling ([`Authentication`]).
///
/// The generated `api::Server::new(api)` starts one. Layers run in the order that they are added. The first one is the
/// outermost as with the `ServiceBuilder` of tower. Each one is a tower `Service` of `http::Request<Body>`. It
/// answers with a [`Response`] or with an error that converts into an [`ApiError`]. That error goes out in the error
/// schema that the description declares. Generic tower layers (`Timeout`, `LoadShed` and `ConcurrencyLimit`) fit
/// too. Their `BoxError` becomes a `503`.
pub struct Server<O: Operations, L = Identity, A = Anonymous> {
    ops: O,
    layer: L,
    config: Config<O::Operation>,
    authentication: std::marker::PhantomData<A>,
}

impl<O: Operations, L, A> std::fmt::Debug for Server<O, L, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server").field("config", &self.config).finish_non_exhaustive()
    }
}

impl<O: Operations> Server<O> {
    /// Creates a server for `ops` without layers: requests reach each operation as they are routed. The generated
    /// `api::Server::new` takes your `Arc<impl Api>` here.
    pub fn new(ops: impl Into<O>) -> Self {
        Self {
            ops: ops.into(),
            layer: Identity::new(),
            config: Config::default(),
            authentication: std::marker::PhantomData,
        }
    }
}

impl<O: Operations, L> Server<O, L, Anonymous> {
    /// Finds who is calling, between the layers added before and those added after. `find` reads the request
    /// (credentials, headers and extensions) and answers with `Some(principal)`, with `None` for nobody, or with an
    /// [`ApiError`] (401, 403 or 429) that goes out in the error schema of the description. The principal goes in the
    /// request's extensions, so the layers after this one read it there, and then to the handlers (`ctx.principal`).
    ///
    /// `find` returns a future that owns what it needs: take the token out of the request before any `.await`.
    pub fn authenticate<F, Fut>(self, find: F) -> Server<O, Stack<AuthenticateLayer<F>, L>, Authenticated>
    where
        F: Fn(&Request) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<O::Principal>, ApiError>> + Send + 'static,
    {
        let Self { ops, layer: outer, config, authentication: _ } = self;
        let layer = Stack::new(AuthenticateLayer(find), outer);
        Server { ops, layer, config, authentication: std::marker::PhantomData }
    }
}

impl<O: Operations, L, A> Server<O, L, A> {
    /// Adds a tower layer between routing and the operations. It goes inside the layers added before. The first one
    /// added is the outermost as with the `ServiceBuilder` of tower. Pass one `ServiceBuilder` to read the order top
    /// to bottom.
    ///
    /// A layer here and one on the axum `Router` that `into_axum` returns differ:
    ///
    /// - An `operation_layer` runs once routing found the operation and its credentials passed. Its extensions hold
    ///   the operation, the credentials and, after `authenticate`, the principal. Its errors go out in the error
    ///   schema of the description. It fits per-principal limits, permissions, signatures and timeouts.
    /// - An axum `Router::layer` runs on every request, also to unknown paths. It knows no operation yet, and its
    ///   errors go out with the body that it writes. It fits CORS, compression and request IDs.
    pub fn operation_layer<T>(self, layer: T) -> Server<O, Stack<T, L>, A> {
        let Self { ops, layer: outer, config, authentication } = self;
        Server { ops, layer: Stack::new(layer, outer), config, authentication }
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
    /// Its answer then has its own body: map it through [`Errors::response`]. A body without `Content-Length` fails
    /// while the server reads it. The server sees a broken body and answers `400`, not `413`.
    #[must_use]
    pub fn max_body_bytes(mut self, limit: usize) -> Self {
        self.config.max_body_bytes = limit;
        self
    }

    /// Sets the response for a request that the server answers without the response of your handler. The cause is no
    /// route, bad input, missing credentials, the `Err` of a layer, validation, a handler `Err` or an encoding failure.
    /// Default: [`Rejection::response`].
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
    pub fn errors(&self) -> Errors<O::Operation> {
        Errors { config: self.config.clone(), schemas: self.ops.errors() }
    }

    /// Returns the server as a tower `Service` over `http` requests. hyper serves it through
    /// `hyper_util::service::TowerToHyperService`.
    pub fn into_service(self) -> ApiService<O, L::Service>
    where
        A: Authentication<O::Principal>,
        L: Layer<Endpoint<O>>,
        L::Service: Service<Request, Response = Response> + Clone + Send + 'static,
        <L::Service as Service<Request>>::Error: Into<ApiError>,
        <L::Service as Service<Request>>::Future: Send + 'static,
    {
        let shared = Arc::new(Shared { ops: self.ops, config: self.config, anonymous: A::anonymous });
        let stack = self.layer.layer(Endpoint { shared: Arc::clone(&shared) });
        ApiService { shared, stack }
    }

    /// Returns an axum `Router` with a route per path template. Merge it next to your own routes or `nest` it under a
    /// prefix. Paths that it does not declare fall through to the fallback of your router.
    #[cfg(feature = "axum")]
    pub fn into_axum(self) -> axum::Router
    where
        A: Authentication<O::Principal>,
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

/// The layer that [`Server::authenticate`] adds.
#[derive(Clone, Debug)]
pub struct AuthenticateLayer<F>(F);

impl<F: Clone, S> Layer<S> for AuthenticateLayer<F> {
    type Service = Authenticate<F, S>;

    fn layer(&self, inner: S) -> Self::Service {
        Authenticate { find: self.0.clone(), inner }
    }
}

/// The service that [`AuthenticateLayer`] wraps around the next layer.
#[derive(Clone, Debug)]
pub struct Authenticate<F, S> {
    find: F,
    inner: S,
}

impl<F, Fut, P, S> Service<Request> for Authenticate<F, S>
where
    F: Fn(&Request) -> Fut,
    Fut: Future<Output = Result<Option<P>, ApiError>> + Send + 'static,
    P: Clone + Send + Sync + 'static,
    S: Service<Request, Response = Response> + Clone + Send + 'static,
    S::Error: Into<ApiError>,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = ApiError;
    type Future = BoxFuture<'static, Result<Response, ApiError>>;

    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> Poll<Result<(), ApiError>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, mut req: Request) -> Self::Future {
        let found = (self.find)(&req);
        // The clone that was polled ready goes with the request; the fresh one stays for the next.
        let fresh = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, fresh);
        Box::pin(async move {
            if let Some(principal) = found.await? {
                req.extensions_mut().insert(principal);
            }
            inner.call(req).await.map_err(Into::into)
        })
    }
}

/// The error responses that the description declares. They render as the server renders its own.
/// [`Server::errors`] returns this.
///
/// A layer outside the server answers with a status and its own body. Examples are `TimeoutLayer` and
/// `RequestBodyLimitLayer` of tower-http and `tower_governor`. Map such a response through [`Errors::response`].
/// Clients then decode it as their typed error.
#[derive(Clone, Debug)]
pub struct Errors<Id> {
    config: Config<Id>,
    schemas: ErrorSchemas,
}

impl<Id: crate::Operation> Errors<Id> {
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
/// It reads the body, decodes, validates, runs your handler and encodes its response. It takes the principal that
/// [`Server::authenticate`] put in the request's extensions.
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
        let principal = extensions.remove::<O::Principal>().or_else(shared.anonymous);
        let mutual_tls_verified = extensions.remove::<ClientCertificateVerified>().is_some();
        Box::pin(async move {
            let Some((operation, event)) = routed else {
                return Err(ApiError::new(
                    http::StatusCode::INTERNAL_SERVER_ERROR,
                    "a request that the server did not route",
                ));
            };
            let call =
                Call { config: &shared.config, operation, satisfied, principal, event: &event, mutual_tls_verified };
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
        let satisfied = crate::check_auth(&head.headers, &crate::parse_query(raw_query), op.security);
        match satisfied {
            Some(satisfied) => (operation, op, satisfied, method),
            None => {
                let event = RequestEvent::new(Some(operation), method);
                event.fail(crate::Failure::Unauthorized, 0);
                let mut resp = config.reject(op.errors, Some(operation), Reason::Unauthorized, head.headers.clone());
                challenge(&mut resp, op.security);
                let resp = for_method(method, resp);
                event.finish(&resp, started);
                (config.on_event)(&event);
                return resp;
            }
        }
    };
    let event = RequestEvent::new(Some(operation), method);
    event.security(&satisfied);
    // The head moves into the layers. A refusal renders by `Accept` alone (a shared buffer and not a copy).
    let accept = head.headers.get(http::header::ACCEPT).cloned();
    let conditions = Conditions::of(&head);
    let ranges = Ranges::of(&head, op.ranges);
    let remote = Remote::of(&head.headers);
    let mut req = Request::from_parts(head, body);
    let extensions = req.extensions_mut();
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
            event.fail(crate::Failure::Forbidden, 0);
            let mut headers = http::HeaderMap::new();
            if let Some(accept) = accept {
                headers.insert(http::header::ACCEPT, accept);
            }
            config.reject(op.errors, Some(operation), Reason::Denied(e), headers)
        }
    };
    // Every 401 carries a challenge (RFC 9110 section 15.5.2): a refusal of the server, of a layer or of a handler.
    challenge(&mut resp, op.security);
    let resp = for_method(method, resp);
    event.finish(&resp, started);
    (config.on_event)(&event);
    resp
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
