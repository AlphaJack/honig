//! What a server answers to a request it rejects.

use std::fmt;

use serde_json::{Map, Value};

use crate::{ApiError, Error, Message, Response, Violation};

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
    /// The body does not decode for a reason that names no place in the request (400).
    Body(String),
    /// The body could not be read. The connection broke or a read timeout of yours passed. It holds why (400).
    Unreadable(String),
    /// What the request holds is wrong (400): a parameter or a value of the body that is missing, does not decode or
    /// breaks a constraint. Each violation says where (`/query/limit`, `/body/sku`) and why.
    Invalid(Vec<Violation>),
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
        let mut r = crate::new_response(self.status().as_u16());
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

    /// Returns how many violations the reason holds.
    pub(crate) fn violations(&self) -> &[Violation] {
        if let Self::Invalid(v) = self { v } else { &[] }
    }

    /// Returns [`Reason::Invalid`] with one violation: `path` is a JSON pointer into the request (`/query/limit`).
    #[must_use]
    pub fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid(vec![Violation { path: path.into(), message: message.into() }])
    }

    /// Returns the response for an [`ApiError`] that carries its own body. It goes out as it is.
    pub fn own(&self) -> Option<Response> {
        let (Self::Denied(e) | Self::Handler(e)) = self else { return None };
        let (media, bytes) = e.body.clone()?;
        let mut r = self.head();
        r.set_body(media, bytes);
        Some(r)
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

    /// Returns RFC 9457 problem details: `type`, `title`, `status` and `detail`. Also
    /// `violations` for [`Reason::Invalid`] and `Allow` for 405.
    pub fn problem(&self) -> Response {
        let status = self.status().as_u16();
        let mut body = Map::new();
        body.insert("type".into(), Value::String("about:blank".into()));
        body.insert("title".into(), Value::String(reason(status).into()));
        body.insert("status".into(), Value::from(status));
        if let Some(d) = self.detail() {
            body.insert("detail".into(), Value::String(d));
        }
        if let Self::Invalid(v) = self {
            body.insert("violations".into(), serde_json::to_value(v).unwrap_or_default());
        }
        let mut r = self.head();
        r.set_body("application/problem+json", serde_json::to_vec(&Value::Object(body)).unwrap_or_default());
        r
    }
}

/// What a field of the description's error schema receives from a [`Reason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorField {
    /// The HTTP status as a number (`code` or `status`).
    Status,
    /// The HTTP status as text (`code` declared as a string).
    StatusText,
    /// The reason phrase (`title`).
    Title,
    /// What went wrong (`message`, `detail` or `error`).
    Detail,
    /// `about:blank` (`type`).
    Type,
    /// Every violation as `{path, message}` (`errors` or `violations`).
    Violations,
    /// Every violation as one string each (`errors` of strings).
    ViolationMessages,
    /// A required `const` (`object: {const: error}`): its one value, as JSON text.
    Const(&'static str),
    /// A required field nothing maps to: its type's empty value.
    Empty(&'static str),
}

/// The error schema of the description as [`Reason::in_schema`] fills it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorSchema {
    /// Media type of the error response.
    pub media: &'static str,
    /// (property, what it receives). None: a `text/*` string body that holds the detail and nothing else.
    pub fields: &'static [(&'static str, ErrorField)],
    /// The schema takes properties it does not declare: validation details go in `violations` when no field
    /// lists them.
    pub open: bool,
}

