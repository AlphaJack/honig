//! A server in memory as the client's transport.

use std::future::Future;

/// A transport that gives each request to `answer`, such as a closure around a server's `handle`. The clones that
/// tower makes share it.
pub fn loopback<F, Fut>(answer: F) -> impl honig::Transport
where
    F: Fn(honig::Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = honig::Response> + Send + 'static,
{
    let answer = std::sync::Arc::new(answer);
    tower::service_fn(move |req| {
        let answered = answer(req);
        async move { Ok::<_, std::convert::Infallible>(answered.await) }
    })
}
