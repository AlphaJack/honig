//! Serving one operation once your layers let it through. The steps are the body, decoding, validation, your handler
//! and encoding in that order. Generated code gives what differs per operation. That is an [`Op`], a decoder and an
//! encoder.

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

use super::operation::{Op, Operation, ProblemSchemas};
use crate::{
    ApiError, Body, Context, Message, Reason, Rejection, Request, RequestEvent, Response, auth::Requirement,
    auth::Scheme, params::Query, reject::BODY_LIMIT, server::auth::SatisfiedSecurity,
};

/// What the server does besides routing and your layers: the builder options of [`Server`](crate::Server). `Id` is
/// the generated `Operation` of the API.
pub struct Config<Id> {
    /// The largest request body buffered, in bytes.
    pub(crate) max_body_bytes: usize,
    /// Renders a request the server answers without your handler's response.
    pub(crate) errors: Arc<dyn Fn(Rejection<Id>) -> Response + Send + Sync>,
    /// Receives each request's wide event.
    pub(crate) on_event: EventSink<Id>,
}

/// What receives each request's wide event.
type EventSink<Id> = Arc<dyn Fn(&RequestEvent<Id>) + Send + Sync>;

impl<Id> Clone for Config<Id> {
    fn clone(&self) -> Self {
        Self {
            max_body_bytes: self.max_body_bytes,
            errors: Arc::clone(&self.errors),
            on_event: Arc::clone(&self.on_event),
        }
    }
}

impl<Id: Operation> Default for Config<Id> {
    fn default() -> Self {
        Self {
            max_body_bytes: BODY_LIMIT,
            errors: Arc::new(|rejection: Rejection<Id>| rejection.response()),
            on_event: Arc::new(log),
        }
    }
}

impl<Id> std::fmt::Debug for Config<Id> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config").field("max_body_bytes", &self.max_body_bytes).finish_non_exhaustive()
    }
}

impl<Id: Operation> Config<Id> {
    /// Returns the response to `reason` through the error renderer.
    pub(crate) fn reject(
        &self,
        errors: ProblemSchemas,
        operation: Option<Id>,
        reason: Reason,
        headers: http::HeaderMap,
    ) -> Response {
        let schema = errors(operation.map(Operation::name), reason.status().as_u16());
        (self.errors)(Rejection { reason, operation, schema, headers })
    }

    /// Returns the response to a request that no operation matches (404 or 405) with its own wide event. An `OPTIONS`
    /// of a path that declares no `options` gets `204` with the methods of the path in `Allow` (RFC 9110 section
    /// 9.3.7).
    pub(crate) fn unrouted(&self, errors: ProblemSchemas, head: &http::request::Parts, reason: Reason) -> Response {
        let started = std::time::Instant::now();
        let method = known_method(head.method.as_str());
        let event = RequestEvent::new(None, method);
        if let (true, Reason::MethodNotAllowed(allow)) = (head.method == http::Method::OPTIONS, &reason) {
            let mut resp = crate::message::new_response(http::StatusCode::NO_CONTENT.as_u16());
            if let Ok(value) = http::HeaderValue::from_str(&crate::reject::allow_header(allow)) {
                resp.headers_mut().insert(http::header::ALLOW, value);
            }
            event.finish(&resp, started);
            (self.on_event)(&event);
            return resp;
        }
        event.fail(&reason);
        let resp = for_method(method, self.reject(errors, None, reason, head.headers.clone()));
        event.finish(&resp, started);
        (self.on_event)(&event);
        resp
    }
}

#[cfg(feature = "tracing")]
fn log<Id: Operation>(event: &RequestEvent<Id>) {
    let f = event.snapshot();
    tracing::info!(
        target: "api",
        operation = f.operation.map_or("", Operation::name),
        method = f.method,
        route = f.route,
        status = f.status,
        duration_us = f.duration_us,
        request_bytes = f.request_bytes,
        response_bytes = f.response_bytes,
        schemes = %f.schemes.join(","),
        scopes = %f.scopes.join(","),
        error = f.error.as_ref().map(Reason::name),
        violations = f.violations,
        // `tracing` fixes the names of its fields at compile time. So the fields that you set go as one JSON object.
        fields = %serde_json::Value::Object(f.extra),
        "request"
    );
}

