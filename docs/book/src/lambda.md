# AWS Lambda

A honig server runs on AWS Lambda through [`lambda_http`](https://docs.rs/lambda_http), behind the API Gateway route `ANY /{proxy+}`. `lambda_http::run` accepts the router in the same way as its adapter does here:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/lambda.rs:lambda}}
```

| Topic | What to know |
|---|---|
| Stage | `lambda_http` puts the stage in front of the path unless `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH` is set. A REST API always has a named stage. |
| Caller's address | There is no socket. API Gateway's `sourceIp` is in `ctx.extension::<lambda_http::request::RequestContext>()`. |
| Binary bodies | A REST API decodes them only for the media types in its `binaryMediaTypes`. |
| Body size | API Gateway buffers the request, and Lambda caps the payload of a synchronous call: see its [quotas](https://docs.aws.amazon.com/general/latest/gr/lambda-service.html). |
