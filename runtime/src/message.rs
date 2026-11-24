//! Requests and responses: the types of the `http` crate with a [`Body`] of the runtime.

use std::borrow::Cow;

use serde::Serialize;
use serde_json::Value;

use crate::header::{header_text, header_value};
use crate::scalar::scalar;
use crate::{Body, ClientError, json::to_value};

/// An HTTP request. Its target is the path and query, or a whole URL for a webhook or a token endpoint.
pub type Request = http::Request<Body>;

/// An HTTP response.
pub type Response = http::Response<Body>;

/// What a request and a response share: headers decoded as text and a body set with its `Content-Type`.
pub trait Message {
    /// Returns the first line of header `name` as text. Bytes that are not UTF-8 are decoded as ISO-8859-1.
    fn header(&self, name: &str) -> Option<Cow<'_, str>>;

    /// Returns the `Content-Type` header, or an empty string.
    fn content_type(&self) -> &str;

    /// Appends a header. Literal values are kept without a copy.
    ///
    /// # Errors
    /// The name is not a header name or the value contains a line break or another control character.
    fn set_header(&mut self, name: &str, value: impl Into<Cow<'static, str>>) -> Result<(), ClientError>;

    /// Sets the body and its `Content-Type`.
    fn set_body(&mut self, content_type: impl Into<Cow<'static, str>>, body: impl Into<Body>);
}

/// The head and body of a message as [`Message`] gets and sets them.
trait Parts {
    fn headers(&self) -> &http::HeaderMap;
    fn headers_mut(&mut self) -> &mut http::HeaderMap;
    fn body_mut(&mut self) -> &mut Body;
}

impl Parts for Request {
    fn headers(&self) -> &http::HeaderMap {
        Self::headers(self)
    }
    fn headers_mut(&mut self) -> &mut http::HeaderMap {
        Self::headers_mut(self)
    }
    fn body_mut(&mut self) -> &mut Body {
        Self::body_mut(self)
    }
}

impl Parts for Response {
    fn headers(&self) -> &http::HeaderMap {
        Self::headers(self)
    }
    fn headers_mut(&mut self) -> &mut http::HeaderMap {
        Self::headers_mut(self)
    }
    fn body_mut(&mut self) -> &mut Body {
        Self::body_mut(self)
    }
}

impl<M: Parts> Message for M {
    fn header(&self, name: &str) -> Option<Cow<'_, str>> {
        first_header(Parts::headers(self), name)
    }

    fn content_type(&self) -> &str {
        content_type_of(Parts::headers(self))
    }

    fn set_header(&mut self, name: &str, value: impl Into<Cow<'static, str>>) -> Result<(), ClientError> {
        let key = http::HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| ClientError::Encode(format!("header name `{name}`: {e}")))?;
        let value = header_value(value.into())
            .ok_or_else(|| ClientError::Encode(format!("header `{name}`: not a valid value")))?;
        Parts::headers_mut(self).append(key, value);
        Ok(())
    }

    fn set_body(&mut self, content_type: impl Into<Cow<'static, str>>, body: impl Into<Body>) {
        if let Some(v) = header_value(content_type.into()) {
            Parts::headers_mut(self).insert(http::header::CONTENT_TYPE, v);
        }
        *self.body_mut() = body.into();
    }
}

/// Creates a request from a method, an encoded path and encoded `name=value` query pairs.
///
/// # Errors
/// The path and query do not make a request target.
pub fn new_request(method: &'static str, path: String, query: Vec<String>) -> Result<Request, ClientError> {
    let mut path = path;
    for (i, q) in query.into_iter().enumerate() {
        path.push(if i == 0 { '?' } else { '&' });
        path.push_str(&q);
    }
    let bad = |e: &dyn std::fmt::Display| ClientError::Encode(format!("request target `{path}`: {e}"));
    let uri = http::Uri::try_from(path.as_str()).map_err(|e| bad(&e))?;
    let method = http::Method::from_bytes(method.as_bytes()).map_err(|e| bad(&e))?;
    let mut req = Request::new(Body::default());
    *req.method_mut() = method;
    *req.uri_mut() = uri;
    Ok(req)
}

/// Returns a copy of `req` without its extensions. Clones of a body share what is still to come.
pub(crate) fn copy_request(req: &Request) -> Request {
    let mut copy = Request::new(req.body().clone());
    *copy.method_mut() = req.method().clone();
    *copy.uri_mut() = req.uri().clone();
    *copy.version_mut() = req.version();
    copy.headers_mut().clone_from(req.headers());
    copy
}

/// Creates an empty response with this status. A status out of range is `500`.
pub fn new_response(status: u16) -> Response {
    let mut resp = Response::new(Body::default());
    *resp.status_mut() = http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    resp
}

/// Returns a response with this body and its `Content-Type` and no other header.
pub fn body_response(status: u16, content_type: impl Into<Cow<'static, str>>, body: impl Into<Body>) -> Response {
    let mut resp = new_response(status);
    resp.set_body(content_type, body);
    resp
}

/// Returns whether the body of `resp` is one of the `declared` media types. No `Content-Type` fits only an empty body.
///
/// # Errors
/// [`ClientError::UnexpectedMedia`] with what was sent and what is declared.
pub fn check_body_media(resp: &Response, declared: &[&str]) -> Result<(), ClientError> {
    let body = Some(resp.body().bytes());
    crate::media::check_response_media(resp.status().as_u16(), resp.content_type(), body, declared)
}

