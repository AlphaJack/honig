//! What a server answers to a request it rejects.

use std::fmt;

use serde_json::{Map, Value};

use crate::{ApiError, ClientError, Message, Response, Violation};

/// Request bodies larger than this get `413` from the server adapters
/// unless the server's `max_body_bytes` says otherwise.
pub const BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Why the server answered with an error. Each reason has its own status. The [`Rejection`] that the server's
/// `error_response` renders holds it (default: RFC 9457 `problem+json`).
///
/// A handler that reads a request body as it arrives gets it as the error of a chunk or an item. `?` turns it into
/// an [`ApiError`] with the same status.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reason {
    /// No route matches the path (404).
    NotFound,
    /// The path exists but the method does not (405). It holds the allowed methods.
    MethodNotAllowed(Vec<&'static str>),
    /// `Accept` takes none of the response media types the operation offers (406): those types.
    NotAcceptable(Vec<&'static str>),
    /// The request body exceeds the limit in bytes (413).
    PayloadTooLarge(usize),
    /// The `Content-Type` of the body is none that the operation declares: (sent, declared) (415).
    UnsupportedMediaType(String, String),
    /// The body does not decode for a reason that points to no place in the request (400).
    Body(String),
    /// The body could not be read. The connection broke or a read timeout of yours passed. It holds why (400).
    Unreadable(String),
    /// What the request holds is wrong (400): a parameter or a value of the body that is missing, does not decode or
    /// breaks a constraint. The violation says where (`/query/limit`, `/body/sku`) and why. A request is refused at its
    /// first violation, so there is one.
    Invalid(Violation),
    /// No security requirement was satisfied (401).
    Unauthorized,
    /// One of your layers refused the request.
    Denied(ApiError),
    /// The handler returned `Err`.
    Handler(ApiError),
    /// The handler's response could not be encoded (500).
    Encode(String),
}

/// Appends a header that a reason carries. One that is not a valid header is left out.
fn add_header(r: &mut Response, name: &str, value: std::borrow::Cow<'static, str>) {
    // The response still goes out without it: such a header cannot be written on the wire.
    r.set_header(name, value).ok();
}

/// Returns the `Allow` of a path: its methods and `OPTIONS`, which the server answers itself where the path declares
/// none.
pub(crate) fn allow_header(allow: &[&str]) -> String {
    let mut methods = allow.join(", ");
    if !allow.contains(&"OPTIONS") {
        methods.push_str(", OPTIONS");
    }
    methods
}

/// Returns the reason phrase of a status or else `Error`.
fn reason(status: u16) -> &'static str {
    crate::status::reason(status).unwrap_or("Error")
}

impl Reason {
    /// Returns a response with the status and the headers that the reason carries. That is `Allow` for 405 and the
    /// headers of the [`ApiError`] that a layer or your handler returned.
    pub fn head(&self) -> Response {
        let mut r = crate::message::new_response(self.status().as_u16());
        match self {
            Self::MethodNotAllowed(allow) => add_header(&mut r, "Allow", allow_header(allow).into()),
            Self::Denied(e) | Self::Handler(e) => {
                r.headers_mut().extend(e.headers.as_ref().clone());
            }
            Self::NotFound
            | Self::NotAcceptable(_)
            | Self::PayloadTooLarge(_)
            | Self::UnsupportedMediaType(..)
            | Self::Body(_)
            | Self::Unreadable(_)
            | Self::Invalid(_)
            | Self::Unauthorized
            | Self::Encode(_) => {}
        }
        r
    }

    /// Returns the violations that the reason holds: one for [`Reason::Invalid`], none otherwise.
    pub(crate) fn violations(&self) -> &[Violation] {
        if let Self::Invalid(v) = self { std::slice::from_ref(v) } else { &[] }
    }

