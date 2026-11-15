//! The client's transports over HTTP libraries, one per feature.

// Browser handles are `!Send`: the adapter only fits the `Transport` of wasm32.
#[cfg(all(feature = "gloo-net", target_arch = "wasm32"))]
pub(crate) mod gloo;
#[cfg(feature = "reqwest")]
pub(crate) mod reqwest;
#[cfg(feature = "reqwest-middleware")]
pub(crate) mod reqwest_middleware;
pub(crate) mod tower;
#[cfg(feature = "ureq")]
pub(crate) mod ureq;
