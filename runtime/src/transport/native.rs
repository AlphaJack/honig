//! Native targets. Futures and transports are `Send` and `Sync`. A thread wakes a task after a wait.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};
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

/// Waits `delay` under any executor. One thread for the process wakes each task at its deadline.
pub(crate) fn sleep(delay: Duration) -> Sleep {
    static IDS: AtomicU64 = AtomicU64::new(0);
    Sleep { until: Instant::now() + delay, id: IDS.fetch_add(1, Ordering::Relaxed), waiting: false }
}

/// The future [`sleep`] returns.
pub(crate) struct Sleep {
    until: Instant,
    /// Tells two sleeps with the same deadline apart in the queue of the timer thread.
    id: u64,
    /// Whether its waker is in that queue.
    waiting: bool,
}

/// The waiting sleeps by deadline and id with the task each wakes, and the signal that the earliest one changed.
struct Timers {
    queue: Mutex<BTreeMap<(Instant, u64), Waker>>,
    changed: Condvar,
}

/// Returns the timers. The first call starts the thread that wakes them.
fn timers() -> &'static Timers {
    static TIMERS: OnceLock<Timers> = OnceLock::new();
    TIMERS.get_or_init(|| {
        std::thread::spawn(|| wake_due(timers()));
        Timers { queue: Mutex::new(BTreeMap::new()), changed: Condvar::new() }
    })
}

/// Wakes each task at its deadline: forever, on the timer thread. A task is woken outside the lock, since waking it may
/// poll it.
fn wake_due(t: &Timers) {
    let lock = || t.queue.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        let now = Instant::now();
        let mut queue = lock();
        let later = queue.split_off(&(now, u64::MAX));
        let due = std::mem::replace(&mut *queue, later);
        let next = queue.first_key_value().map(|((until, _), _)| *until - now);
        if due.is_empty() {
            match next {
                Some(left) => drop(t.changed.wait_timeout(queue, left)),
                None => drop(t.changed.wait(queue)),
            }
        } else {
            drop(queue);
            due.into_values().for_each(Waker::wake);
        }
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if Instant::now() >= self.until {
            return Poll::Ready(());
        }
        let t = timers();
        t.queue.lock().unwrap_or_else(PoisonError::into_inner).insert((self.until, self.id), cx.waker().clone());
        self.waiting = true;
        t.changed.notify_one();
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if self.waiting {
            timers().queue.lock().unwrap_or_else(PoisonError::into_inner).remove(&(self.until, self.id));
        }
    }
}

/// Returns `items` with `beat()` after each quiet `every`.
pub(crate) fn heartbeat<S>(items: S, every: Duration, beat: fn() -> S::Item) -> impl Stream<Item = S::Item>
where
    S: Stream + Unpin + Send,
{
    Heartbeat { items, every, beat, quiet: None }
}

/// Items with a beat after each quiet `every`.
struct Heartbeat<S, T> {
    items: S,
    every: Duration,
    beat: fn() -> T,
    /// The wait that runs while the next item is awaited. An item or a beat starts the next one.
    quiet: Option<Sleep>,
}

impl<S: Stream + Unpin> Stream for Heartbeat<S, S::Item> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = self.get_mut();
        if let Poll::Ready(item) = Pin::new(&mut this.items).poll_next(cx) {
            this.quiet = None;
            return Poll::Ready(item);
        }
        let quiet = this.quiet.get_or_insert_with(|| sleep(this.every));
        std::task::ready!(Pin::new(quiet).poll(cx));
        this.quiet = None;
        Poll::Ready(Some((this.beat)()))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::task::Wake;

    use super::*;

    /// A task that counts its wakes. Its `Arc` count says who still holds its waker.
    #[derive(Default)]
    struct Task(AtomicU64);

    impl Wake for Task {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `n` items with a pending poll before each.
    struct Turns(u32, bool);

    impl Stream for Turns {
        type Item = u32;

        fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<u32>> {
            self.1 = !self.1;
            match (self.1, self.0) {
                (true, _) => Poll::Pending,
                (false, 0) => Poll::Ready(None),
                (false, n) => {
                    self.0 = n - 1;
                    Poll::Ready(Some(n))
                }
            }
        }
    }

    #[test]
    fn a_heartbeat_holds_no_task_after_its_items() {
        let task = Arc::new(Task::default());
        let waker = Waker::from(Arc::clone(&task));
        let mut cx = Context::from_waker(&waker);
        let mut beats = std::pin::pin!(heartbeat(Turns(500, false), Duration::from_secs(20), || 0));
        let mut items = 0;
        loop {
            match beats.as_mut().poll_next(&mut cx) {
                Poll::Ready(Some(_)) => items += 1,
                Poll::Ready(None) => break,
                Poll::Pending => {}
            }
        }
        assert_eq!(items, 500);
        drop(waker);
        assert_eq!(Arc::strong_count(&task), 1, "each wait still holds the task");
        assert_eq!(task.0.load(Ordering::Relaxed), 0);
    }

    /// A stream that never yields.
    struct Quiet;

    impl Stream for Quiet {
        type Item = u32;

        fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<u32>> {
            Poll::Pending
        }
    }

    #[test]
    fn a_quiet_stream_beats() {
        let mut beats = heartbeat(Quiet, Duration::from_millis(10), || 0);
        assert_eq!(crate::block_on(std::future::poll_fn(|cx| Pin::new(&mut beats).poll_next(cx))), Some(0));
    }
}
