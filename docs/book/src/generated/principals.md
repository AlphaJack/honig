<!-- Generated from runtime/src/server/auth.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

Who is calling and the scopes that their credential grants: what a `verify_` method of the `Api` found.

The `Api` has one `verify_` method per security scheme that its operations use, and does not compile without it.
The server checks that a request presents the credentials that its operation declares. It calls the method of each
scheme of an alternative, at most once per request, and checks the scopes that the credentials grant. Each
operation then takes what its `security` asks for:

| The operation's `security` | `ctx.principal` | When no alternative passes |
|---|---|---|
| none (`security: []`) | `()` | the handler runs |
| an empty alternative (`[{}, {apiKey: []}]`) | `Option<Principal>` | `None`; 401 when a method refused a credential |
| credentials required | `Principal` | 401; 403 when only a scope is missing |

The principal is the one of the first scheme of the alternative that passes. Layers read it from the request's
extensions.
