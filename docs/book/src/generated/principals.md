<!-- Generated from runtime/src/server/service.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Whether the server finds who is calling: `Anonymous` until `Server::authenticate`, then `Authenticated`.

An `Api` whose `Principal` is `()` and whose description declares no `security` builds into a service as it is.
Any other needs `Server::authenticate`: without it `into_service` does not compile. The server checks that a
request presents the credentials that its operation declares; `authenticate` checks that they are valid. Each
operation then takes what its `security` asks for:

| The operation's `security` | `ctx.principal` | When `authenticate` found nobody |
|---|---|---|
| none (`security: []`) | `()` | the handler runs |
| an empty alternative (`[{}, {apiKey: []}]`) | `Option<Principal>` | the handler runs with `None` |
| credentials required | `Principal` | 401 in the error schema |
