//! The half of the generated client: the `Transport` bound over tower services, event stream reconnection and the
//! `OAuth` flows.

pub(crate) mod adapters;
pub(crate) mod auth;
#[cfg(feature = "oauth")]
pub(crate) mod oauth;
pub(crate) mod reconnect;
pub(crate) mod transport;
