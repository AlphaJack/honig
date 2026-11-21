//! What a generated client call returns when it fails.

use std::borrow::Cow;
use std::fmt;

#[derive(Debug)]
#[non_exhaustive]
/// Client-side error of a generated operation call.
pub enum ClientError {
    /// The request could not be sent or the connection broke off (connection or TLS for example).
    Transport(String),
    /// A parameter or the body could not be serialized.
    Encode(String),
    /// A documented response does not fit its schema. `pointer` says where.
    Decode {
        /// HTTP status code of the response, `None` for a request that a server decodes.
        status: Option<u16>,
        /// What failed to decode.
        message: String,
        /// Raw response body.
        body: bytes::Bytes,
        /// JSON pointer of the value that failed (`/attachments/0/url`); empty for the whole body.
        pointer: String,
    },
    /// The status is declared but the response's `Content-Type` is none that the description declares for it.
    UnexpectedMedia {
        /// HTTP status code.
        status: u16,
        /// The response's `Content-Type`; empty when it sent none.
        content_type: String,
        /// The media types that the description declares joined with `, `.
        declared: String,
        /// Raw response body. It is empty for a stream because that is left unread.
        body: bytes::Bytes,
    },
    /// `304 Not Modified` to a request that sent `If-None-Match` or `If-Modified-Since`. The operation declares no
    /// `304`. The representation that the client holds is still current.
    NotModified {
        /// The headers of the response: its `ETag`, `Last-Modified` and caching directives.
        headers: http::HeaderMap,
    },
    /// A status that the operation declares neither as itself nor as a `default`.
    UnexpectedStatus {
        /// HTTP status code.
        status: http::StatusCode,
        /// Raw response body. Its `Debug` shows it as text.
        body: bytes::Bytes,
    },
    /// A body still arriving went past a limit in bytes: the `max_body_bytes` of the server.
    TooLarge(usize),
}

impl ClientError {
    /// Returns a transport failure from the error of the HTTP client or from a message. The message holds the error and
    /// each of its causes (`error sending request: connection closed before message completed`). The own message of an
    /// HTTP client often says only which step failed.
    pub fn transport<'e>(e: impl Into<Box<dyn std::error::Error + 'e>>) -> Self {
        let e = e.into();
        let mut message = e.to_string();
        let mut cause = e.source();
        while let Some(c) = cause {
            let text = c.to_string();
            // Some errors repeat their cause in their own message.
            if !message.ends_with(&text) {
                message.push_str(": ");
                message.push_str(&text);
            }
            cause = c.source();
        }
        Self::Transport(message)
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(m) => write!(f, "transport error: {m}"),
            Self::Encode(m) => write!(f, "cannot encode request: {m}"),
            Self::Decode { status, message, pointer, .. } => {
                match status {
                    None => f.write_str("cannot decode request")?,
                    Some(status) => write!(f, "cannot decode {status} response")?,
                }
                if !pointer.is_empty() {
                    write!(f, " at {pointer}")?;
                }
                write!(f, ": {message}")
            }
            Self::UnexpectedMedia { status, content_type, declared, .. } => {
                let sent = if content_type.is_empty() { "no Content-Type" } else { content_type.as_str() };
                write!(f, "unexpected media type in {status} response: {sent}, declared {declared}")
            }
            Self::NotModified { .. } => f.write_str("not modified"),
            Self::UnexpectedStatus { status, body } => {
                let text = String::from_utf8_lossy(body.get(..body.len().min(200)).unwrap_or_default());
                write!(f, "unexpected status {}: {text}", status.as_u16())
            }
            Self::TooLarge(limit) => write!(f, "body exceeds {limit} bytes"),
        }
    }
}

impl std::error::Error for ClientError {}

