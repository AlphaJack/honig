#![doc = include_str!("../README.md")]
// Every request goes through this crate: `push_str(&format!(..))` and `.map(|x| format!(..)).collect()` allocate a
// `String` per piece. The workspace allows both for code that runs once; `codec::push_all` appends without one.
#![warn(clippy::format_push_string, clippy::format_collect)]

mod auth;
#[cfg(feature = "base64")]
mod base64;
mod binary;
mod builder;
#[cfg(feature = "client")]
mod client;
mod codec;
mod coerce;
mod decimal;
mod duration;
mod error;
mod escape;
mod exact;
mod finite;
mod format_checks;
mod formats;
mod formatted;
mod full_time;
mod header;
mod http_date;
mod integral;
mod json;
mod json_pointer;
mod links;
mod linkset;
mod media;
mod message;
mod no_seq;
mod nullable;
mod params;
mod path;
#[cfg(feature = "regress")]
mod pattern;
mod raw_value;
mod reject;
mod scalar;
#[cfg(feature = "server")]
mod server;
mod servers;
mod source;
mod status;
mod stream;
mod tagged;
mod text;
mod trace_context;
mod transport;
mod unique_keys;
mod uri;
mod uri_template;
mod validate;
#[cfg(feature = "xml")]
mod xml;
#[cfg(feature = "xml")]
mod xml_tree;

pub use auth::Presented;
#[cfg(feature = "base64")]
pub use binary::Base64Url;
pub use binary::Binary;
pub use builder::BuildError;
pub use bytes::Bytes;
#[cfg(feature = "oauth")]
pub use client::oauth::{AuthMetadata, DeviceCode, DevicePoll, Token};
#[cfg(feature = "pkce")]
pub use client::oauth::{NoRandomness, Pkce};
#[cfg(feature = "client")]
pub use client::transport::{Transport, send, send_streaming};
#[cfg(feature = "multipart")]
pub use codec::PartTypes;
pub use decimal::Decimal;
pub use error::{ApiError, Error};
pub use formatted::Formatted;
pub use formatted::UuidText;
pub use formatted::{
    DateText, DateTimeLocalText, DateTimeText, Decimal128Text, DecimalText, DurationText, HttpDateText,
};
pub use formatted::{EmailText, HostnameText, IdnEmailText, IdnHostnameText};
pub use formatted::{Ipv4CidrText, Ipv6CidrText, IriReferenceText, IriText, JsonPointerText, RegexText};
pub use formatted::{RelativeJsonPointerText, TimeLocalText, UriReferenceText, UriTemplateText, UriText};
pub use full_time::FullTime;
/// The `http` crate that [`Request`], [`Response`] and their parts come from.
pub use http;
pub use links::Exchange;
/// The string header helpers that generated code calls. Your code uses the `headers()` of `http`.
#[doc(hidden)]
pub use message::Message;
pub use message::{Request, Response};
pub use nullable::Nullable;
pub use reject::{ErrorField, ErrorSchema, Reason, Rejection};
#[cfg(feature = "server")]
pub use server::conditional::CacheValidators;
#[cfg(feature = "server")]
pub use server::event::peer;
#[cfg(feature = "server")]
pub use server::event::{Context, EventKey, Failure, RequestEvent, RequestEventFields};
#[cfg(feature = "server")]
pub use server::operation::Operation;
#[cfg(feature = "server")]
pub use server::service::{
    Anonymous, ApiService, Authenticate, AuthenticateLayer, Authenticated, Authentication, ClientCertificateVerified,
    Endpoint, Errors, MatchedOperation, Secured, Server, read_body,
};
pub use source::{Blob, Body, ByteStream, Sse};
pub use stream::{Framing, ItemStream, Items, Streamed};
#[cfg(any(feature = "client", feature = "server"))]
pub use tower_service::Service;
pub use transport::block_on;
pub use validate::{InstancePath, Violation};

// The hidden items that the modules use to name each other from the root.
#[cfg_attr(not(test), expect(clippy::wildcard_imports, reason = "which items each feature set uses differs"))]
use __private::*;

/// What generated code calls: codecs, parameter styles, routing, validation checks and `serve`. It is not for your
/// code. It changes with the generator. The version of the runtime is pinned to the version of the generator.
#[doc(hidden)]
pub mod __private {
    pub use crate::auth::*;
    #[cfg(feature = "base64")]
    pub use crate::base64::*;
    pub use crate::binary::*;
    pub use crate::builder::{Builder, field};
    /// Library types of formats, read with their text checked.
    pub mod formats {
        pub use crate::formats::*;
    }
    #[cfg(feature = "client")]
    pub use crate::client::auth::*;
    #[cfg(feature = "oauth")]
    pub use crate::client::oauth::*;
    #[cfg(feature = "client")]
    pub use crate::client::transport::*;
    pub use crate::codec::*;
    pub use crate::coerce::*;
    pub use crate::header::header_text;
    pub use crate::json::to_value;
    pub use crate::links::*;
    pub use crate::linkset::*;
    pub use crate::media::*;
    pub use crate::message::*;
    pub use crate::no_seq::NoSeq;
    pub use crate::nullable::{not_null, required_nullable};
    pub use crate::params::*;
    #[cfg(feature = "regress")]
    pub use crate::pattern::*;
    pub use crate::reject::*;
    #[cfg(feature = "server")]
    pub use crate::server::auth::*;
    #[cfg(feature = "server")]
    pub use crate::server::operation::*;
    #[cfg(feature = "server")]
    pub use crate::server::route::*;
    #[cfg(feature = "server")]
    pub use crate::server::serve::*;
    #[cfg(feature = "server")]
    pub use crate::server::service::{BoxFuture, Operations};
    pub use crate::servers::*;
    pub use crate::source::*;
    pub use crate::status::*;
    pub use crate::stream::*;
    pub use crate::tagged::*;
    pub use crate::text::*;
    pub use crate::trace_context::*;
    pub use crate::transport::*;
    pub use crate::unique_keys::UniqueKeys;
    pub use crate::validate::*;
    #[cfg(feature = "xml")]
    pub use crate::xml::*;
    #[cfg(feature = "xml")]
    pub use crate::xml_tree::*;
}

#[cfg(feature = "reqwest")]
pub use client::adapters::reqwest::{Execute, ReqwestTransport};
#[cfg(feature = "reqwest-middleware")]
pub use client::adapters::reqwest_middleware::ReqwestMiddlewareTransport;
#[cfg(feature = "client")]
pub use client::adapters::tower::TowerTransport;
#[cfg(feature = "ureq")]
pub use client::adapters::ureq::UreqTransport;
