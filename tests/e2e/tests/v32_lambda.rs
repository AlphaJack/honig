//! Hand-written checks for the `lambda` fixture: the axum router behind one API Gateway `{proxy+}` route. Each
//! event goes through the own adapter of `lambda_http`, the one that `lambda_http::run` serves with. So the request
//! and the response are converted exactly as on AWS.

#![cfg(feature = "axum")]

use e2e::v32_lambda::api::{self, operations as ops, types};
use lambda_http::request::{LambdaRequest, RequestContext};
use serde_json::{Value, json};
use tower::Service;

struct Server;

impl api::Api for Server {
    type Principal = ();

    async fn get_item(
        &self,
        params: ops::get_item::Params,
        ctx: &api::Context<'_, ()>,
    ) -> Result<ops::get_item::Response, api::ApiError> {
        // API Gateway knows the principal and not a socket. `lambda_http` puts its request context in the extensions.
        let source_ip = match ctx.extension::<RequestContext>() {
            Some(RequestContext::ApiGatewayV2(c)) => c.http.source_ip.clone(),
            Some(RequestContext::ApiGatewayV1(c)) => c.identity.source_ip.clone(),
            _ => None,
        };
        Ok(ops::get_item::Response::Ok(types::GetItemOk {
            id: params.id,
            tags: params.tag.unwrap_or_default(),
            trace: params.x_trace,
            session: params.session,
            source_ip,
        }))
    }

    async fn put_blob(
        &self,
        params: ops::put_blob::Params,
        _: &api::Context<'_, ()>,
    ) -> Result<ops::put_blob::Response, api::ApiError> {
        let body = params.body.collect().await?;
        Ok(ops::put_blob::Response::Ok(api::Blob::from(body)))
    }
}

/// The response that API Gateway gets for `event` as JSON.
fn invoke(event: Value) -> Value {
    // ANCHOR: lambda
    let mut adapter = lambda_http::Adapter::from(api::Server::new(Server).into_axum());
    // ANCHOR_END: lambda
    let payload: LambdaRequest = serde_json::from_value(event).unwrap();
    let event = lambda_http::LambdaEvent::new(payload, lambda_http::Context::default());
    serde_json::to_value(api::block_on(adapter.call(event)).unwrap()).unwrap()
}

/// An HTTP API (payload 2.0) event on the `$default` stage. The `httpApi` of Serverless Framework deploys that one.
fn v2(method: &str, path: &str, query: &str, body: Option<&str>) -> Value {
    json!({
        "version": "2.0",
        "routeKey": "ANY /{proxy+}",
        "rawPath": path,
        "rawQueryString": query,
        "cookies": ["session=s1"],
        "headers": {"x-trace": "t1", "content-type": "application/octet-stream"},
        "requestContext": {
            "accountId": "123456789012",
            "apiId": "api",
            "domainName": "api.example.com",
            "http": {"method": method, "path": path, "protocol": "HTTP/1.1", "sourceIp": "192.0.2.7", "userAgent": "t"},
            "requestId": "r",
            "routeKey": "ANY /{proxy+}",
            "stage": "$default",
            "time": "11/Nov/2026:10:00:00 +0000",
            "timeEpoch": 0
        },
        "body": body,
        "isBase64Encoded": body.is_some()
    })
}

/// A REST API (payload 1.0) event on the `prod` stage as the `http` events of Serverless Framework deploy.
fn v1(method: &str, path: &str, tags: &[&str], body: Option<&str>) -> Value {
    json!({
        "resource": "/{proxy+}",
        "path": path,
        "httpMethod": method,
        "headers": {"X-Trace": "t1", "Cookie": "session=s1", "Content-Type": "application/octet-stream"},
        "multiValueHeaders": {
            "X-Trace": ["t1"],
            "Cookie": ["session=s1"],
            "Content-Type": ["application/octet-stream"]
        },
        "queryStringParameters": tags.last().map(|t| json!({"tag": t})),
        "multiValueQueryStringParameters": if tags.is_empty() { Value::Null } else { json!({"tag": tags}) },
        "pathParameters": {"proxy": path.trim_start_matches('/')},
        "requestContext": {
            "accountId": "123456789012",
            "apiId": "api",
            "domainName": "api.example.com",
            "httpMethod": method,
            "identity": {"sourceIp": "192.0.2.7"},
            "path": format!("/prod{path}"),
            "requestId": "r",
            "resourcePath": "/{proxy+}",
            "stage": "prod"
        },
        "body": body,
        "isBase64Encoded": body.is_some()
    })
}

fn item(tags: &[&str]) -> Value {
    json!({"id": "7", "tags": tags, "trace": "t1", "session": "s1", "sourceIp": "192.0.2.7"})
}

/// The status and the decoded JSON body of a response.
fn json_of(resp: &Value) -> (u64, Value) {
    (resp["statusCode"].as_u64().unwrap(), serde_json::from_str(resp["body"].as_str().unwrap()).unwrap())
}

#[test]
fn http_api_parameters_reach_the_operation() {
    assert_eq!(json_of(&invoke(v2("GET", "/items/7", "tag=a&tag=b", None))), (200, item(&["a", "b"])));
}

/// Payload 2.0 joins repeated query values with commas in `queryStringParameters`. The router reads
/// `rawQueryString`. So a comma inside a value stays inside it.
#[test]
fn http_api_repeated_query_values_keep_their_commas() {
    assert_eq!(json_of(&invoke(v2("GET", "/items/7", "tag=a%2Cb&tag=c", None))), (200, item(&["a,b", "c"])));
}

#[test]
fn http_api_binary_bodies_roundtrip_base64() {
    let resp = invoke(v2("PUT", "/items/7/blob", "", Some("AAEC/w==")));
    assert_eq!(
        (&resp["statusCode"], &resp["body"], &resp["isBase64Encoded"]),
        (&json!(200), &json!("AAEC/w=="), &json!(true))
    );
}

/// `lambda_http` puts the stage of a REST API in front of the path unless
/// `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH` is set. No route declares the stage. One test, so that no other reads the
/// variable while it changes.
#[test]
fn rest_api_routes_once_the_stage_is_ignored() {
    let get = || invoke(v1("GET", "/items/7", &["a,b", "c"], None));
    assert_eq!(get()["statusCode"], json!(404));
    // SAFETY: no other test in this binary reads the environment.
    unsafe { std::env::set_var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH", "true") };
    assert_eq!(json_of(&get()), (200, item(&["a,b", "c"])));
    let resp = invoke(v1("PUT", "/items/7/blob", &[], Some("AAEC/w==")));
    assert_eq!(
        (&resp["statusCode"], &resp["body"], &resp["isBase64Encoded"]),
        (&json!(200), &json!("AAEC/w=="), &json!(true))
    );
}
