//! The wide event and the handler's view of a request.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{Operation, Reason, Request, Response, server::auth::SatisfiedSecurity};

/// One wide event per request. The server puts it in the extensions of the response once the response is ready, so a
/// layer outside the server reads it with `resp.extensions().get::<api::RequestEvent>()`.
///
/// Layers and handlers add fields with `event.set(TENANT, id)`, where `const TENANT: EventKey =
/// EventKey::new("tenant")`. `Id` is the generated `Operation` of the API.
///
/// Clones share the event: the server puts one in the extensions of the request, so a layer gets it with
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
    /// The operation. `None` for a request that no operation matches (404 or 405).
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
    /// Why the server refused the request, when it did.
    pub error: Option<Reason>,
    /// Number of constraint violations of an `invalid` request.
    pub violations: usize,
    /// Fields set by handlers.
    pub extra: Map<String, Value>,
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
    /// Creates a new event for `operation`, or for a request that no operation matches.
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

    /// Records why the request was refused, and how many violations it carries.
    pub fn fail(&self, reason: &Reason) {
        self.with(|f| {
            f.violations = reason.violations().len();
            f.error = Some(reason.clone());
        });
    }

    /// Records the granted schemes and required scopes.
    pub fn security(&self, satisfied: &SatisfiedSecurity) {
        self.with(|f| {
            f.schemes = satisfied.auth.iter().map(|(s, _)| s.name()).collect();
            f.scopes.clone_from(&satisfied.scopes);
        });
    }

    /// Records the size of the request body the server received (a streamed one counts as 0).
    pub fn received(&self, request_bytes: usize) {
        self.with(|f| f.request_bytes = request_bytes);
    }

    /// Returns a copy of the typed fields.
    #[must_use]
    pub fn snapshot(&self) -> RequestEventFields<Id> {
        self.with(|f| f.clone())
    }
}

impl<Id: Operation> RequestEvent<Id> {
    /// Records the outcome (status, duration and response size) and puts the event in the extensions of `resp`.
    pub(crate) fn finish(&self, mut resp: Response, started: std::time::Instant) -> Response {
        let duration_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.with(|f| {
            f.status = resp.status().as_u16();
            f.duration_us = duration_us;
            f.response_bytes = resp.body().len();
        });
        resp.extensions_mut().insert(self.clone());
        resp
    }

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
            if let Some(e) = &f.error {
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
    /// Credentials of the satisfied security requirements with their scheme: get them with the generated
    /// `server::credentials` or with [`Secured::presented`](crate::Secured::presented).
    pub(crate) auth: Vec<(crate::auth::Scheme, String)>,
    /// The operation being served: one variant of the generated `Operation`.
    pub operation: Id,
    /// The request's wide event: add fields with [`RequestEvent::set`].
    pub event: &'a RequestEvent<Id>,
    /// Who is calling: the generated `Security` of the operation, with what the `verify_` method of each scheme of
    /// the alternative that passed found.
    pub security: C,
}

impl<'a, Id> Context<'a, (), Id> {
    /// Creates context of a request for `operation` with the credentials that satisfied
    /// its security requirement. It records into `event`.
    pub fn new(req: &'a Request, operation: Id, satisfied: SatisfiedSecurity, event: &'a RequestEvent<Id>) -> Self {
        Self { request: req, auth: satisfied.auth, operation, event, security: () }
    }

    /// Returns the context with who is calling.
    #[must_use]
    pub fn with_security<C>(self, security: C) -> Context<'a, C, Id> {
        let Self { request, auth, operation, event, security: () } = self;
        Context { request, auth, operation, event, security }
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
