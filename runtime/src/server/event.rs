//! The wide event and the handler's view of a request.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{Operation, Reason, Request, Response, SatisfiedSecurity};

/// One wide event per request. It goes to the `on_event` of the server.
///
/// Layers and handlers add fields with `event.set(TENANT, id)`, where `const TENANT: EventKey =
/// EventKey::new("tenant")`. `Id` is the generated `Operation` of the API.
///
/// Clones share the event: the server puts one in the extensions of the request, so a layer reads it with
/// `req.extensions().get::<api::RequestEvent>()`.
#[derive(Debug)]
pub struct RequestEvent<Id = ()>(std::sync::Arc<std::sync::Mutex<RequestEventFields<Id>>>);

impl<Id> Clone for RequestEvent<Id> {
    fn clone(&self) -> Self {
        Self(std::sync::Arc::clone(&self.0))
    }
}

/// The fields of a [`RequestEvent`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RequestEventFields<Id = ()> {
    /// The operation. `None` for a request that no operation takes (404 or 405).
    pub operation: Option<Id>,
    /// HTTP method in upper case.
    pub method: &'static str,
    /// Path template or the webhook name.
    pub route: &'static str,
    /// Response status.
    pub status: u16,
    /// Time from routing to the encoded response.
    pub duration_us: u64,
    /// Request body size.
    pub request_bytes: usize,
    /// Response body size.
    pub response_bytes: usize,
    /// Security schemes whose credentials were presented.
    pub schemes: Vec<&'static str>,
    /// Scopes the operation requires.
    pub scopes: Vec<&'static str>,
    /// What went wrong, when something did.
    pub error: Option<Failure>,
    /// Number of constraint violations of an `invalid` request.
    pub violations: usize,
    /// Fields set by handlers.
    pub extra: Map<String, Value>,
}

/// What kind of failure a request met: the `error` of its [`RequestEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Failure {
    /// No route matches the path (404).
    NotFound,
    /// The path exists but not for the method (405).
    MethodNotAllowed,
    /// `Accept` takes none of the response media types (406).
    NotAcceptable,
    /// The body is larger than the limit (413).
    PayloadTooLarge,
    /// A parameter or the body does not decode, or its media type is not declared (400 or 415).
    BadRequest,
    /// The request breaks schema constraints (400).
    Invalid,
    /// No credentials, or credentials a layer refused (401).
    Unauthorized,
    /// A layer refused the request (403, 429 or another status).
    Forbidden,
    /// The handler returned `Err`.
    Handler,
    /// The handler's response could not be encoded (500).
    Encode,
}

impl Failure {
    /// Returns the kind of failure that `reason` is.
    pub(crate) fn of(reason: &Reason) -> Self {
        match reason {
            Reason::NotFound => Self::NotFound,
            Reason::MethodNotAllowed(_) => Self::MethodNotAllowed,
            Reason::NotAcceptable(_) => Self::NotAcceptable,
            Reason::PayloadTooLarge(_) => Self::PayloadTooLarge,
            Reason::UnsupportedMediaType(..) | Reason::Body(_) | Reason::Unreadable(_) => Self::BadRequest,
            Reason::Invalid(_) => Self::Invalid,
            Reason::Unauthorized => Self::Unauthorized,
            Reason::Denied(e) if e.status == http::StatusCode::UNAUTHORIZED => Self::Unauthorized,
            Reason::Denied(_) => Self::Forbidden,
            Reason::Handler(_) => Self::Handler,
            Reason::Encode(_) => Self::Encode,
        }
    }

    /// Returns the name in the event's JSON and log line (`not_found`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::NotAcceptable => "not_acceptable",
            Self::PayloadTooLarge => "payload_too_large",
            Self::BadRequest => "bad_request",
            Self::Invalid => "invalid",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::Handler => "api_error",
            Self::Encode => "encode_error",
        }
    }
}