/// An error returned by a server implementation. Becomes a response through the server's `error_response`.
///
/// With a [body](ApiError::with_body) it is sent as it is. Without one the default fills the error schema that the
/// description declares for the operation and status. Otherwise it uses RFC 9457 problem details. The generated
/// `From<types::YourError>` sets the body from the type of your error schema.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApiError {
    /// HTTP status code of the response.
    pub status: http::StatusCode,
    /// What went wrong. It is the detail of the error schema when there is no body. It is always the error of the wide
    /// event.
    pub message: String,
    /// Headers that the error response carries: `WWW-Authenticate` on a 401 and `Retry-After` on a 429 or 503. Boxed:
    /// an `ApiError` stays small enough to return in a `Result` that clippy does not flag.
    pub headers: Box<http::HeaderMap>,
    /// The response body as it is sent: (media type, bytes).
    pub body: Option<(Cow<'static, str>, bytes::Bytes)>,
}

impl ApiError {
    /// Creates an error with this status and message: `ApiError::new(StatusCode::NOT_FOUND, "no such order")`.
    #[must_use]
    pub fn new(status: http::StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), headers: Box::default(), body: None }
    }

    /// Sets the error's body to `body` serialized as JSON under `media` (`application/problem+json`). `body` is
    /// your own error type whatever schema it follows. A body that does not serialize leaves the error without one.
    #[must_use]
    pub fn with_body<T: serde::Serialize + ?Sized>(mut self, media: impl Into<Cow<'static, str>>, body: &T) -> Self {
        self.body = serde_json::to_vec(&crate::finite::Finite(body)).ok().map(|bytes| (media.into(), bytes.into()));
        self
    }

    /// Adds one more response header to the error: `ApiError::new(StatusCode::TOO_MANY_REQUESTS, "slow
    /// down").with_header(header::RETRY_AFTER, HeaderValue::from_static("60"))`.
    #[must_use]
    pub fn with_header(mut self, name: http::HeaderName, value: http::HeaderValue) -> Self {
        self.headers.append(name, value);
        self
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status.as_u16(), self.message)
    }
}

impl std::error::Error for ApiError {}

/// The error of a generic tower layer. So `Timeout`, `LoadShed` and `ConcurrencyLimit` fit between routing and the
/// operations. An [`ApiError`] that a later layer returned passes through as it is. Any other error is
/// `503 Service Unavailable`, such as a timeout or a shed request.
impl From<Box<dyn std::error::Error + Send + Sync>> for ApiError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        match e.downcast::<Self>() {
            Ok(own) => *own,
            Err(other) => Self::new(http::StatusCode::SERVICE_UNAVAILABLE, other.to_string()),
        }
    }
}

/// A value that a handler built and that its schema refuses. A handler's inputs are checked already, so the value
/// came from the server's own data: `500 Internal Server Error`.
impl From<crate::BuildError> for ApiError {
    fn from(e: crate::BuildError) -> Self {
        Self::new(http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiError, ClientError};

    /// `?` on a value that a handler built turns its schema's refusal into a `500`.
    #[test]
    fn a_value_that_its_schema_refuses_is_a_server_error() {
        let e = ApiError::from(crate::BuildError::value("`x` is not a valid uuid".to_owned()));
        assert_eq!(e.status, http::StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(e.message, "`x` is not a valid uuid");
    }

    #[derive(Debug)]
    struct Wrapped(std::io::Error);

    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("error sending request")
        }
    }

    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    fn message(e: ClientError) -> String {
        let ClientError::Transport(m) = e else { panic!("not a transport error: {e}") };
        m
    }

    #[test]
    fn a_transport_error_names_each_cause() {
        let e = Wrapped(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset by peer"));
        assert_eq!(message(ClientError::transport(e)), "error sending request: connection reset by peer");
    }

    #[test]
    fn an_unexpected_status_shows_its_body_as_text_in_debug() {
        let e = ClientError::UnexpectedStatus {
            status: http::StatusCode::NOT_FOUND,
            body: bytes::Bytes::from_static(b"{\"a\":1}"),
        };
        assert_eq!(format!("{e:?}"), r#"UnexpectedStatus { status: 404, body: b"{\"a\":1}" }"#);
    }

    #[test]
    fn a_transport_error_takes_a_message() {
        assert_eq!(message(ClientError::transport("no route")), "no route");
        assert_eq!(message(ClientError::transport(String::from("no route"))), "no route");
    }

    #[test]
    fn an_api_error_shows_its_status_and_message() {
        let e = ApiError::new(http::StatusCode::CONFLICT, "taken");
        assert_eq!(e.to_string(), "409: taken");
    }
}
