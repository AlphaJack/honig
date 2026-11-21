# Observability

Every request that reaches the server produces one wide event. Under `into_axum` a path that the description does not declare gets one only through [the fallback in Adapters](adapters.md#axum). The event holds these fields:

- the operation and route
- the status and duration
- the body sizes
- the security schemes and scopes
- the failure kind if there is one

Layers add fields with `req.event().set(key, value)`. Handlers add fields with `ctx.event.set(key, value)`. A key is an `EventKey`. Declare it as a `const` such as `const TENANT: EventKey = EventKey::new("tenant")`. Then a key that has the name of a built-in field fails to compile.

Send the event anywhere with `on_event`:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/observability.rs:events}}
```

## Tracing

With the `tracing` adapter each request runs in a span that wraps your layers and handler. The span follows the HTTP server conventions of OpenTelemetry (`otel.name`, `http.route` and `http.response.status_code`). The default `on_event` logs the wide event inside the span as one structured line. Export the spans with `tracing-opentelemetry`.

## Trace context

The `tracing-opentelemetry` adapter adds W3C Trace Context. The span of a request then continues the trace that its `traceparent` header identifies. The requests of the client carry the `traceparent` and `tracestate` of the current span. A request that already has a `traceparent` keeps it. Spans join only once a `tracing-opentelemetry` layer exports them:

```rust
{{#include ../../../tests/e2e/tests/v32_catalog/observability.rs:trace-context}}
```

The runtime is built against `opentelemetry` 0.33 and `tracing-opentelemetry` 0.34. Depend on the same versions. A span from another version holds no context that the runtime can read.
