//! `wasm32`: browser futures and handles are neither `Send` nor `Sync`.

use std::{future::Future, pin::Pin};

/// `Send` except on `wasm32`. There browser fetch futures are `!Send`.
pub trait MaybeSend {}
impl<T: ?Sized> MaybeSend for T {}

/// `Sync` everywhere except `wasm32`. See [`MaybeSend`].
pub trait MaybeSync {}
impl<T: ?Sized> MaybeSync for T {}

/// A future that is `Send` except on `wasm32`.
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Returns `items` as they are. A browser serves no event stream: nothing needs a beat to keep it open.
pub(crate) fn heartbeat<S>(items: S, _: std::time::Duration, _: fn() -> S::Item) -> S
where
    S: futures_core::Stream + Unpin,
{
    items
}

/// Waits `delay` on the browser's timer.
pub(crate) fn sleep(delay: std::time::Duration) -> impl Future<Output = ()> {
    gloo_timers::future::sleep(delay)
}

/// The transports of the browser: `GlooTransport` sends through `fetch`.
pub mod browser {
    #[cfg(feature = "gloo-net")]
    pub use crate::client::adapters::gloo::GlooTransport;
}