/// The name of a field that you add to a [`RequestEvent`].
///
/// A `const` key with the name of a built-in field fails to
/// compile: `const STATUS: EventKey = EventKey::new("status")`. Any other key with such a name panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventKey(&'static str);

/// The names of the fields that every event has.
const BUILT_IN: [&str; 11] = [
    "operation",
    "method",
    "route",
    "status",
    "duration_us",
    "request_bytes",
    "response_bytes",
    "schemes",
    "scopes",
    "error",
    "violations",
];

impl EventKey {
    /// Creates a key named `name`.
    ///
    /// # Panics
    /// `name` is the name of a built-in field. In a `const` that is a compile error.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        let mut i = 0;
        while i < BUILT_IN.len() {
            assert!(!BUILT_IN[i].eq_ignore_ascii_case(name), "the name of a built-in field of the event");
            i += 1;
        }
        Self(name)
    }

    /// Returns the name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.0
    }
}

impl<Id: Operation> RequestEvent<Id> {
    /// Creates a new event for `operation`, or for a request that no operation takes.
    pub fn new(operation: Option<Id>, method: &'static str) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(RequestEventFields {
            operation,
            method,
            route: operation.map_or("", Operation::route),
            status: 0,
            duration_us: 0,
            request_bytes: 0,
            response_bytes: 0,
            schemes: Vec::new(),
            scopes: Vec::new(),
            error: None,
            violations: 0,
            extra: Map::new(),
        })))
    }
}

impl<Id: Clone> RequestEvent<Id> {
    fn with<R>(&self, f: impl FnOnce(&mut RequestEventFields<Id>) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Sets a field of your own (any serializable value; others become `null`).
    pub fn set(&self, key: EventKey, value: impl Serialize) {
        let value = serde_json::to_value(value).unwrap_or(Value::Null);
        self.with(|f| f.extra.insert(key.0.to_owned(), value));
    }

    /// Records a failure (and how many violations for [`Failure::Invalid`]).
    pub fn fail(&self, error: Failure, violations: usize) {
        self.with(|f| {
            f.error = Some(error);
            f.violations = violations;
        });
    }

    /// Records the granted schemes and required scopes.
    pub fn security(&self, satisfied: &SatisfiedSecurity) {
        self.with(|f| {
            f.schemes = satisfied.auth.iter().map(|(s, _)| s.name()).collect();
            f.scopes.clone_from(&satisfied.scopes);
        });
    }

    /// Records the size of the request body the server read (a streamed one counts as 0).
    pub fn received(&self, request_bytes: usize) {
        self.with(|f| f.request_bytes = request_bytes);
    }

    /// Records the outcome: status, duration and response size.
    pub fn finish(&self, resp: &Response, started: std::time::Instant) {
        let duration_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.with(|f| {
            f.status = resp.status().as_u16();
            f.duration_us = duration_us;
            f.response_bytes = resp.body().len();
        });
    }

    /// Returns a copy of the typed fields.
    #[must_use]
    pub fn snapshot(&self) -> RequestEventFields<Id> {
        self.with(|f| f.clone())
    }
}

impl<Id: Operation> RequestEvent<Id> {
    /// Returns every field as one JSON object (typed fields and then handler fields).
    #[must_use]
    pub fn fields(&self) -> Map<String, Value> {
        self.with(|f| {
            let mut o = Map::new();
            o.insert("operation".into(), f.operation.map_or("", Operation::name).into());
            o.insert("method".into(), f.method.into());
            o.insert("route".into(), f.route.into());
            o.insert("status".into(), f.status.into());
            o.insert("duration_us".into(), f.duration_us.into());
            o.insert("request_bytes".into(), f.request_bytes.into());
            o.insert("response_bytes".into(), f.response_bytes.into());
            if !f.schemes.is_empty() {
                o.insert("schemes".into(), f.schemes.clone().into());
            }
            if !f.scopes.is_empty() {
                o.insert("scopes".into(), f.scopes.clone().into());
            }
            if let Some(e) = f.error {
                o.insert("error".into(), e.name().into());
            }
            if f.violations > 0 {
                o.insert("violations".into(), f.violations.into());
            }
            o.extend(f.extra.clone());
            o
        })
    }

