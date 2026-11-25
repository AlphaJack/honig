<!-- Generated from honig/src/formats.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Format | Type | Accepts |
|---|---|---|
| `date-time` | `api::DateTimeStr` | `date-time`: RFC 3339 with an offset. |
| `date-time-local` | `api::DateTimeLocalStr` | `date-time-local`: a date and a time without an offset. |
| `time-local` | `api::TimeLocalStr` | `time-local`: a time without an offset. |
| `date` | `api::DateStr` | `date`: RFC 3339 `full-date`. |
| `uuid` | `api::UuidStr` | `uuid`: the hyphenated form of RFC 9562. |
| `uri` | `api::UriStr` | `uri`: an absolute RFC 3986 URI. |
| `ipv4-cidr` | `api::Ipv4CidrStr` | `ipv4-cidr`: an IPv4 address block. |
| `ipv6-cidr` | `api::Ipv6CidrStr` | `ipv6-cidr`: an IPv6 address block. |
| `email` | `api::EmailStr` | `email`: an RFC 5321 mail address. An ASCII dot-atom or quoted local part, then `@`, then a host name or an address literal. |
| `idn-email` | `api::IdnEmailStr` | `idn-email`: an RFC 6531 mail address, an `email` that may contain UTF-8. |
| `hostname` | `api::HostnameStr` | `hostname`: an RFC 1123 host name with ASCII labels. A punycode label must decode to a valid name. |
| `idn-hostname` | `api::IdnHostnameStr` | `idn-hostname`: an IDNA2008 host name, a `hostname` whose labels may be Unicode. |
| `decimal` | `api::DecimalStr` | `decimal` on a string: the digits of a decimal number. |
| `decimal128` | `api::Decimal128Str` | `decimal128` on a string: a decimal number that IEEE 754 decimal128 represents exactly. |
| `http-date` | `api::HttpDateStr` | `http-date`: one of the three forms of RFC 9110 section 5.6.7. |
| `duration` | `api::DurationStr` | `duration`: an ISO 8601 duration of RFC 3339 Appendix A. |
| `uri-reference` | `api::UriReferenceStr` | `uri-reference`: an RFC 3986 URI or relative reference. |
| `iri` | `api::IriStr` | `iri`: an absolute RFC 3987 IRI. |
| `iri-reference` | `api::IriReferenceStr` | `iri-reference`: an RFC 3987 IRI or relative reference. |
| `json-pointer` | `api::JsonPointerStr` | `json-pointer`: an RFC 6901 JSON pointer. |
| `relative-json-pointer` | `api::RelativeJsonPointerStr` | `relative-json-pointer`: a relative JSON pointer. |
| `regex` | `api::RegexStr` | `regex`: an ECMA-262 regular expression. |
| `uri-template` | `api::UriTemplateStr` | `uri-template`: an RFC 6570 URI template. |
