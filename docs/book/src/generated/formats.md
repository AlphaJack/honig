<!-- Generated from src/emit/rust/syntax.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Format | Without its library | With `uuid`, `url`, `ipnet` and `chrono` | With `jiff` in place of `chrono` |
|---|---|---|---|
| `uuid` | `api::UuidText` | `uuid::Uuid` | `uuid::Uuid` |
| `date-time` | `api::DateTimeText` | `chrono::DateTime<chrono::Utc>` | `jiff::Timestamp` |
| `date` | `api::DateText` | `chrono::NaiveDate` | `jiff::civil::Date` |
| `ipv4` | `std::net::Ipv4Addr` | `std::net::Ipv4Addr` | `std::net::Ipv4Addr` |
| `ipv6` | `std::net::Ipv6Addr` | `std::net::Ipv6Addr` | `std::net::Ipv6Addr` |
| `time` | `api::TimeText` | `api::TimeText` | `api::FullTime` |
| `base64url` | `api::Base64Url` | `api::Base64Url` | `api::Base64Url` |
| `decimal` | `api::DecimalText` | `api::DecimalText` | `api::DecimalText` |
| `char` | `char` | `char` | `char` |
| `date-time-local` | `api::DateTimeLocalText` | `chrono::NaiveDateTime` | `jiff::civil::DateTime` |
| `time-local` | `api::TimeLocalText` | `chrono::NaiveTime` | `jiff::civil::Time` |
| `uri` | `api::UriText` | `url::Url` | `url::Url` |
| `ipv4-cidr` | `api::Ipv4CidrText` | `ipnet::Ipv4Net` | `ipnet::Ipv4Net` |
| `ipv6-cidr` | `api::Ipv6CidrText` | `ipnet::Ipv6Net` | `ipnet::Ipv6Net` |
| `email` | `api::Email` | `api::Email` | `api::Email` |
| `idn-email` | `api::IdnEmail` | `api::IdnEmail` | `api::IdnEmail` |
| `hostname` | `api::Hostname` | `api::Hostname` | `api::Hostname` |
| `idn-hostname` | `api::IdnHostname` | `api::IdnHostname` | `api::IdnHostname` |
| `http-date` | `api::HttpDateText` | `api::HttpDateText` | `api::HttpDateText` |
| `duration` | `api::DurationText` | `api::DurationText` | `api::DurationText` |
| `uri-reference` | `api::UriReferenceText` | `api::UriReferenceText` | `api::UriReferenceText` |
| `iri` | `api::IriText` | `api::IriText` | `api::IriText` |
| `iri-reference` | `api::IriReferenceText` | `api::IriReferenceText` | `api::IriReferenceText` |
| `json-pointer` | `api::JsonPointerText` | `api::JsonPointerText` | `api::JsonPointerText` |
| `relative-json-pointer` | `api::RelativeJsonPointerText` | `api::RelativeJsonPointerText` | `api::RelativeJsonPointerText` |
| `regex` | `api::RegexText` | `api::RegexText` | `api::RegexText` |
| `uri-template` | `api::UriTemplateText` | `api::UriTemplateText` | `api::UriTemplateText` |
