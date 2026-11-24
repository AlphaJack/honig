<!-- Generated from honig/src/server/service.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Adds a tower layer between routing and the operations. It goes inside the layers added before. The first one
added is the outermost as with the `ServiceBuilder` of tower. Pass one `ServiceBuilder` to see the order top
to bottom.

A layer here and one on the axum `Router` that `into_axum` returns differ:

- An `operation_layer` runs once routing found the operation and its credentials passed. Its extensions contain
  the operation, the credentials and the `Passed` alternative. Its errors go out in the error
  schema of the description. It fits per-caller limits, permissions, signatures and timeouts.
- An axum `Router::layer` runs on every request, also to unknown paths. It knows no operation yet, and its
  errors go out with the body that it writes. It fits CORS, compression and request IDs.
