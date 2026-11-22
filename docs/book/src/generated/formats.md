<!-- Generated from src/emit/rust/syntax.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Format | Without its library | With `uuid`, `url`, `ipnet` and `jiff` |
|---|---|---|
| `uuid` | `api::UuidText` | `uuid::Uuid` |
| `date-time` | `api::DateTimeText` | `jiff::Timestamp` |
| `date` | `api::DateText` | `jiff::civil::Date` |
| `ipv4` | `std::net::Ipv4Addr` | `std::net::Ipv4Addr` |
| `ipv6` | `std::net::Ipv6Addr` | `std::net::Ipv6Addr` |
| `time` | `api::FullTime` | `api::FullTime` |
| `base64url` | `api::Base64Url` | `api::Base64Url` |
| `decimal` | `api::DecimalText` | `api::DecimalText` |
| `char` | `char` | `char` |
| `date-time-local` | `api::DateTimeLocalText` | `jiff::civil::DateTime` |
| `time-local` | `api::TimeLocalText` | `jiff::civil::Time` |
| `uri` | `api::UriText` | `url::Url` |
| `ipv4-cidr` | `api::Ipv4CidrText` | `ipnet::Ipv4Net` |
| `ipv6-cidr` | `api::Ipv6CidrText` | `ipnet::Ipv6Net` |
| `email` | `api::EmailText` | `api::EmailText` |
| `idn-email` | `api::IdnEmailText` | `api::IdnEmailText` |
| `hostname` | `api::HostnameText` | `api::HostnameText` |
| `idn-hostname` | `api::IdnHostnameText` | `api::IdnHostnameText` |
| `http-date` | `api::HttpDateText` | `api::HttpDateText` |
| `duration` | `api::DurationText` | `api::DurationText` |
| `uri-reference` | `api::UriReferenceText` | `api::UriReferenceText` |
| `iri` | `api::IriText` | `api::IriText` |
| `iri-reference` | `api::IriReferenceText` | `api::IriReferenceText` |
| `json-pointer` | `api::JsonPointerText` | `api::JsonPointerText` |
| `relative-json-pointer` | `api::RelativeJsonPointerText` | `api::RelativeJsonPointerText` |
| `regex` | `api::RegexText` | `api::RegexText` |
| `uri-template` | `api::UriTemplateText` | `api::UriTemplateText` |
| `decimal128` | `api::Decimal128Text` | `api::Decimal128Text` |
