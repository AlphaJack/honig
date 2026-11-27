//! The Lambda page of the book on the catalog: its server behind one API Gateway `{proxy+}` route, through the own
//! adapter of `lambda_http`. So the request and the response are converted exactly as on AWS.

use lambda_http::request::LambdaRequest;
use serde_json::{Value, json};
use tower::Service;

use super::{Catalog, api};

/// An HTTP API (payload 2.0) event on the `$default` stage: `GET path` with the key of the catalog.
fn event(path: &str) -> Value {
    json!({
        "version": "2.0",
        "routeKey": "ANY /{proxy+}",
        "rawPath": path,
        "rawQueryString": "",
        "headers": {"x-api-key": "secret"},
        "requestContext": {
            "accountId": "123456789012",
            "apiId": "api",
            "domainName": "shop.example.com",
            "http": {"method": "GET", "path": path, "protocol": "HTTP/1.1", "sourceIp": "192.0.2.7", "userAgent": "t"},
            "requestId": "r",
            "routeKey": "ANY /{proxy+}",
            "stage": "$default",
            "time": "11/Nov/2026:10:00:00 +0000",
            "timeEpoch": 0
        },
        "isBase64Encoded": false
    })
}

#[test]
fn the_catalog_answers_through_the_lambda_adapter() {
    // ANCHOR: lambda
    let mut adapter = lambda_http::Adapter::from(api::Server::new(Catalog::default()).into_axum());
    // ANCHOR_END: lambda
    let payload: LambdaRequest = serde_json::from_value(event("/v1/products")).unwrap();
    let call = lambda_http::LambdaEvent::new(payload, lambda_http::Context::default());
    let resp = serde_json::to_value(honig::block_on(adapter.call(call)).unwrap()).unwrap();
    assert_eq!((&resp["statusCode"], &resp["body"]), (&json!(200), &json!("[]")));
}