impl Reason {
    /// Returns the reason in the description's own error schema.
    pub fn in_schema(&self, schema: &ErrorSchema) -> Response {
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
                ErrorField::Status => Value::from(status),
                ErrorField::StatusText => Value::String(status.to_string()),
                ErrorField::Title => Value::String(reason(status).into()),
                ErrorField::Detail => Value::String(detail.clone()),
                ErrorField::Type => Value::String("about:blank".into()),
                ErrorField::Violations => serde_json::to_value(violations).unwrap_or_default(),
                ErrorField::ViolationMessages => Value::Array(
                    violations.iter().map(|v| Value::String(format!("{}: {}", v.path, v.message))).collect(),
                ),
                ErrorField::Const(json) => serde_json::from_str(json).unwrap_or_default(),
                ErrorField::Empty(kind) => match *kind {
                    "integer" | "number" => Value::from(0),
                    "boolean" => Value::Bool(false),
                    "array" => Value::Array(vec![]),
                    "object" => Value::Object(Map::new()),
                    _ => Value::String(String::new()),
                },
            };
            o.insert((*name).to_owned(), v);
        }
        // Validation details survive even when the schema has no list for them.
        let listed =
            schema.fields.iter().any(|(_, f)| matches!(f, ErrorField::Violations | ErrorField::ViolationMessages));
        if schema.open && !listed && !violations.is_empty() {
            o.insert("violations".into(), serde_json::to_value(violations).unwrap_or_default());
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

/// Returns `400 Bad Request` listing every violation as JSON.
pub fn violations_response(violations: &[Violation]) -> Response {
    let mut r = crate::new_response(http::StatusCode::BAD_REQUEST.as_u16());
    let body = serde_json::to_vec(&serde_json::json!({ "violations": violations })).unwrap_or_default();
    r.set_body("application/json", body);
    r
}

impl Reason {
    /// Returns `413` past `limit`. `400` when the body could not be read.
    #[must_use]
    pub fn unread_body(e: crate::BodyError, limit: usize) -> Self {
        match e {
            crate::BodyError::TooLarge => Self::PayloadTooLarge(limit),
            crate::BodyError::Unreadable(why) => Self::Unreadable(why),
        }
    }
}

/// Is the `Content-Type` of the body one that the operation declares? A request that names none passes. Many clients
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

/// Checks a JSON request body as sent against the whole schema of the body.
///
/// Decoding into the generated type drops what it does not hold and picks one variant. So `unevaluatedProperties`,
/// an overlapping `oneOf` or nested `prefixItems` are judged here on the raw value. A body that is not JSON passes:
/// decoding reports it.
///
/// # Errors
/// [`Reason::Invalid`] with every violation, at `/body/...`.
pub fn check_raw_json(body: &[u8], schema: &Value) -> Result<(), Reason> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else { return Ok(()) };
    let mut out = vec![];
    crate::validate::check_schema(
        &mut out,
        &crate::InstancePath::Key(&crate::InstancePath::Root, "body"),
        &value,
        schema,
    );
    if out.is_empty() { Ok(()) } else { Err(Reason::Invalid(out)) }
}

impl From<Error> for Reason {
    fn from(e: Error) -> Self {
        match e {
            // A request body that does not decode: the server decodes requests with status 0.
            Error::Decode { status: 0, pointer, message, .. } => {
                Self::Invalid(vec![decode_violation(&pointer, &message)])
            }
            Error::Invalid(v) => Self::Invalid(v),
            Error::Transport(why) => Self::Unreadable(why),
            Error::TooLarge(limit) => Self::PayloadTooLarge(limit),
            other @ (Error::Encode(_)
            | Error::Decode { .. }
            | Error::UnexpectedMedia { .. }
            | Error::UnexpectedStatus { .. }
            | Error::NotModified { .. }) => Self::Body(other.to_string()),
        }
    }
}

/// A body or an item still arriving that failed, with the status of its [`Reason`]. So `?` on a chunk or an item that
/// a handler reads works.
impl From<Error> for ApiError {
    fn from(e: Error) -> Self {
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
            Self::Invalid(v) => {
                for (i, v) in v.iter().enumerate() {
                    write!(f, "{}{}: {}", if i == 0 { "" } else { "; " }, v.path, v.message)?;
                }
                Ok(())
            }
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
    pub schema: Option<&'static ErrorSchema>,
    /// The headers of the request. An error response may then follow its `Accept`. A request that one of your layers
    /// refused has moved on with its headers. Only its `Accept` is here.
    pub headers: http::HeaderMap,
}

impl<Id> Rejection<Id> {
    /// Returns the body that your handler or a layer set ([`ApiError::with_body`]). Otherwise the reason in the error
    /// schema of the description ([`Rejection::schema`]). Otherwise RFC 9457 `problem+json`.
    pub fn response(&self) -> Response {
        self.reason
            .own()
            .unwrap_or_else(|| self.schema.map_or_else(|| self.reason.problem(), |s| self.reason.in_schema(s)))
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
        let schema = ErrorSchema {
            media: "application/json",
            fields: &[
                ("message", ErrorField::Detail),
                ("object", ErrorField::Const("\"error\"")),
                ("version", ErrorField::Const("2")),
            ],
            open: false,
        };
        let r = Reason::invalid("/query/limit", "is required").in_schema(&schema);
        let body: Value = serde_json::from_slice(r.body().bytes()).unwrap();
        assert_eq!(body, serde_json::json!({"message": "/query/limit: is required", "object": "error", "version": 2}));
    }

    #[test]
    fn a_text_error_body_is_the_detail() {
        let schema = ErrorSchema { media: "text/plain", fields: &[], open: false };
        let r = Reason::invalid("/query/limit", "is required").in_schema(&schema);
        assert_eq!(r.body().bytes(), b"/query/limit: is required");
        assert_eq!(r.headers().get("content-type").map(|v| v.to_str().unwrap()), Some("text/plain"));
    }

    /// A request body that the server fails to decode, as `json_decode(0, body)` reports it.
    fn undecoded(pointer: &str, message: &str) -> Reason {
        Reason::from(Error::Decode {
            status: 0,
            message: message.to_owned(),
            body: bytes::Bytes::new(),
            pointer: pointer.to_owned(),
        })
    }