    /// Returns the fields as one JSON object such as for a log line.
    #[must_use]
    pub fn to_json(&self) -> String {
        Value::Object(self.fields()).to_string()
    }
}

/// What a server implementation sees about the request.
#[derive(Debug, Clone)]
pub struct Context<'a, C = (), Id = ()> {
    /// The request as received.
    pub request: &'a Request,
    /// Credentials of the satisfied security requirements with their scheme: read them with the generated
    /// `server::credentials` or with [`Secured::presented`](crate::Secured::presented).
    pub(crate) auth: Vec<(crate::Scheme, String)>,
    /// Scopes of every satisfied requirement (OR of ANDs) in document order: check them with the generated
    /// `server::scopes_held`.
    pub(crate) requirements: Vec<Vec<&'static str>>,
    /// A satisfied security requirement relies on `mutualTLS`. Only your TLS layer can verify it.
    pub mutual_tls: bool,
    /// The operation being served: one variant of the generated `Operation`.
    pub operation: Id,
    /// The request's wide event: add fields with [`RequestEvent::set`].
    pub event: &'a RequestEvent<Id>,
    /// Who is calling: what [`Server::authenticate`](crate::Server::authenticate) found.
    pub principal: C,
}

impl<'a, Id> Context<'a, (), Id> {
    /// Creates context of a request for `operation` with the credentials that satisfied
    /// its security requirement. It records into `event`.
    pub fn new(req: &'a Request, operation: Id, satisfied: SatisfiedSecurity, event: &'a RequestEvent<Id>) -> Self {
        Self {
            request: req,
            auth: satisfied.auth,
            requirements: satisfied.requirements,
            mutual_tls: satisfied.mutual_tls,
            operation,
            event,
            principal: (),
        }
    }

    /// Returns the context with who is calling.
    #[must_use]
    pub fn with_principal<C>(self, principal: C) -> Context<'a, C, Id> {
        let Self { request, auth, requirements, mutual_tls, operation, event, principal: () } = self;
        Context { request, auth, requirements, mutual_tls, operation, event, principal }
    }
}

impl<'a, C, Id> Context<'a, C, Id> {
    /// Returns the value of type `T` that the server framework or a tower layer attached to the request. Examples are a
    /// `tower_sessions::Session` and a verified user.
    #[must_use]
    pub fn extension<T: Send + Sync + 'static>(&self) -> Option<&'a T> {
        self.request.extensions().get()
    }

    /// Returns HTTP method in upper case.
    pub fn method(&self) -> &str {
        self.request.method().as_str()
    }

    /// Returns the path and query string as received.
    pub fn path(&self) -> &str {
        self.request.uri().path_and_query().map_or("/", http::uri::PathAndQuery::as_str)
    }

    /// Returns the request's headers.
    #[must_use]
    pub fn headers(&self) -> &'a http::HeaderMap {
        self.request.headers()
    }

    /// Returns the address of the client when the framework knows it: the `ConnectInfo<SocketAddr>` of axum or a
    /// `SocketAddr` in the request extensions.
    pub fn peer(&self) -> Option<std::net::SocketAddr> {
        peer(self.request.extensions())
    }
}

/// Returns the address of the client. That is the `ConnectInfo<SocketAddr>` of axum or a `SocketAddr` in the request
/// extensions.
pub fn peer(extensions: &http::Extensions) -> Option<std::net::SocketAddr> {
    #[cfg(feature = "axum")]
    {
        use axum::extract::{ConnectInfo, connect_info::MockConnectInfo};
        if let Some(addr) = extensions
            .get::<ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0)
            .or_else(|| extensions.get::<MockConnectInfo<std::net::SocketAddr>>().map(|m| m.0))
        {
            return Some(addr);
        }
    }
    extensions.get().copied()
}
