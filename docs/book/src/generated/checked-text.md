<!-- Generated from runtime/src/formats.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Format | Type | Accepts |
|---|---|---|
| `date-time` | `api::DateTimeText` | `date-time`: RFC 3339 with an offset. |
| `date-time-local` | `api::DateTimeLocalText` | `date-time-local`: a date and a time without an offset. |
| `time-local` | `api::TimeLocalText` | `time-local`: a time without an offset. |
| `date` | `api::DateText` | `date`: RFC 3339 `full-date`. |
| `uuid` | `api::UuidText` | `uuid`: the hyphenated form of RFC 9562. |
| `uri` | `api::UriText` | `uri`: an absolute RFC 3986 URI. |
| `ipv4-cidr` | `api::Ipv4CidrText` | `ipv4-cidr`: an IPv4 address block. |
| `ipv6-cidr` | `api::Ipv6CidrText` | `ipv6-cidr`: an IPv6 address block. |
| `email` | `api::Email` | `email`: an RFC 5321 mail address. An ASCII dot-atom or quoted local part, then `@`, then a host name or an address literal. |
| `idn-email` | `api::IdnEmail` | `idn-email`: an RFC 6531 mail address, an `email` that may hold UTF-8. |
| `hostname` | `api::Hostname` | `hostname`: an RFC 1123 host name with ASCII labels. A punycode label must decode to a valid name. |
| `idn-hostname` | `api::IdnHostname` | `idn-hostname`: an IDNA2008 host name, a `hostname` whose labels may be Unicode. |
| `decimal` | `api::DecimalText` | `decimal` on a string: the digits of a decimal number. |
| `decimal128` | `api::Decimal128Text` | `decimal128` on a string: a decimal number that IEEE 754 decimal128 holds exactly. |
| `http-date` | `api::HttpDateText` | `http-date`: one of the three forms of RFC 9110 section 5.6.7. |
| `duration` | `api::DurationText` | `duration`: an ISO 8601 duration of RFC 3339 Appendix A. |
| `uri-reference` | `api::UriReferenceText` | `uri-reference`: an RFC 3986 URI or relative reference. |
| `iri` | `api::IriText` | `iri`: an absolute RFC 3987 IRI. |
| `iri-reference` | `api::IriReferenceText` | `iri-reference`: an RFC 3987 IRI or relative reference. |
| `json-pointer` | `api::JsonPointerText` | `json-pointer`: an RFC 6901 JSON pointer. |
| `relative-json-pointer` | `api::RelativeJsonPointerText` | `relative-json-pointer`: a relative JSON pointer. |
| `regex` | `api::RegexText` | `regex`: an ECMA-262 regular expression. |
| `uri-template` | `api::UriTemplateText` | `uri-template`: an RFC 6570 URI template. |