/// Returns whether a response received whole, as its `head` and `body`, is one of the `declared` media types.
///
/// # Errors
/// [`ClientError::UnexpectedMedia`] with what was sent and what is declared.
pub fn check_media_of(head: &http::response::Parts, body: &[u8], declared: &[&str]) -> Result<(), ClientError> {
    crate::media::check_response_media(head.status.as_u16(), content_type_of(&head.headers), Some(body), declared)
}

/// Returns the `Content-Type` in `headers`, or an empty string.
pub fn content_type_of(headers: &http::HeaderMap) -> &str {
    headers.get(http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or_default()
}

/// Returns the first line of header `name` as text. Bytes that are not UTF-8 are decoded as ISO-8859-1.
pub fn first_header<'h>(headers: &'h http::HeaderMap, name: &str) -> Option<Cow<'h, str>> {
    lines(headers, name).next()
}

/// Returns every line of header `name` in order as text.
fn lines<'h>(headers: &'h http::HeaderMap, name: &str) -> impl Iterator<Item = Cow<'h, str>> {
    headers.get_all(name).into_iter().map(|v| header_text(v.as_bytes()))
}

/// Returns every line of a header joined with `,`.
///
/// RFC 9110 section 5.3 makes
/// `X: a` + `X: b` the same as `X: a,b` (3.2 Header Object "Representing the
/// `Set-Cookie` Header"). So a list-valued header decodes the same either way.
/// Borrowed when there is one line. Not for `Set-Cookie` ([`header_lines`]).
pub fn joined_header<'h>(headers: &'h http::HeaderMap, name: &str) -> Option<Cow<'h, str>> {
    let mut lines = lines(headers, name);
    let first = lines.next()?;
    match lines.next() {
        None => Some(first),
        Some(second) => {
            let mut out = format!("{first},{second}");
            for l in lines {
                out.push(',');
                out.push_str(&l);
            }
            Some(Cow::Owned(out))
        }
    }
}

/// Returns every line of a header as its own value.
///
/// `Set-Cookie` "can only use the
/// one-value-per-line form" (RFC 9110 section 5.3 and 3.2 Header Object). The
/// `Expires` of a cookie contains a comma. `None` when the header is absent. One
/// string for a single-valued schema. Otherwise an array.
pub fn header_lines(headers: &http::HeaderMap, name: &str, array: bool) -> Option<Value> {
    let lines: Vec<Value> = lines(headers, name).map(|v| Value::String(v.into_owned())).collect();
    match (lines.is_empty(), array) {
        (true, _) => None,
        (false, true) => Some(Value::Array(lines)),
        (false, false) => lines.into_iter().next(),
    }
}

/// Returns a `Set-Cookie` value as header lines: one per array item.
/// Errors: the value does not serialize.
pub fn encode_header_lines<T: Serialize + ?Sized>(v: &T) -> Result<Vec<String>, ClientError> {
    Ok(match to_value(v)? {
        Value::Null => vec![],
        Value::Array(a) => a.iter().filter(|x| !x.is_null()).map(scalar).collect(),
        x @ (Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Object(_)) => vec![scalar(&x)],
    })
}

/// Returns a base URL without its trailing slashes.
///
/// A transport puts it before the path of each request. `relative`: a path such as `/api` is one too: a browser
/// resolves it against the origin of the page, and a service in the same process needs no host.
///
/// # Errors
/// It has no scheme or no host and is no path that `relative` allows, or it has a query or a fragment.
pub fn base_url(url: impl Into<String>, relative: bool) -> Result<String, ClientError> {
    let url = url.into();
    let uri = http::Uri::try_from(url.as_str()).map_err(ClientError::transport)?;
    let absolute = uri.scheme().is_some() && uri.authority().is_some();
    if !(absolute || relative && url.starts_with('/')) || uri.query().is_some() || url.contains('#') {
        return Err(ClientError::Transport(format!(
            "`{url}` is no base URL: write a scheme and a host, such as `https://api.example.com/v1`, and no query"
        )));
    }
    Ok(url.trim_end_matches('/').to_owned())
}

/// Returns the path and query of `uri`: what the router matches.
pub fn path_and_query(uri: &http::Uri) -> &str {
    uri.path_and_query().map_or("/", http::uri::PathAndQuery::as_str)
}

/// Returns where a request goes. A target that is a whole URL stays as it is. Any other goes after `base_url`.
pub fn target(base_url: &str, uri: &http::Uri) -> String {
    if uri.scheme().is_some() {
        uri.to_string()
    } else {
        format!("{base_url}{}", uri.path_and_query().map_or("/", http::uri::PathAndQuery::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::{body_response, check_body_media};
    use crate::ClientError;

    #[test]
    fn a_body_of_an_undeclared_media_type_is_an_error() {
        let resp = body_response(200, "text/html", "<p>hi</p>");
        assert!(check_body_media(&resp, &["text/html", "application/json"]).is_ok());
        let err = check_body_media(&resp, &["application/json"]).unwrap_err();
        assert!(
            matches!(&err, ClientError::UnexpectedMedia { status: 200, content_type, declared, .. }
                if content_type == "text/html" && declared == "application/json"),
            "{err:?}"
        );
    }

    #[test]
    fn a_base_url_needs_a_scheme_and_a_host() {
        assert!(super::base_url("localhost:3000", false).is_err());
        assert!(super::base_url("/v1", false).is_err());
        assert_eq!(super::base_url("/v1/", true).ok().as_deref(), Some("/v1"), "a browser resolves a path");
        assert!(super::base_url("https://api.example.com/v1?a=1", true).is_err());
        assert!(super::base_url("https://api.example.com/v1#a", true).is_err());
        let base = super::base_url("https://api.example.com/v1/", false);
        assert_eq!(base.ok().as_deref(), Some("https://api.example.com/v1"));
    }
}
