<!-- Generated from runtime/src/validate.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Format | Type | Accepts |
|---|---|---|
| `email` | `api::Email` | An RFC 5321 mail address (`format: email`). It is an ASCII dot-atom or quoted local part, then `@`, then a host name or an address literal. |
| `idn-email` | `api::IdnEmail` | An RFC 6531 mail address (`format: idn-email`): an `Email` that may hold UTF-8. |
| `hostname` | `api::Hostname` | An RFC 1123 host name (`format: hostname`) with ASCII labels. A punycode label must decode to a valid name. |
| `idn-hostname` | `api::IdnHostname` | An IDNA2008 host name (`format: idn-hostname`): a `Hostname` whose labels may be Unicode. |