    #[test]
    fn a_body_that_does_not_decode_lists_one_violation() {
        let r = undecoded("/sku", "`bad` does not match `^[A-Z]{3}$` at line 1 column 42");
        assert_eq!(r.to_string(), "/body/sku: `bad` does not match `^[A-Z]{3}$`");
        let body: Value = serde_json::from_slice(r.problem().body().bytes()).unwrap();
        assert_eq!(
            body["violations"],
            serde_json::json!([{"path": "/body/sku", "message": "`bad` does not match `^[A-Z]{3}$`"}])
        );
    }

    #[test]
    fn a_missing_field_is_required_at_its_own_path() {
        let r = undecoded("", "missing field `name` at line 1 column 18");
        assert_eq!(r.to_string(), "/body/name: is required");
        let schema = ErrorSchema {
            media: "application/json",
            fields: &[("code", ErrorField::Status), ("message", ErrorField::Detail)],
            open: true,
        };
        let body: Value = serde_json::from_slice(r.in_schema(&schema).body().bytes()).unwrap();
        assert_eq!(body["violations"], serde_json::json!([{"path": "/body/name", "message": "is required"}]));
        let nested = undecoded("/items/0", "missing field `id`");
        assert_eq!(nested.violations()[0].path, "/body/items/0/id");
        let unknown = undecoded("", "unknown field `b`, expected `a` at line 1 column 12");
        assert_eq!(unknown.to_string(), "/body/b: is not allowed (additionalProperties)");
    }

    #[test]
    fn a_body_checked_whole_is_judged_as_sent() {
        let schema = serde_json::json!({"properties": {"foo": {}}, "unevaluatedProperties": false});
        assert!(check_raw_json(br#"{"foo": 1}"#, &schema).is_ok());
        let Err(Reason::Invalid(v)) = check_raw_json(br#"{"foo": 1, "bar": 2}"#, &schema) else {
            panic!("an unevaluated property is refused");
        };
        assert_eq!(v.first().map(|v| v.path.as_str()), Some("/body/bar"));
        assert!(check_raw_json(b"not json", &schema).is_ok(), "decoding reports it");
    }

    #[test]
    fn a_union_that_no_branch_matches_names_the_field_of_the_closest_branch() {
        let urls = serde_json::json!({"oneOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]});
        let schema = serde_json::json!({"oneOf": [
            {"type": "object", "required": ["status", "alerts"], "properties": {"urls": urls}},
            {"type": "object", "not": {"required": ["status"]}, "properties": {"urls": urls}},
        ]});
        let first = |body: &[u8]| match check_raw_json(body, &schema) {
            Err(Reason::Invalid(v)) => v.into_iter().next().map(|v| (v.path, v.message)),
            other => panic!("refused as invalid: {other:?}"),
        };
        // The second branch only fails on `urls`: the first also misses `status` and `alerts`.
        let (path, message) = first(br#"{"urls": 5}"#).unwrap();
        assert_eq!((path.as_str(), message.as_str()), ("/body/urls", "matches 0 of `oneOf`, not exactly one"));
        // Every branch fails on the value itself: the union's own message.
        let (path, message) = first(b"5").unwrap();
        assert_eq!((path.as_str(), message.as_str()), ("/body", "matches 0 of `oneOf`, not exactly one"));
        // Each branch misses its own required property: neither got further.
        let charge = serde_json::json!({"oneOf": [{"required": ["refund"]}, {"required": ["fee"]}]});
        let Err(Reason::Invalid(v)) = check_raw_json(br#"{"neither": 1}"#, &charge) else { panic!("refused") };
        assert_eq!(v.first().map(|v| v.path.as_str()), Some("/body"));
        let any = serde_json::json!({"anyOf": [{"properties": {"a": {"type": "string"}}}, {"type": "array"}]});
        let Err(Reason::Invalid(v)) = check_raw_json(br#"{"a": 1}"#, &any) else { panic!("refused") };
        assert_eq!(v.first().map(|v| v.path.as_str()), Some("/body/a"));
    }

    /// A required field that nothing maps to gets the empty value of its type.
    #[test]
    fn a_required_field_gets_the_empty_value_of_its_type() {
        static SCHEMA: ErrorSchema = ErrorSchema {
            media: "application/json",
            fields: &[
                ("n", ErrorField::Empty("integer")),
                ("ok", ErrorField::Empty("boolean")),
                ("list", ErrorField::Empty("array")),
                ("more", ErrorField::Empty("object")),
                ("text", ErrorField::Empty("string")),
            ],
            open: false,
        };
        let r = Rejection::<()> {
            reason: Reason::NotFound,
            operation: None,
            schema: Some(&SCHEMA),
            headers: http::HeaderMap::new(),
        };
        assert_eq!(r.status(), http::StatusCode::NOT_FOUND);
        let body: Value = serde_json::from_slice(r.response().body().bytes()).unwrap();
        assert_eq!(body, serde_json::json!({"n": 0, "ok": false, "list": [], "more": {}, "text": ""}));
    }
}
