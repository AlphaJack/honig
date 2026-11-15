<!-- Generated from src/emit/rust/syntax.rs by src/tests/docs_pages.rs. Edit the source, not this file. -->

| Format | Without its library | With `uuid`, `url`, `ipnet` and `chrono` | With `jiff` in place of `chrono` |
|---|---|---|---|
| `uuid` | `String` | `uuid::Uuid` | `uuid::Uuid` |
| `date-time` | `String` | `chrono::DateTime<chrono::Utc>` | `jiff::Timestamp` |
| `date` | `String` | `chrono::NaiveDate` | `jiff::civil::Date` |
| `ipv4` | `std::net::Ipv4Addr` | `std::net::Ipv4Addr` | `std::net::Ipv4Addr` |
| `ipv6` | `std::net::Ipv6Addr` | `std::net::Ipv6Addr` | `std::net::Ipv6Addr` |
| `time` | `String` | `chrono::NaiveTime` | `jiff::civil::Time` |
| `base64url` | `api::Base64Url` | `api::Base64Url` | `api::Base64Url` |
| `decimal` | `String` | `String` | `String` |
| `char` | `char` | `char` | `char` |
| `date-time-local` | `String` | `chrono::NaiveDateTime` | `jiff::civil::DateTime` |
| `time-local` | `String` | `chrono::NaiveTime` | `jiff::civil::Time` |
| `uri` | `String` | `url::Url` | `url::Url` |
| `ipv4-cidr` | `String` | `ipnet::Ipv4Net` | `ipnet::Ipv4Net` |
| `ipv6-cidr` | `String` | `ipnet::Ipv6Net` | `ipnet::Ipv6Net` |
| `email` | `api::Email` | `api::Email` | `api::Email` |
| `idn-email` | `api::IdnEmail` | `api::IdnEmail` | `api::IdnEmail` |
| `hostname` | `api::Hostname` | `api::Hostname` | `api::Hostname` |
| `idn-hostname` | `api::IdnHostname` | `api::IdnHostname` | `api::IdnHostname` |
| `http-date` | `String` | `String` | `String` |
| `duration` | `String` | `String` | `String` |
| `uri-reference` | `String` | `String` | `String` |
| `iri` | `String` | `String` | `String` |
| `iri-reference` | `String` | `String` | `String` |
| `json-pointer` | `String` | `String` | `String` |
| `relative-json-pointer` | `String` | `String` | `String` |
| `regex` | `String` | `String` | `String` |
| `uri-template` | `String` | `String` | `String` |