#[cfg(not(feature = "tracing"))]
fn log<Id>(_: &RequestEvent<Id>) {}

/// A routed request.
#[derive(Debug)]
pub struct Routed<'r> {
    /// The request; an operation that consumes its body moves it out.
    pub request: &'r mut Request,
    /// The path template's variables, in order.
    pub caps: &'r [&'r str],
    /// The query string without `?`.
    pub raw_query: &'r str,
    /// The query string's pairs, decoded.
    pub query: &'r Query<'r>,
}

/// What an operation's parameter decoder decodes.
#[derive(Debug)]
pub struct Incoming<'r> {
    /// The request; its body is empty when the operation consumes it.
    pub request: &'r Request,
    /// The path template's variables, in order.
    pub caps: &'r [&'r str],
    /// The query string without `?`.
    pub raw_query: &'r str,
    /// The query string's pairs, decoded.
    pub query: &'r Query<'r>,
    /// The body. It is moved out of the request when the operation consumes it. Otherwise it is borrowed.
    pub body: Cow<'r, [u8]>,
    /// The body still to come when the operation consumes it as it arrives. Otherwise empty.
    pub stream: crate::ByteStream,
    /// The index in [`Op::produces`] of the media type `Accept` chose: what the handler answers in.
    pub media: usize,
}

/// What your layers found out about a request on its way to the operation. Generated code passes it through.
#[derive(Debug)]
pub struct Call<'r, C, Id> {
    pub(crate) config: &'r Config<Id>,
    pub(crate) operation: Id,
    pub(crate) satisfied: SatisfiedSecurity,
    pub(crate) principal: C,
    pub(crate) event: &'r RequestEvent<Id>,
    /// The target that was routed and where its path variables are.
    pub(crate) uri: http::Uri,
    pub(crate) spans: crate::server::route::Spans,
}

impl<'r, C, Id> Call<'r, Option<C>, Id> {
    /// Drops the principal for an operation without `security`: its handler sees none.
    pub fn public(self) -> Call<'r, (), Id> {
        let Self { config, operation, satisfied, principal: _, event, uri, spans } = self;
        Call { config, operation, satisfied, principal: (), event, uri, spans }
    }
}

impl<C, Id: Copy> Call<'_, C, Id> {
    /// Returns the operation that the server routed the request to.
    pub const fn operation(&self) -> Id {
        self.operation
    }

    /// Returns the target that the server routed: its path variables are in it, wherever a layer took the request.
    #[must_use]
    pub const fn uri(&self) -> &http::Uri {
        &self.uri
    }

    /// Returns the path variables in `path`, the routed path as [`normalize_path`](crate::server::route::normalize_path) gives it.
    #[must_use]
    pub fn captures<'p>(&self, path: &'p str) -> crate::server::route::Captures<'p> {
        self.spans.captures(path)
    }
}

/// Returns the response as `method` gets it. A `HEAD` gets the headers of the `GET` without its body (RFC 9110 section
/// 9.3.2). A body in memory keeps its length as `Content-Length`. A streamed one is never consumed.
pub(crate) fn for_method(method: &str, resp: Response) -> Response {
    if method != "HEAD" {
        return resp;
    }
    let (mut head, body) = resp.into_parts();
    if !body.is_streamed() && !head.headers.contains_key(http::header::CONTENT_LENGTH) {
        head.headers.insert(http::header::CONTENT_LENGTH, http::HeaderValue::from(body.len()));
    }
    Response::from_parts(head, Body::default())
}