    /// Returns [`Reason::Invalid`] with one violation: `path` is a JSON pointer into the request (`/query/limit`).
    #[must_use]
    pub fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid(Violation { path: path.into(), message: message.into() })
    }

    /// Returns the response for an [`ApiError`] that carries its own body. It goes out as it is.
    pub fn own(&self) -> Option<Response> {
        let (Self::Denied(e) | Self::Handler(e)) = self else { return None };
        let (media, bytes) = e.body.clone()?;
        let mut r = self.head();
        r.set_body(media, bytes);
        Some(r)
    }

    /// Returns the name of the reason in the wide event's JSON and log line (`not_found`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::MethodNotAllowed(_) => "method_not_allowed",
            Self::NotAcceptable(_) => "not_acceptable",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::UnsupportedMediaType(..) | Self::Body(_) | Self::Unreadable(_) => "bad_request",
            Self::Invalid(_) => "invalid",
            Self::Unauthorized => "unauthorized",
            Self::Denied(e) if e.status == http::StatusCode::UNAUTHORIZED => "unauthorized",
            Self::Denied(_) => "forbidden",
            Self::Handler(_) => "api_error",
            Self::Encode(_) => "encode_error",
        }
    }

    /// Returns the HTTP status.
    #[must_use]
    pub fn status(&self) -> http::StatusCode {
        use http::StatusCode as S;
        match self {
            Self::NotFound => S::NOT_FOUND,
            Self::MethodNotAllowed(_) => S::METHOD_NOT_ALLOWED,
            Self::NotAcceptable(_) => S::NOT_ACCEPTABLE,
            Self::PayloadTooLarge(_) => S::PAYLOAD_TOO_LARGE,
            Self::UnsupportedMediaType(..) => S::UNSUPPORTED_MEDIA_TYPE,
            Self::Body(_) | Self::Unreadable(_) | Self::Invalid(_) => S::BAD_REQUEST,
            Self::Unauthorized => S::UNAUTHORIZED,
            Self::Denied(e) | Self::Handler(e) => e.status,
            Self::Encode(_) => S::INTERNAL_SERVER_ERROR,
        }
    }

    /// Returns human-readable detail if there is any.
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::NotFound | Self::Unauthorized | Self::MethodNotAllowed(_) => None,
            Self::Denied(e) | Self::Handler(e) => (!e.message.is_empty()).then(|| e.message.clone()),
            Self::NotAcceptable(_)
            | Self::PayloadTooLarge(_)
            | Self::UnsupportedMediaType(..)
            | Self::Body(_)
            | Self::Unreadable(_)
            | Self::Invalid(_)
            | Self::Encode(_) => Some(self.to_string()),
        }
    }

    /// Returns RFC 9457 problem details: `type`, `title`, `status` and `detail`. Also `errors` for
    /// [`Reason::Invalid`]. The headers of the reason (`Allow` for 405) go with it.
    #[must_use]
    pub fn problem(&self) -> Problem {
        let status = self.status().as_u16();
        Problem {
            r#type: "about:blank".to_owned(),
            title: reason(status).to_owned(),
            status,
            detail: self.detail(),
            instance: None,
            errors: self.violations().iter().map(ProblemError::of).collect(),
            headers: self.head().headers().clone(),
        }
    }
}

/// RFC 9457 problem details, the body of `application/problem+json`. [`Rejection::problem`] returns the one the
/// server would send. Change a member and send it with [`Problem::into_response`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Problem {
    /// A URI that identifies the problem type. `about:blank`: the problem is what the status says.
    pub r#type: String,
    /// The reason phrase of the status.
    pub title: String,
    /// The HTTP status.
    pub status: u16,
    /// What went wrong in this occurrence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// A URI that identifies this occurrence, such as a request ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    /// An extension member: where and why the request is invalid.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ProblemError>,
    /// The headers of the response, such as `Allow` of a 405. Not part of the body.
    #[serde(skip)]
    pub headers: http::HeaderMap,
}

