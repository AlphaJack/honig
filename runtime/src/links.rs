//! Runtime expressions of links and callbacks.

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Message, Request, Response, params::parse_query};

/// One finished HTTP exchange for the evaluation of `OpenAPI` runtime expressions in links and callbacks.
///
/// The expressions are `$url`, `$method`, `$statusCode`, `$request.*` and `$response.*`. Leave out what you do not
/// have. An expression that refers to a missing part evaluates to `None`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Exchange<'a> {
    /// The full request URL.
    pub url: &'a str,
    /// The request method such as `POST`.
    pub method: &'a str,
    /// The response status; `0` when there is no response yet (callbacks).
    pub status: u16,
    /// The request: headers, query string (in `path`) and body.
    pub request: Option<&'a Request>,
    /// The response: headers and body.
    pub response: Option<&'a Response>,
}

impl Exchange<'_> {
    /// Evaluates one runtime expression (without `{}`) as JSON.
    /// `$request.path.*` needs the parameters of the operation. Generated code
    /// gets those from `Params` instead.
    #[must_use]
    pub fn eval(&self, expr: &str) -> Option<Value> {
        fn body(bytes: &[u8], ptr: &str) -> Option<Value> {
            let v: Value = serde_json::from_slice(bytes).ok()?;
            match ptr.strip_prefix('#') {
                Some(p) => v.pointer(p).cloned(),
                None if ptr.is_empty() => Some(v),
                None => None,
            }
        }
        match expr {
            "$url" => return (!self.url.is_empty()).then(|| Value::String(self.url.to_owned())),
            "$method" => return (!self.method.is_empty()).then(|| Value::String(self.method.to_owned())),
            "$statusCode" => return (self.status != 0).then(|| Value::from(self.status)),
            _ => {}
        }
        if let Some(rest) = expr.strip_prefix("$request.") {
            let req = self.request?;
            if let Some(name) = rest.strip_prefix("header.") {
                return req.header(name).map(|v| Value::String(v.into_owned()));
            }
            if let Some(name) = rest.strip_prefix("query.") {
                let query = req.uri().query().unwrap_or_default();
                return parse_query(query)
                    .into_iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| Value::String(v.into_owned()));
            }
            return rest.strip_prefix("body").and_then(|ptr| body(req.body().bytes(), ptr));
        }
        if let Some(rest) = expr.strip_prefix("$response.") {
            let resp = self.response?;
            if let Some(name) = rest.strip_prefix("header.") {
                return resp.header(name).map(|v| Value::String(v.into_owned()));
            }
            return rest.strip_prefix("body").and_then(|ptr| body(resp.body().bytes(), ptr));
        }
        None
    }
}

/// Returns a runtime expression template with its expressions already evaluated.
///
/// `https://{$request.header.host}/x` becomes `literals` around `values`
/// (`literals.len() == values.len() + 1`). `None` when a value is missing.
/// A template that is one expression keeps its JSON value.
#[must_use]
pub fn template(literals: &[&str], values: Vec<Option<Value>>) -> Option<Value> {
    if let ([a, b], [_]) = (literals, values.as_slice())
        && a.is_empty()
        && b.is_empty()
    {
        return values.into_iter().next().flatten();
    }
    let mut out = String::new();
    for (lit, v) in literals.iter().zip(values.into_iter().map(Some).chain(std::iter::once(None))) {
        out.push_str(lit);
        match v {
            Some(Some(v)) => out.push_str(&value_text(v)),
            Some(None) => return None,
            None => {}
        }
    }
    Some(Value::String(out))
}

/// Returns a value as URL text. A string stays as it is. Anything else becomes JSON.
#[must_use]
pub fn value_text(v: Value) -> String {
    match v {
        Value::String(s) => s,
        other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
            other.to_string()
        }
    }
}

/// Returns a link value as the linked parameter's type; a string containing a number or
/// bool (`"0"`) also fills a typed parameter.
pub fn link_value<T: DeserializeOwned>(v: Value) -> Option<T> {
    let text = v.as_str().map(str::to_owned);
    serde_json::from_value(v).ok().or_else(|| serde_json::from_str(&text?).ok())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn request() -> Request {
        let query = vec!["url=https%3A%2F%2Fhook.example%2Fq".to_owned()];
        let mut req = crate::message::new_request("POST", "/orders".to_owned(), query).unwrap();
        req.set_header("X-Trace", "t-1").unwrap();
        req.set_body("application/json", br#"{"callbackUrl":"https://cb.example/a","ids":[7]}"#.as_slice());
        req
    }

    fn response() -> Response {
        let mut resp = crate::message::new_response(201);
        resp.set_header("Location", "/orders/7").unwrap();
        resp.set_body("application/json", br#"{"id":7}"#.as_slice());
        resp
    }

    #[test]
    fn each_expression_reads_its_part_of_the_exchange() {
        let (req, resp) = (request(), response());
        let ex = Exchange {
            url: "https://api.example/orders",
            method: "POST",
            status: 201,
            request: Some(&req),
            response: Some(&resp),
        };
        let cases = [
            ("$url", Some(json!("https://api.example/orders"))),
            ("$method", Some(json!("POST"))),
            ("$statusCode", Some(json!(201))),
            ("$request.header.x-trace", Some(json!("t-1"))),
            ("$request.query.url", Some(json!("https://hook.example/q"))),
            ("$request.query.missing", None),
            ("$request.body#/callbackUrl", Some(json!("https://cb.example/a"))),
            ("$request.body#/ids/0", Some(json!(7))),
            ("$request.body#/missing", None),
            ("$request.body", Some(json!({"callbackUrl": "https://cb.example/a", "ids": [7]}))),
            ("$request.bodyx", None),
            ("$response.header.location", Some(json!("/orders/7"))),
            ("$response.header.content-type", Some(json!("application/json"))),
            ("$response.body#/id", Some(json!(7))),
            ("$request.path.id", None),
            ("$other", None),
        ];
        for (expr, want) in cases {
            assert_eq!(ex.eval(expr), want, "{expr}");
        }
    }

    #[test]
    fn a_missing_part_evaluates_to_none() {
        let ex = Exchange::default();
        for expr in ["$url", "$method", "$statusCode", "$request.header.a", "$response.body#/id"] {
            assert_eq!(ex.eval(expr), None, "{expr}");
        }
    }

    #[test]
    fn a_template_joins_literals_and_values() {
        assert_eq!(template(&["", ""], vec![Some(json!(7))]), Some(json!(7)), "one expression keeps its JSON value");
        assert_eq!(
            template(&["https://", "/orders/", ""], vec![Some(json!("cb.example")), Some(json!(7))]),
            Some(json!("https://cb.example/orders/7"))
        );
        assert_eq!(template(&["a", "b"], vec![None]), None);
        assert_eq!(template(&["", "/x"], vec![None]), None);
        assert_eq!(template(&["plain"], vec![]), Some(json!("plain")));
    }

    #[test]
    fn value_text_is_a_string_as_it_is_and_anything_else_as_json() {
        assert_eq!(value_text(json!("a b")), "a b");
        assert_eq!(value_text(json!(7)), "7");
        assert_eq!(value_text(json!({"a": [true, null]})), r#"{"a":[true,null]}"#);
    }
}