/// Returns the name of the method for an event. An event stores `'static` text. So an unknown method is `OTHER`.
pub(crate) fn known_method(method: &str) -> &'static str {
    const KNOWN: [&str; 9] = ["GET", "HEAD", "POST", "PUT", "DELETE", "CONNECT", "OPTIONS", "TRACE", "PATCH"];
    KNOWN.into_iter().find(|m| m.eq_ignore_ascii_case(method)).unwrap_or("OTHER")
}

/// Returns the name of the method for the event of a request that reached `op`. A method of the description's own
/// (`additionalOperations`, such as `PROPFIND`) is the operation's: the route matched it exactly.
pub(crate) fn routed_method(method: &str, op: &Op) -> &'static str {
    if op.method == method { op.method } else { known_method(method) }
}

/// The future of an operation's own steps.
pub type StepFuture<'r> = std::pin::Pin<Box<dyn Future<Output = Result<Response, Reason>> + Send + 'r>>;

/// The steps that differ per operation besides its [`Op`].
///
/// They decode the parameters, check them, run your handler and write its response.
/// [`Steps`] implements it for each operation. Behind a trait object [`serve`] is compiled once for your server and
/// not once per operation.
pub trait OpSteps<C, Id>: Sync {
    /// Returns the steps for one request from what it carries and its context.
    fn run<'r>(&'r self, incoming: Incoming<'r>, ctx: &'r Context<'r, C, Id>) -> StepFuture<'r>;
}

/// What the future of a handler resolves to in a box. [`Steps::call`] returns it.
pub type HandlerFuture<'r, R> = std::pin::Pin<Box<dyn Future<Output = Result<R, ApiError>> + Send + 'r>>;

/// The steps of one operation from its own functions. Generated code fills one per operation.
pub struct Steps<'a, A, C, P, R, Id> {
    /// Your server.
    pub api: &'a A,
    /// Reads the parameters.
    pub decode: for<'r> fn(Incoming<'r>) -> Result<P, Reason>,
    /// Runs your handler.
    pub call: for<'r> fn(&'r A, P, &'r Context<'r, C, Id>) -> HandlerFuture<'r, R>,
    /// Writes its response in the media type that `Accept` chose (an index in [`Op::produces`]).
    pub encode: fn(R, usize) -> Result<Response, crate::ClientError>,
}

impl<A, C, P, R, Id> std::fmt::Debug for Steps<'_, A, C, P, R, Id> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Steps").finish_non_exhaustive()
    }
}

impl<A: Sync, C: Sync, P: Send, R, Id: Send + Sync> OpSteps<C, Id> for Steps<'_, A, C, P, R, Id> {
    fn run<'r>(&'r self, incoming: Incoming<'r>, ctx: &'r Context<'r, C, Id>) -> StepFuture<'r> {
        Box::pin(async move {
            let media = incoming.media;
            // Decoding checks every keyword of the description: the handler gets only valid parameters.
            let params = (self.decode)(incoming)?;
            let response = (self.call)(self.api, params, ctx).await.map_err(Reason::Handler)?;
            (self.encode)(response, media).map_err(|e| Reason::Encode(e.to_string()))
        })
    }
}