/// One entry of the `errors` extension member of a [`Problem`]: the shape of the validation example of RFC 9457
/// (section 3).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProblemError {
    /// Why the value is invalid.
    pub detail: String,
    /// A JSON Pointer in its URI fragment form: into the request content for a value of the body (`#/sku`), into the
    /// request otherwise (`#/query/limit`).
    pub pointer: String,
}

impl ProblemError {
    fn of(v: &Violation) -> Self {
        let pointer = match v.path.strip_prefix("/body") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("#{rest}"),
            _ => format!("#{}", v.path),
        };
        Self { detail: v.message.clone(), pointer }
    }
}

impl Problem {
    /// Returns the response: the status, the headers and the problem as `application/problem+json`.
    #[must_use]
    pub fn into_response(self) -> Response {
        let mut r = crate::message::new_response(self.status);
        let body = serde_json::to_vec(&self).unwrap_or_default();
        *r.headers_mut() = self.headers;
        r.set_body("application/problem+json", body);
        r
    }
}

/// What a member of the description's error body receives from a [`Reason`]: a member of RFC 9457 `problem+json`
/// by its name in the RFC, or a required `const`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemMember {
    /// `status`: the HTTP status.
    Status,
    /// `title`: the reason phrase.
    Title,
    /// `detail`: what went wrong.
    Detail,
    /// `type`: `about:blank`.
    Type,
    /// A required `const`: its one value, as JSON text.
    Const(&'static str),
}

/// The error schema of the description as [`Reason::in_schema`] fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProblemSchema {
    /// Media type of the error response.
    pub media: &'static str,
    /// (property, what it receives). None: a `text/*` string body that holds the detail and nothing else.
    pub fields: &'static [(&'static str, ProblemMember)],
    /// The schema takes properties it does not declare: validation details go in `violations` when no field
    /// lists them.
    pub open: bool,
}

impl Reason {
    /// Returns the reason in the description's own error schema.
    pub fn in_schema(&self, schema: &ProblemSchema) -> Response {
        let status = self.status().as_u16();
        let detail = self.detail().unwrap_or_else(|| reason(status).to_owned());
        if schema.fields.is_empty() {
            let mut r = self.head();
            r.set_body(schema.media, detail.into_bytes());
            return r;
        }
        let violations = self.violations();
        let mut o = Map::new();
        for (name, field) in schema.fields {
            let v = match field {
                ProblemMember::Status => Value::from(status),
                ProblemMember::Title => Value::String(reason(status).into()),
                ProblemMember::Detail => Value::String(detail.clone()),
                ProblemMember::Type => Value::String("about:blank".into()),
                ProblemMember::Const(json) => serde_json::from_str(json).unwrap_or_default(),
            };
            o.insert((*name).to_owned(), v);
        }
        // An extension member of RFC 9457, where the body takes members that it does not declare.
        if schema.open && !violations.is_empty() {
            o.insert("errors".into(), errors_member(violations));
        }
        let mut r = self.head();
        r.set_body(schema.media, serde_json::to_vec(&Value::Object(o)).unwrap_or_default());
        r
    }
}

/// Returns the violation of a request body that does not decode at `pointer`, which is relative to the body. A missing
/// field is reported at its own path as `is required`, and an undeclared one under `additionalProperties: false` as
/// `is not allowed`, as validation reports them. The decoder's `at line 1 column 42`
/// is left out because the path says where.
fn decode_violation(pointer: &str, why: &str) -> Violation {
    let why = why.rsplit_once(" at line ").filter(|(_, at)| at.contains(" column ")).map_or(why, |(why, _)| why);
    let at = |field: &str| format!("/body{pointer}/{}", field.replace('~', "~0").replace('/', "~1"));
    if let Some(field) = why.strip_prefix("missing field `").and_then(|rest| rest.strip_suffix('`')) {
        return Violation { path: at(field), message: "is required".to_owned() };
    }
    if let Some((field, _)) = why.strip_prefix("unknown field `").and_then(|rest| rest.split_once('`')) {
        return Violation { path: at(field), message: "is not allowed (additionalProperties)".to_owned() };
    }
    Violation { path: format!("/body{pointer}"), message: why.to_owned() }
}

