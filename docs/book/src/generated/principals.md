<!-- Generated from runtime/src/server/service.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Whether the server finds who is calling: `Anonymous` until `Server::authenticate`, then `Authenticated`.

An `Api` whose `Principal` is `()` builds into a service as it is. Any other `Principal` needs
`Server::authenticate`: without it `into_service` does not compile. Each operation then takes what its
`security` asks for:

| The operation's `security` | `ctx.principal` | When `authenticate` found nobody |
|---|---|---|
| none (`security: []`) | `()` | the handler runs |
| an empty alternative (`[{}, {apiKey: []}]`) | `Option<Principal>` | the handler runs with `None` |
| credentials required | `Principal` | 401 in the error schema |