/// Serves `routed` as the operation of `call` for the principal that your layers found.
///
/// Read the body, decode, validate, run your handler and encode its response. Every failure goes through the error
/// renderer.
pub async fn serve<C, Id: Operation>(
    routed: Routed<'_>,
    call: Call<'_, C, Id>,
    steps: &dyn OpSteps<C, Id>,
) -> Response {
    let Routed { request, caps, raw_query, query } = routed;
    let Call { config, operation, satisfied, principal, event, .. } = call;
    let op = operation.op();
    // The headers of the request go along for the error renderer. They are moved out at the failure and not up front.
    let fail = |headers: http::HeaderMap, reason: Reason| {
        event.fail(&reason);
        config.reject(op.errors, Some(operation), reason, headers)
    };
    if let Err(reason) = crate::params::check_query(raw_query) {
        return fail(request.headers().clone(), reason);
    }
    // 406 before the body is consumed: no answer the client accepts is worth receiving it for.
    let media = if op.produces.is_empty() {
        0
    } else {
        match crate::media::negotiate(request.header("accept").as_deref(), op.produces) {
            Some(i) => i,
            None => {
                return fail(request.headers().clone(), Reason::NotAcceptable(op.produces.to_vec()));
            }
        }
    };
    // Every body but a binary or sequential one is buffered whole here once your layers let the request through.
    if let Some(unread) = (!op.takes_body).then(|| request.body().chunks().take_source()).flatten() {
        match unread.read_all(config.max_body_bytes).await {
            Ok(body) => *request.body_mut() = Body::new(body),
            Err(e) => {
                return fail(request.headers().clone(), Reason::unread_body(e, config.max_body_bytes));
            }
        }
    }
    // Before the body may move into the parameters; a streamed one counts as 0.
    event.received(request.body().len());
    let taken = op.takes_body.then(|| std::mem::take(request.body_mut()));
    let request: &Request = request;
    let ctx = Context::new(request, operation, satisfied, event).with_principal(principal);
    let (body, stream) = match taken {
        Some(taken) => {
            let (bytes, stream) = taken.into_parts();
            (Cow::Owned(Vec::from(bytes)), stream)
        }
        None => (Cow::Borrowed(request.body().bytes()), crate::ByteStream::default()),
    };
    let incoming = Incoming { request, caps, raw_query, query, body, stream, media };
    match steps.run(incoming, &ctx).await {
        Ok(resp) => resp,
        Err(reason) => fail(request.headers().clone(), reason),
    }
}

/// Serves `routed` as the operation of `call` for the principal that your layers found. Its `security` requires
/// credentials. Without a principal the answer is 401.
pub async fn serve_signed_in<C, Id: Operation>(
    routed: Routed<'_>,
    call: Call<'_, Option<C>, Id>,
    steps: &dyn OpSteps<C, Id>,
) -> Response {
    let Call { config, operation, satisfied, principal, event, uri, spans } = call;
    let Some(principal) = principal else {
        event.fail(&Reason::Unauthorized);
        let errors = operation.op().errors;
        return config.reject(errors, Some(operation), Reason::Unauthorized, routed.request.headers().clone());
    };
    serve(routed, Call { config, operation, satisfied, principal, event, uri, spans }, steps).await
}

/// Adds the `WWW-Authenticate` challenge of a 401 (RFC 9110 section 11.6.1). There is one per HTTP scheme that
/// `security` accepts. The error renderer may set its own.
pub(crate) fn challenge(resp: &mut Response, security: &[&[Requirement]]) {
    if resp.status() != http::StatusCode::UNAUTHORIZED || resp.headers().contains_key(http::header::WWW_AUTHENTICATE) {
        return;
    }
    let mut seen: Vec<&str> = Vec::new();
    for (scheme, _) in security.iter().flat_map(|alt| alt.iter()) {
        let token = match scheme {
            #[cfg(feature = "base64")]
            Scheme::Basic(_) => "Basic",
            Scheme::Bearer(_) => "Bearer",
            Scheme::Http(_, token) => token,
            Scheme::ApiKey(..) | Scheme::MutualTls(_) => continue,
        };
        if !seen.contains(&token) {
            seen.push(token);
            if let Ok(token) = http::HeaderValue::from_str(token) {
                resp.headers_mut().append(http::header::WWW_AUTHENTICATE, token);
            }
        }
    }
}

/// One operation for the tests of the runtime.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TestOp;

#[cfg(test)]
impl Operation for TestOp {
    fn op(self) -> &'static Op {
        static OP: Op = Op {
            name: "update_product",
            method: "PUT",
            route: "/products/{id}",
            span: "PUT /products/{id}",
            security: &[],
            takes_body: false,
            errors: |_, _| None,
            produces: &[],
            ranges: false,
        };
        &OP
    }
}