/// Returns the `errors` extension member of a problem.
fn errors_member(violations: &[Violation]) -> Value {
    serde_json::to_value(violations.iter().map(ProblemError::of).collect::<Vec<_>>()).unwrap_or_default()
}

impl Reason {
    /// Returns `413` past `limit`. `400` when the body could not be read.
    #[must_use]
    pub fn unread_body(e: crate::source::BodyError, limit: usize) -> Self {
        match e {
            crate::source::BodyError::TooLarge => Self::PayloadTooLarge(limit),
            crate::source::BodyError::Unreadable(why) => Self::Unreadable(why),
        }
    }
}

/// Is the `Content-Type` of the body one that the operation declares? A request that states none passes. Many clients
/// leave it out. The body then decodes or not on its own.
///
/// # Errors
/// [`Reason::UnsupportedMediaType`] with what was sent and what is accepted.
pub fn check_media(actual: &str, declared: &[&str]) -> Result<(), Reason> {
    if crate::media::declared_media(actual, declared) {
        return Ok(());
    }
    Err(Reason::UnsupportedMediaType(actual.to_owned(), declared.join(", ")))
}

impl From<ClientError> for Reason {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Decode { status: None, pointer, message, .. } => {
                Self::Invalid(decode_violation(&pointer, &message))
            }
            ClientError::Transport(why) => Self::Unreadable(why),
            ClientError::TooLarge(limit) => Self::PayloadTooLarge(limit),
            other @ (ClientError::Encode(_)
            | ClientError::Decode { .. }
            | ClientError::UnexpectedMedia { .. }
            | ClientError::UnexpectedStatus { .. }
            | ClientError::NotModified { .. }) => Self::Body(other.to_string()),
        }
    }
}

/// A body or an item still arriving that failed, with the status of its [`Reason`]. So `?` on a chunk or an item that
/// a handler reads works.
impl From<ClientError> for ApiError {
    fn from(e: ClientError) -> Self {
        Reason::from(e).into()
    }
}

impl From<Reason> for ApiError {
    /// Returns the error of your layer or handler as it was. Any other reason with its status and detail.
    fn from(reason: Reason) -> Self {
        match reason {
            Reason::Denied(e) | Reason::Handler(e) => e,
            other @ (Reason::NotFound
            | Reason::MethodNotAllowed(_)
            | Reason::NotAcceptable(_)
            | Reason::PayloadTooLarge(_)
            | Reason::UnsupportedMediaType(..)
            | Reason::Body(_)
            | Reason::Unreadable(_)
            | Reason::Invalid(_)
            | Reason::Unauthorized
            | Reason::Encode(_)) => Self::new(other.status(), other.detail().unwrap_or_default()),
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("no route matches the path"),
            Self::MethodNotAllowed(allow) => write!(f, "method not allowed; allowed: {}", allow.join(", ")),
            Self::NotAcceptable(offered) => write!(f, "`Accept` takes none of: {}", offered.join(", ")),
            Self::PayloadTooLarge(limit) => write!(f, "request body exceeds {limit} bytes"),
            Self::UnsupportedMediaType(sent, declared) => {
                write!(f, "Content-Type `{sent}` is not one of: {declared}")
            }
            Self::Body(why) | Self::Encode(why) => f.write_str(why),
            Self::Unreadable(why) => write!(f, "unreadable request body: {why}"),
            Self::Invalid(v) => write!(f, "{}: {}", v.path, v.message),
            Self::Unauthorized => f.write_str("no security requirement is satisfied"),
            Self::Denied(e) | Self::Handler(e) => write!(f, "{}: {}", e.status, e.message),
        }
    }
}

impl std::error::Error for Reason {}

