//! Native targets. Futures and transports are `Send` and `Sync`. A thread wakes a task after a wait.

use std::{future::Future, pin::Pin};

use futures_core::Stream;

/// `Send` except on `wasm32`. There browser fetch futures are `!Send`.
pub trait MaybeSend: Send {}
impl<T: Send + ?Sized> MaybeSend for T {}

/// `Sync` everywhere except `wasm32`. See [`MaybeSend`].
pub trait MaybeSync: Sync {}
impl<T: Sync + ?Sized> MaybeSync for T {}

/// A future that is `Send` except on `wasm32`.
pub type Pending<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Waits `delay` under any executor. A thread sleeps it out and then wakes the task.
pub(crate) fn sleep(delay: std::time::Duration) -> Sleep {
    Sleep { until: std::time::Instant::now() + delay, waker: None }
}

/// The future [`sleep`] returns.
pub(crate) struct Sleep {
    until: std::time::Instant,
    /// The task to wake. It is shared with the thread that wakes it. None until the first poll that has to wait.
    waker: Option<std::sync::Arc<std::sync::Mutex<Option<std::task::Waker>>>>,
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        let now = std::time::Instant::now();
        if now >= self.until {
            return std::task::Poll::Ready(());
        }
        if let Some(waker) = &self.waker {
            if let Ok(mut w) = waker.lock() {
                *w = Some(cx.waker().clone());
            }
        } else {
            let waker = std::sync::Arc::new(std::sync::Mutex::new(Some(cx.waker().clone())));
            let (shared, left) = (waker.clone(), self.until - now);
            std::thread::spawn(move || {
                #[expect(clippy::disallowed_methods, reason = "the timer itself: nothing else to wait on")]
                std::thread::sleep(left);
                if let Some(w) = shared.lock().ok().and_then(|mut w| w.take()) {
                    w.wake();
                }
            });
            self.waker = Some(waker);
        }
        // The thread may have woken the task just before its waker was replaced: the clock decides.
        if std::time::Instant::now() >= self.until { std::task::Poll::Ready(()) } else { std::task::Poll::Pending }
    }
}

/// Returns `items` with `beat()` after each quiet `every`.
pub(crate) fn heartbeat<S>(items: S, every: std::time::Duration, beat: fn() -> S::Item) -> impl Stream<Item = S::Item>
where
    S: Stream + Unpin + Send,
{
    Heartbeat { items, every, beat, quiet: None }
}

/// Items with a beat after each quiet `every`.
struct Heartbeat<S, T> {
    items: S,
    every: std::time::Duration,
    beat: fn() -> T,
    /// The wait that runs while the next item is awaited. An item or a beat starts the next one.
    quiet: Option<Sleep>,
}

impl<S: Stream + Unpin> Stream for Heartbeat<S, S::Item> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<S::Item>> {
        let this = self.get_mut();
        if let std::task::Poll::Ready(item) = Pin::new(&mut this.items).poll_next(cx) {
            this.quiet = None;
            return std::task::Poll::Ready(item);
        }
        let quiet = this.quiet.get_or_insert_with(|| sleep(this.every));
        std::task::ready!(Pin::new(quiet).poll(cx));
        this.quiet = None;
        std::task::Poll::Ready(Some((this.beat)()))
    }
}
