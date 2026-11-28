//! The allocation bench of a pull request: no kind of path makes more than its `max_allocs` times the allocations of
//! its hand-rolled baseline at any of `cost::BENCH_SIZES`. Counts do not depend on the machine or the build, so it runs
//! with the other tests. The one exception is the box of each `std::sync::Mutex` on Apple targets: it is subtracted
//! from the count of the generated side (`cost::lock_box`), so the budgets set on the CI apply on a Mac too. An extra
//! copy of a body or a round trip through `serde_json::Value` shows here. `benches/time.rs` times the same paths.

#[path = "../cost/mod.rs"]
mod cost;
#[path = "../cost/roundtrip.rs"]
mod roundtrip;

/// Those of a roundtrip: the slot of the body that the generated client sends too.
const ROUNDTRIP_LOCKS: u64 = cost::SERVER_LOCKS + 1;

#[test]
fn no_generated_path_allocates_much_more_than_its_baseline() {
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::bench_table("head", "base", "999999");
    let mut rows = vec![];
    for size in cost::BENCH_SIZES {
        let (generated, baseline) = cost::at(size);
        for path in cost::paths(size) {
            let (allocs, baseline_allocs) = runtime.block_on(async {
                cost::same_answer(&generated, &baseline, &path).await.unwrap();
                (cost::heap(&generated, &path).await.0, cost::heap(&baseline, &path).await.0)
            });
            let allocs = allocs.saturating_sub(cost::SERVER_LOCKS * cost::lock_box());
            let ratio = allocs as f64 / baseline_allocs.max(1) as f64;
            let sides = [allocs.to_string(), baseline_allocs.to_string()];
            rows.push(cost::Measured { path: path.name, size, sides, ratio, max: path.max_allocs });
        }
    }
    failed.extend(cost::print_rows(&table, rows, None));
    assert!(failed.is_empty(), "more allocations than the budget:\n{}", failed.join("\n"));
}

/// The same of a roundtrip: the generated client and server against a hand-rolled client and server.
#[test]
fn no_generated_roundtrip_allocates_much_more_than_its_baseline() {
    let runtime = cost::runtime();
    let mut failed = vec![];
    let table = cost::bench_table("head", "base", "999999");
    let mut rows = vec![];
    for size in cost::BENCH_SIZES {
        let (head, base) = roundtrip::at(size);
        for trip in &roundtrip::TRIPS {
            let (allocs, baseline_allocs) = runtime.block_on(async {
                roundtrip::same_answer(&head, &base, trip.name).await.unwrap();
                let ours = cost::heap_of(async || {
                    roundtrip::head(&head, trip.name).await.ok();
                });
                let theirs = cost::heap_of(async || {
                    roundtrip::base(&base, trip.name).await.ok();
                });
                (ours.await.0, theirs.await.0)
            });
            let allocs = allocs.saturating_sub(ROUNDTRIP_LOCKS * cost::lock_box());
            let ratio = allocs as f64 / baseline_allocs.max(1) as f64;
            let sides = [allocs.to_string(), baseline_allocs.to_string()];
            rows.push(cost::Measured { path: trip.name, size, sides, ratio, max: trip.max_allocs });
        }
    }
    failed.extend(cost::print_rows(&table, rows, None));
    assert!(failed.is_empty(), "more allocations than the budget:\n{}", failed.join("\n"));
}

/// Collecting a binary body keeps one copy of it, plus the chunks while they arrive: no buffer grows by doubling and
/// none is copied again. With a known length the buffer is reserved once; a body of one chunk is not copied at all.
#[test]
fn a_collected_body_is_copied_at_most_once() {
    const CHUNK: usize = 256 * 1024;
    const CHUNKS: usize = 16;
    let body = |len: Option<u64>, chunks: usize| {
        let pieces = (0..chunks).map(|_| Ok(honig::Bytes::from(vec![7_u8; CHUNK])));
        honig::Body::stream(honig::ByteStream::chunks(futures_util::stream::iter(pieces), len))
    };
    let runtime = cost::runtime();
    let peak = |len: Option<u64>, chunks: usize| {
        runtime
            .block_on(cost::heap_of(async || {
                assert_eq!(body(len, chunks).collect().await.unwrap().len(), chunks * CHUNK);
            }))
            .1
    };
    let size_kib = (CHUNKS * CHUNK / 1024) as isize;
    let known = peak(Some((CHUNKS * CHUNK) as u64), CHUNKS);
    let unknown = peak(None, CHUNKS);
    let one = peak(None, 1);
    let chunk_kib = (CHUNK / 1024) as isize;
    // The first two chunks are held when the buffer is reserved: one chunk alone is not copied.
    assert!(known <= size_kib + 2 * chunk_kib, "a known length: {known} KiB for {size_kib} KiB");
    assert!(unknown * 10 <= size_kib * 16 + chunk_kib * 10, "no second copy: {unknown} KiB for {size_kib} KiB");
    assert!(one <= chunk_kib, "one chunk is not copied: {one} KiB for {chunk_kib} KiB");
}

/// A download whose `Content-Length` the server states is collected into one buffer of that size: the reqwest
/// transport hands the length on, so the buffer never grows.
#[test]
fn a_download_of_a_stated_length_is_collected_without_growing() {
    use honig::Service as _;
    const LEN: usize = 4 << 20;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let server = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        server.block_on(async move {
            listener.set_nonblocking(true).unwrap();
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            let app = axum::Router::new().route("/blob", axum::routing::get(|| async { vec![7_u8; LEN] }));
            axum::serve(listener, app).await.unwrap();
        });
    });
    let runtime = cost::runtime();
    let transport = honig::ReqwestTransport::new(format!("http://{addr}")).unwrap();
    let peak = runtime
        .block_on(cost::heap_of(async || {
            let req = honig::http::Request::get("/blob").body(honig::Body::default()).unwrap();
            let resp = transport.clone().call(req).await.unwrap();
            assert_eq!(resp.into_body().collect().await.unwrap().len(), LEN);
        }))
        .1;
    assert!(peak * 10 <= (LEN / 1024) as isize * 11, "{peak} KiB for {} KiB", LEN / 1024);
}