/// A request that the server answers without your handler's response. The server's `error_response` renders it.
/// `Id` is the generated `Operation` of the API.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Rejection<Id = ()> {
    /// Why.
    pub reason: Reason,
    /// The operation the request reached; `None` when no route matched.
    pub operation: Option<Id>,
    /// The error schema that the description declares for this operation and status when the generator can fill it.
    pub schema: Option<&'static ProblemSchema>,
    /// The headers of the request. An error response may then follow its `Accept`. A request that one of your layers
    /// refused has moved on with its headers. Only its `Accept` is here.
    pub headers: http::HeaderMap,
}

impl<Id> Rejection<Id> {
    /// Returns the body that your handler or a layer set ([`ApiError::with_body`]). Otherwise the reason in the error
    /// body of the description that the server fills by name ([`Rejection::schema`]). Otherwise RFC 9457
    /// `problem+json`. It is the default `error_response` of an `Api` whose error bodies are all filled so.
    pub fn response(&self) -> Response {
        self.reason.own().unwrap_or_else(|| {
            self.schema.map_or_else(|| self.reason.problem().into_response(), |s| self.reason.in_schema(s))
        })
    }

    /// Returns `body` as the JSON error body of the reason, with its status and headers. A body that your handler or a
    /// layer set ([`ApiError::with_body`]) goes out instead. A `body` that does not serialize gives
    /// [`Rejection::response`].
    pub fn json<T: serde::Serialize + ?Sized>(&self, body: &T) -> Response {
        if let Some(own) = self.reason.own() {
            return own;
        }
        match serde_json::to_vec(body) {
            Ok(bytes) => {
                let mut r = self.reason.head();
                r.set_body("application/json", bytes);
                r
            }
            Err(_) => self.response(),
        }
    }

    /// Returns what went wrong: the detail of the reason, or the reason phrase of its status.
    #[must_use]
    pub fn message(&self) -> String {
        self.reason.detail().unwrap_or_else(|| reason(self.status().as_u16()).to_owned())
    }

    /// Returns the reason as RFC 9457 problem details. Set `instance` or another member, then send it with
    /// [`Problem::into_response`].
    #[must_use]
    pub fn problem(&self) -> Problem {
        self.reason.problem()
    }

