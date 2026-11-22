<!-- Generated from src/emit/rust/syntax.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Format | Without its library | With `uuid`, `url`, `ipnet` and `jiff` |
|---|---|---|
| `uuid` | `api::UuidStr` | `uuid::Uuid` |
| `date-time` | `api::DateTimeStr` | `jiff::Timestamp` |
| `date` | `api::DateStr` | `jiff::civil::Date` |
| `ipv4` | `std::net::Ipv4Addr` | `std::net::Ipv4Addr` |
| `ipv6` | `std::net::Ipv6Addr` | `std::net::Ipv6Addr` |
| `time` | `api::FullTime` | `api::FullTime` |
| `base64url` | `api::Base64Url` | `api::Base64Url` |
| `decimal` | `api::DecimalStr` | `api::DecimalStr` |
| `char` | `char` | `char` |
| `date-time-local` | `api::DateTimeLocalStr` | `jiff::civil::DateTime` |
| `time-local` | `api::TimeLocalStr` | `jiff::civil::Time` |
| `uri` | `api::UriStr` | `url::Url` |
| `ipv4-cidr` | `api::Ipv4CidrStr` | `ipnet::Ipv4Net` |
| `ipv6-cidr` | `api::Ipv6CidrStr` | `ipnet::Ipv6Net` |
| `email` | `api::EmailStr` | `api::EmailStr` |
| `idn-email` | `api::IdnEmailStr` | `api::IdnEmailStr` |
| `hostname` | `api::HostnameStr` | `api::HostnameStr` |
| `idn-hostname` | `api::IdnHostnameStr` | `api::IdnHostnameStr` |
| `http-date` | `api::HttpDateStr` | `api::HttpDateStr` |
| `duration` | `api::DurationStr` | `api::DurationStr` |
| `uri-reference` | `api::UriReferenceStr` | `api::UriReferenceStr` |
| `iri` | `api::IriStr` | `api::IriStr` |
| `iri-reference` | `api::IriReferenceStr` | `api::IriReferenceStr` |
| `json-pointer` | `api::JsonPointerStr` | `api::JsonPointerStr` |
| `relative-json-pointer` | `api::RelativeJsonPointerStr` | `api::RelativeJsonPointerStr` |
| `regex` | `api::RegexStr` | `api::RegexStr` |
| `uri-template` | `api::UriTemplateStr` | `api::UriTemplateStr` |
| `decimal128` | `api::Decimal128Str` | `api::Decimal128Str` |
