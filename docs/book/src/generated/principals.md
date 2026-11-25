<!-- Generated from honig/src/server/auth.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

What a `verify_` method of the `Api` found for a credential and the scopes that the credential grants.

The `Api` has one `verify_` method and one associated type per security scheme that its operations use, and does
not compile without them. The server checks that a request presents the credentials that its operation declares.
It calls the method of each scheme of an alternative, at most once per request, and checks the scopes that the
credentials grant. Each operation then gets what its `security` asks for in `ctx.security`:

| The operation's `security` | `ctx.security` | When no alternative passes |
|---|---|---|
| none (`security: []`) | `()` | the handler runs |
| an empty alternative (`[{}, {apiKey: []}]`) | its `Security`, `Anonymous` for none | `Anonymous`; 401 when a method refused a credential |
| credentials required | its `Security` | 401; 403 when only a scope is missing |

The `Security` of an operation has one variant per alternative with the value of each of its schemes: an
alternative of two schemes gives both values, which the handler can compare.