    /// Returns the HTTP status.
    #[must_use]
    pub fn status(&self) -> http::StatusCode {
        self.reason.status()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_const_field_gets_its_value() {
        let schema = ProblemSchema {
            media: "application/json",
            fields: &[
                ("detail", ProblemMember::Detail),
                ("object", ProblemMember::Const("\"error\"")),
                ("version", ProblemMember::Const("2")),
            ],
            open: false,
        };
        let r = Reason::invalid("/query/limit", "is required").in_schema(&schema);
        let body: Value = serde_json::from_slice(r.body().bytes()).unwrap();
        assert_eq!(body, serde_json::json!({"detail": "/query/limit: is required", "object": "error", "version": 2}));
    }

    #[test]
    fn a_text_error_body_is_the_detail() {
        let schema = ProblemSchema { media: "text/plain", fields: &[], open: false };
        let r = Reason::invalid("/query/limit", "is required").in_schema(&schema);
        assert_eq!(r.body().bytes(), b"/query/limit: is required");
        assert_eq!(r.headers().get("content-type").map(|v| v.to_str().unwrap()), Some("text/plain"));
    }

    /// A request body that the server fails to decode, as `json_decode(None, body)` reports it.
    fn undecoded(pointer: &str, message: &str) -> Reason {
        Reason::from(ClientError::Decode {
            status: None,
            message: message.to_owned(),
            body: bytes::Bytes::new(),
            pointer: pointer.to_owned(),
        })
    }

    #[test]
    fn a_body_that_does_not_decode_lists_one_violation() {
        let r = undecoded("/sku", "`bad` does not match `^[A-Z]{3}$` at line 1 column 42");
        assert_eq!(r.to_string(), "/body/sku: `bad` does not match `^[A-Z]{3}$`");
        let body: Value = serde_json::from_slice(r.problem().into_response().body().bytes()).unwrap();
        assert_eq!(
            body["errors"],
            serde_json::json!([{"detail": "`bad` does not match `^[A-Z]{3}$`", "pointer": "#/sku"}])
        );
    }

    #[test]
    fn a_missing_field_is_required_at_its_own_path() {
        let r = undecoded("", "missing field `name` at line 1 column 18");
        assert_eq!(r.to_string(), "/body/name: is required");
        let schema = ProblemSchema {
            media: "application/json",
            fields: &[("status", ProblemMember::Status), ("detail", ProblemMember::Detail)],
            open: true,
        };
        let body: Value = serde_json::from_slice(r.in_schema(&schema).body().bytes()).unwrap();
        assert_eq!(body["errors"], serde_json::json!([{"detail": "is required", "pointer": "#/name"}]));
        let nested = undecoded("/items/0", "missing field `id`");
        assert_eq!(nested.violations()[0].path, "/body/items/0/id");
        let unknown = undecoded("", "unknown field `b`, expected `a` at line 1 column 12");
        assert_eq!(unknown.to_string(), "/body/b: is not allowed (additionalProperties)");
    }

    #[test]
    fn a_problem_takes_an_instance_and_keeps_its_headers() {
        let rejection = Rejection::<()> {
            reason: Reason::MethodNotAllowed(vec!["GET"]),
            operation: None,
            schema: None,
            headers: http::HeaderMap::new(),
        };
        let mut problem = rejection.problem();
        problem.instance = Some("urn:request:7".to_owned());
        let r = problem.into_response();
        assert_eq!(r.headers().get("allow").map(|v| v.to_str().unwrap()), Some("GET, OPTIONS"));
        assert_eq!(r.headers().get("content-type").map(|v| v.to_str().unwrap()), Some("application/problem+json"));
        let body: Value = serde_json::from_slice(r.body().bytes()).unwrap();
        assert_eq!(
            body,
            serde_json::json!({"type": "about:blank", "title": "Method Not Allowed", "status": 405, "instance": "urn:request:7"})
        );
    }

    #[test]
    fn a_status_without_a_reason_phrase_is_titled_error() {
        let status = http::StatusCode::from_u16(599).unwrap();
        let problem = Reason::Handler(ApiError::new(status, "upstream")).problem();
        assert_eq!(problem.title, "Error");
    }

    #[test]
    fn a_json_error_body_takes_the_status_and_headers_of_the_reason() {
        let rejection =
            |reason| Rejection::<()> { reason, operation: None, schema: None, headers: http::HeaderMap::new() };
        let r = rejection(Reason::MethodNotAllowed(vec!["GET"]));
        let resp = r.json(&serde_json::json!({"code": 405, "message": r.message()}));
        assert_eq!(resp.status(), 405);
        assert_eq!(resp.headers().get("allow").map(|v| v.to_str().unwrap()), Some("GET, OPTIONS"));
        let body: Value = serde_json::from_slice(resp.body().bytes()).unwrap();
        assert_eq!(body, serde_json::json!({"code": 405, "message": "Method Not Allowed"}));
        let own = ApiError::new(http::StatusCode::CONFLICT, "taken").with_body("text/plain", "own");
        let resp = rejection(Reason::Handler(own)).json(&serde_json::json!({"code": 409}));
        assert_eq!(resp.body().bytes(), b"\"own\"", "the handler's own body goes out");
    }

    /// A request is refused at its first violation: the reason holds that one, and the body lists it.
    #[test]
    fn an_invalid_request_holds_its_one_violation() {
        let reason = Reason::invalid("/query/limit", "must be at most 100");
        let Reason::Invalid(Violation { path, message }) = &reason else { panic!("invalid") };
        assert_eq!((path.as_str(), message.as_str()), ("/query/limit", "must be at most 100"));
        assert_eq!(reason.to_string(), "/query/limit: must be at most 100");
    }
}
