<!-- Generated from honig/src/server/service.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

A request goes through these steps. When a step refuses the request it answers in the error schema of the
description:

1. **Routing**: an undeclared path is 404. An undeclared method is 405. A `HEAD` uses the `GET` of its path and
   gets its headers without the body.
2. **Credentials**: the `security` of the operation. If none of its requirements are present then the answer is
   401. Otherwise the `verify_` method of the `Api` for each scheme checks its credential, and the server checks
   the scopes that it grants (403). The alternative that passed goes in the extensions (`Passed`).
3. **Your layers** as an `http::Request<Body>` in the order that you add them. The first one is the outermost.
   Its extensions contain the generated `Operation`, the `RequestEvent`, the credentials (`Secured`) and the
   `Passed` alternative. The body is not consumed yet. So a refused request never consumes its body. `read_body` buffers it
   for a layer that needs it.
4. **The body**: buffered whole up to `max_body_bytes` (413). An operation that streams its body skips this step.
5. **Decoding and validation** (400 and 415). Then your handler runs and its response is encoded.
