//! Bodies a piece at a time across a transport and the `Send` bounds and timers of the platform.

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
use native as imp;
#[cfg(target_arch = "wasm32")]
use wasm as imp;

pub use imp::*;

/// Runs `fut` on this thread with no async runtime.
///
/// The thread sleeps while the future waits and its waker wakes it. `UreqTransport` calls run this way. So do
/// multipart parts already in memory.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}
