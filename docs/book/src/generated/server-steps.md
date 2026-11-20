<!-- Generated from runtime/src/server/service.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

A request goes through these steps. When a step refuses the request it answers in the error schema of the
description:

1. **Routing**: an undeclared path is 404. An undeclared method is 405. A `HEAD` takes the `GET` of its path and
   gets its headers without the body.
2. **Credentials**: the `security` of the operation. If none of its requirements are present then the answer is
   401.
3. **Your layers** as an `http::Request<Body>` in the order that you add them. The first one is the outermost.
   Its extensions hold the generated `Operation`, the `RequestEvent` and the credentials (`Secured`). The body
   is not read yet. So a refused request costs no body read. `read_body` reads it for a layer that needs it.
4. **The body**: read whole up to `max_body_bytes` (413). An operation that streams its body skips this step.
5. **Decoding and validation** (400 and 415). Then your handler runs and its response is encoded.

`Server::authenticate` finds who is calling and puts the principal in the extensions. When your `Api` declares a
`Principal` the server builds into a service only once it does.
