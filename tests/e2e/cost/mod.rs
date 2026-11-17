//! The cost of a request, in one place: each kind of path as a generated server and a hand-rolled axum baseline, the
//! request of each size, how one request is served, timed and counted, and the budget of each kind. Both serve in the
//! process through `oneshot`, so no socket or other process adds noise.
//!
//! - `tests/allocations.rs`: no kind allocates more than `max_allocs` times its baseline at any of [`BENCH_SIZES`].
//! - `benches/time.rs`: no kind takes more than `max_time` times its baseline in a release build at any of them.
//! - `roundtrip.rs` with `benches/roundtrip.rs` and `tests/allocations.rs`: the same through the generated client
//!   against a hand-rolled one.
//! - `benches/size_sweep.rs`: the CSV of every kind from 256 B to 32 MiB.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::Multipart;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response, Sse, sse::Event};
use axum::routing::{get, post, put};
use base64::Engine;
use bench_support::table::{self, Column, Table};
use futures_util::StreamExt;
use http_body_util::BodyExt;
use tower::ServiceExt;

use e2e::{
    v32_download as dl, v32_multipart as mp, v32_sse_items as sse, v32_uploads as up, v32_validation as val,
    v32_xml as xml,
};

/// The payload of every path in bytes, set before the routers serve.
pub static SIZE: AtomicUsize = AtomicUsize::new(0);
/// The chunks of a download and the frames of a request body: 64 KiB, as a socket delivers them.
pub const FRAME: usize = 64 << 10;
/// About the bytes of one SSE or JSON Lines item on the wire.
const ITEM: usize = 32;
/// About the bytes of one manifest entry in JSON.
const ENTRY: usize = 26;
/// The largest body either side reads: above the largest size.
const LIMIT: usize = 64 << 20;
/// MIME breaks base64 into lines of this many characters (RFC 2045 section 6.8).
const MIME_LINE: usize = 76;

fn size() -> usize {
    SIZE.load(Ordering::Relaxed)
}

fn chunks() -> impl Iterator<Item = Vec<u8>> + Send {
    let n = size();
    (0..n.div_ceil(FRAME)).map(move |i| vec![7; FRAME.min(n - i * FRAME)])
}

fn tile(i: usize) -> sse::api::types::TileEvent {
    sse::api::types::TileEvent { tile: format!("t{i}"), rows: i64::try_from(i).unwrap_or_default() }
}

fn items() -> usize {
    (size() / ITEM).max(1)
}

pub fn manifest() -> Vec<String> {
    (0..(size() / ENTRY).max(1)).map(|i| format!("photos/product-{i:08}.jpg")).collect()
}

fn unused<T>() -> std::future::Ready<Result<T, mp::api::ApiError>> {
    std::future::ready(Err(mp::api::ApiError::new(
        mp::api::http::StatusCode::INTERNAL_SERVER_ERROR,
        "not in the sweep",
    )))
}

struct Scans;

impl mp::api::Api for Scans {
    type Principal = ();

    fn upload(
        &self,
        _: mp::api::operations::upload::Params,
        _: &mp::api::Context<'_>,
    ) -> impl Future<Output = Result<mp::api::operations::upload::Response, mp::api::ApiError>> + Send {
        unused()
    }
    fn put_icon(
        &self,
        _: mp::api::operations::put_icon::Params,
        _: &mp::api::Context<'_>,
    ) -> impl Future<Output = Result<mp::api::operations::put_icon::Response, mp::api::ApiError>> + Send {
        unused()
    }
    async fn upload_scan(
        &self,
        p: mp::api::operations::upload_scan::Params,
        _: &mp::api::Context<'_>,
    ) -> Result<mp::api::operations::upload_scan::Response, mp::api::ApiError> {
        std::hint::black_box(p.body.scan.bytes.len());
        Ok(mp::api::operations::upload_scan::Response::NoContent)
    }
    fn deploy(
        &self,
        _: mp::api::operations::deploy::Params,
        _: &mp::api::Context<'_>,
    ) -> impl Future<Output = Result<mp::api::operations::deploy::Response, mp::api::ApiError>> + Send {
        unused()
    }
}

struct Files;

impl up::api::Api for Files {
    type Principal = ();

    async fn put_file(
        &self,
        p: up::api::operations::put_file::Params,
        _: &up::api::Context<'_, ()>,
    ) -> Result<up::api::operations::put_file::Response, up::api::ApiError> {
        let mut body = p.body;
        let (mut bytes, mut chunks) = (0, 0);
        while let Some(chunk) = futures_util::StreamExt::next(&mut body).await {
            bytes += chunk?.len();
            chunks += 1;
        }
        let bytes = i64::try_from(bytes).unwrap_or_default();
        Ok(up::api::operations::put_file::Response::Ok(up::api::types::PutFileOk { bytes, chunks }))
    }
}

struct Exports;

impl dl::api::Api for Exports {
    type Principal = ();

    async fn get_export(
        &self,
        _: dl::api::operations::get_export::Params,
        _: &dl::api::Context<'_>,
    ) -> Result<dl::api::operations::get_export::Response, dl::api::ApiError> {
        let blob = dl::api::runtime::Blob::stream(futures_util::stream::iter(chunks()));
        Ok(dl::api::operations::get_export::Response::Ok(blob))
    }
    async fn get_manifest(
        &self,
        _: dl::api::operations::get_manifest::Params,
        _: &dl::api::Context<'_>,
    ) -> Result<dl::api::operations::get_manifest::Response, dl::api::ApiError> {
        Ok(dl::api::operations::get_manifest::Response::Ok { body: manifest(), e_tag: None, last_modified: None })
    }
    async fn replace_manifest(
        &self,
        p: dl::api::operations::replace_manifest::Params,
        _: &dl::api::Context<'_>,
    ) -> Result<dl::api::operations::replace_manifest::Response, dl::api::ApiError> {
        std::hint::black_box(p.body.len());
        Ok(dl::api::operations::replace_manifest::Response::NoContent)
    }
    fn get_thumbnail(
        &self,
        _: dl::api::operations::get_thumbnail::Params,
        _: &dl::api::Context<'_>,
    ) -> impl Future<Output = Result<dl::api::operations::get_thumbnail::Response, dl::api::ApiError>> + Send {
        unused()
    }
}

struct Tiles;

impl sse::api::Api for Tiles {
    type Principal = ();

    async fn tile_stream(
        &self,
        _: sse::api::operations::tile_stream::Params,
        _: &sse::api::Context<'_>,
    ) -> Result<sse::api::operations::tile_stream::Response, sse::api::ApiError> {
        let items = sse::api::Items::stream(futures_util::stream::iter((0..items()).map(tile)));
        Ok(sse::api::operations::tile_stream::Response::Ok(items))
    }
    async fn tile_lines(
        &self,
        _: sse::api::operations::tile_lines::Params,
        _: &sse::api::Context<'_>,
    ) -> Result<sse::api::operations::tile_lines::Response, sse::api::ApiError> {
        let items = sse::api::Items::stream(futures_util::stream::iter((0..items()).map(tile)));
        Ok(sse::api::operations::tile_lines::Response::Ok(items))
    }
    fn tile_updates(
        &self,
        _: sse::api::operations::tile_updates::Params,
        _: &sse::api::Context<'_>,
    ) -> impl Future<Output = Result<sse::api::operations::tile_updates::Response, sse::api::ApiError>> + Send {
        unused()
    }
    fn chat_stream(
        &self,
        _: sse::api::operations::chat_stream::Params,
        _: &sse::api::Context<'_>,
    ) -> impl Future<Output = Result<sse::api::operations::chat_stream::Response, sse::api::ApiError>> + Send {
        unused()
    }
}

struct Books;

impl xml::api::Api for Books {
    type Principal = ();

    async fn add_book(
        &self,
        p: xml::api::operations::add_book::Params,
        _: &xml::api::Context<'_>,
    ) -> Result<xml::api::operations::add_book::Response, xml::api::ApiError> {
        Ok(xml::api::operations::add_book::Response::Created(p.body))
    }
}

fn not_here<T>() -> std::future::Ready<Result<T, val::api::ApiError>> {
    std::future::ready(Err(val::api::ApiError::new(
        val::api::http::StatusCode::INTERNAL_SERVER_ERROR,
        "not in the sweep",
    )))
}

struct Members;

impl val::api::Api for Members {
    type Principal = ();

    fn create_user(
        &self,
        _: val::api::operations::create_user::Params,
        _: &val::api::Context<'_, ()>,
    ) -> impl Future<Output = Result<val::api::operations::create_user::Response, val::api::ApiError>> + Send {
        not_here()
    }
    fn create_shipment(
        &self,
        _: val::api::operations::create_shipment::Params,
        _: &val::api::Context<'_, ()>,
    ) -> impl Future<Output = Result<val::api::operations::create_shipment::Response, val::api::ApiError>> + Send {
        not_here()
    }
    fn login(
        &self,
        _: val::api::operations::login::Params,
        _: &val::api::Context<'_, ()>,
    ) -> impl Future<Output = Result<val::api::operations::login::Response, val::api::ApiError>> + Send {
        not_here()
    }
    async fn add_member(
        &self,
        _: val::api::operations::add_member::Params,
        _: &val::api::Context<'_, ()>,
    ) -> Result<val::api::operations::add_member::Response, val::api::ApiError> {
        Ok(val::api::operations::add_member::Response::Created)
    }
    fn add_badge(
        &self,
        _: val::api::operations::add_badge::Params,
        _: &val::api::Context<'_, ()>,
    ) -> impl Future<Output = Result<val::api::operations::add_badge::Response, val::api::ApiError>> + Send {
        not_here()
    }
}

/// The generated servers, each under the prefix of its fixture.
pub fn generated() -> Router {
    Router::new()
        .nest("/mp", mp::api::Server::new(Scans).max_body_bytes(LIMIT).into_axum())
        .nest("/up", up::api::Server::new(Files).max_body_bytes(LIMIT).into_axum())
        .nest("/dl", dl::api::Server::new(Exports).max_body_bytes(LIMIT).into_axum())
        .nest("/sse", sse::api::Server::new(Tiles).max_body_bytes(LIMIT).into_axum())
        .nest("/xml", xml::api::Server::new(Books).max_body_bytes(LIMIT).into_axum())
        .nest("/val", val::api::Server::new(Members).max_body_bytes(LIMIT).into_axum())
}

async fn baseline_scan(mut form: Multipart) -> StatusCode {
    let mut scanned = false;
    while let Ok(Some(field)) = form.next_field().await {
        let scan = field.name() == Some("scan");
        scanned |= scan;
        let Ok(data) = field.bytes().await else { return StatusCode::BAD_REQUEST };
        if scan {
            let unwrapped: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
            let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(unwrapped) else {
                return StatusCode::BAD_REQUEST;
            };
            std::hint::black_box(raw.len());
        }
    }
    // `scan` is required.
    if !scanned {
        return StatusCode::BAD_REQUEST;
    }
    StatusCode::NO_CONTENT
}

async fn baseline_put(body: Body) -> Response {
    let mut stream = body.into_data_stream();
    let (mut bytes, mut chunks) = (0, 0);
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { return StatusCode::PAYLOAD_TOO_LARGE.into_response() };
        bytes += chunk.len();
        chunks += 1;
    }
    axum::Json(serde_json::json!({"bytes": bytes, "chunks": chunks})).into_response()
}

async fn baseline_export() -> Response {
    let body = Body::from_stream(futures_util::stream::iter(chunks().map(Ok::<_, std::io::Error>)));
    ([(header::CONTENT_TYPE, "application/zip")], body).into_response()
}

async fn baseline_manifest() -> Response {
    axum::Json(manifest()).into_response()
}

async fn baseline_replace(axum::Json(names): axum::Json<Vec<String>>) -> StatusCode {
    std::hint::black_box(names.len());
    StatusCode::NO_CONTENT
}

async fn baseline_events() -> impl IntoResponse {
    Sse::new(futures_util::stream::iter((0..items()).map(|i| Event::default().json_data(tile(i)))))
}

async fn baseline_lines() -> Response {
    let lines = futures_util::stream::iter((0..items()).map(|i| {
        let mut line = serde_json::to_vec(&tile(i)).unwrap_or_default();
        line.push(b'\n');
        Ok::<_, std::io::Error>(line)
    }));
    ([(header::CONTENT_TYPE, "application/jsonl")], Body::from_stream(lines)).into_response()
}

/// The `bk:book` root, its namespace declared on it, as the generated side writes it.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename = "bk:book")]
pub struct Book {
    #[serde(rename = "@xmlns:bk")]
    pub namespace: String,
    #[serde(rename = "@id")]
    pub id: i64,
    pub title: String,
    pub authors: Authors,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Authors {
    #[serde(default)]
    pub author: Vec<Author>,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct Author {
    pub name: String,
    pub born: Option<i64>,
}

async fn baseline_book(body: Bytes) -> Response {
    let Some(book) = std::str::from_utf8(&body).ok().and_then(|s| quick_xml::de::from_str::<Book>(s).ok()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(xml) = quick_xml::se::to_string(&book) else { return StatusCode::INTERNAL_SERVER_ERROR.into_response() };
    let out = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{xml}");
    (StatusCode::CREATED, [(header::CONTENT_TYPE, "application/xml")], out).into_response()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Admin,
    User,
}

#[derive(serde::Deserialize)]
struct Member {
    name: String,
    age: Option<i64>,
    role: Role,
}

/// What the `User` schema asks of the fields that the form sends: `name` 2 to 10 characters from an uppercase letter,
/// `age` from 0 below 150 and `role` one of its values. The generated side checks them all.
async fn baseline_member(axum::Form(m): axum::Form<Member>) -> StatusCode {
    static NAME: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = NAME.get_or_init(|| regex::Regex::new(r"^\p{Lu}").expect("a valid pattern"));
    let name_ok = (2..=10).contains(&m.name.chars().count()) && pattern.is_match(&m.name);
    if !name_ok || m.age.is_some_and(|a| !(0..150).contains(&a)) {
        return StatusCode::BAD_REQUEST;
    }
    std::hint::black_box((m.name, m.age, matches!(m.role, Role::Admin | Role::User)));
    StatusCode::CREATED
}

/// The hand-rolled baselines on the same paths, nested under the same prefixes: nesting costs axum a few allocations.
pub fn baseline() -> Router {
    Router::new()
        .nest("/mp", Router::new().route("/scans", post(baseline_scan)))
        .nest("/up", Router::new().route("/files/{name}", put(baseline_put)))
        .nest(
            "/dl",
            Router::new()
                .route("/exports/{id}", get(baseline_export))
                .route("/exports/{id}/manifest", get(baseline_manifest).put(baseline_replace)),
        )
        .nest(
            "/sse",
            Router::new().route("/tiles/stream", get(baseline_events)).route("/tiles/lines", get(baseline_lines)),
        )
        .nest("/xml", Router::new().route("/books", post(baseline_book)))
        .nest("/val", Router::new().route("/members", post(baseline_member)))
        .layer(axum::extract::DefaultBodyLimit::max(LIMIT))
}

/// A request body of `n` bytes for the upload.
pub fn upload_body(n: usize) -> Bytes {
    (0..n).map(|i| u8::try_from(i * 31 % 251).unwrap_or_default()).collect::<Vec<u8>>().into()
}

/// A multipart body whose `scan` part is `n` bytes in base64, broken into MIME lines.
pub fn scan_body(n: usize) -> Bytes {
    scan_form(&upload_body(n))
}

/// A multipart body whose `scan` part is `raw` in base64, broken into MIME lines.
pub fn scan_form(raw: &[u8]) -> Bytes {
    let text = base64::engine::general_purpose::STANDARD.encode(raw);
    let lines = text.as_bytes().chunks(MIME_LINE).collect::<Vec<_>>().join(&b"\r\n"[..]);
    let mut body =
        b"--XyZ\r\nContent-Disposition: form-data; name=\"scan\"\r\nContent-Transfer-Encoding: base64\r\n\r\n".to_vec();
    body.extend_from_slice(&lines);
    body.extend_from_slice(b"\r\n--XyZ--\r\n");
    body.into()
}

/// The content type of [`scan_body`].
pub const SCAN_TYPE: &str = "multipart/form-data; boundary=XyZ";

/// About the bytes of one author in the XML of a book.
pub const AUTHOR: usize = 64;

/// A book in XML of about `n` bytes: its authors grow with it.
pub fn book_body(n: usize) -> Bytes {
    let authors: String = (0..(n / AUTHOR).max(1))
        .map(|i| format!("<author><name>Author {i}</name><born>{}</born></author>", 1900 + i % 100))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><bk:book xmlns:bk=\"https://example.com/books\" id=\"1\">\
         <title>Dune</title><authors>{authors}</authors></bk:book>"
    )
    .into()
}

/// The manifest in JSON: a body of about `n` bytes.
pub fn manifest_body() -> Bytes {
    serde_json::to_vec(&manifest()).unwrap_or_default().into()
}

/// A form of a member: it is small whatever the size.
pub fn member_body() -> Bytes {
    Bytes::from_static(b"name=Alice&age=30&role=admin")
}

/// The sizes that both benches check every kind at. A kind keeps one budget across them. 2 KiB is about the median
/// JSON body of real APIs; 256 B, 16 KiB and 1 MiB bracket it.
#[allow(dead_code, reason = "the size sweep picks its own sizes")]
pub const BENCH_SIZES: [usize; 4] = [256, 2 << 10, 16 << 10, 1 << 20];

/// The table that a bench prints: per kind of path and size, what the generated side (`head`) and its baseline
/// (`base`) measured, as wide as `sample`, their ratio, the budget of the kind and whether it holds.
#[allow(dead_code, reason = "the size sweep prints no bench")]
pub fn bench_table(head: &'static str, base: &'static str, sample: &str) -> Table {
    let sizes = BENCH_SIZES.map(table::size);
    let widest_size = sizes.iter().max_by_key(|s| s.len()).map_or("", String::as_str);
    let widest_path = paths(BENCH_SIZES[0]).iter().map(|p| p.name.len()).max().unwrap_or_default();
    Table::new(&[
        Column::left("path", widest_path),
        Column::right("size", widest_size),
        Column::right(head, sample),
        Column::right(base, sample),
        Column::right("ratio", "99.99"),
        Column::right("max", "9.99"),
        Column::left("result", "FAIL".len()),
    ])
}

/// One row of a bench: a kind of path at a size, both sides as measured, their ratio and the budget of the kind.
#[allow(dead_code, reason = "the size sweep prints no bench")]
pub struct Measured {
    pub path: &'static str,
    pub size: usize,
    /// The generated side and its baseline, as the table shows them.
    pub sides: [String; 2],
    pub ratio: f64,
    pub max: f64,
}

/// Prints `rows` by path and then by size, so the sizes of one kind sit together. Returns each row over its budget.
#[allow(dead_code, reason = "the size sweep prints no bench")]
pub fn print_rows(table: &Table, mut rows: Vec<Measured>) -> Vec<String> {
    rows.sort_by_key(|r| (r.path, r.size));
    let mut over = vec![];
    for Measured { path, size, sides: [head, base], ratio, max } in rows {
        let failed = ratio > max;
        let result = if failed { "FAIL" } else { "ok" };
        let cells = [path.to_owned(), table::size(size), head, base, format!("{ratio:.2}"), format!("{max:.2}")];
        table.row(&[cells.as_slice(), &[result.to_owned()]].concat());
        if failed {
            over.push(format!("{path} at {}", table::size(size)));
        }
    }
    over
}

/// One kind of path, the request that it gets at one size, and its budgets against its baseline. The allocation budget
/// is the most that the kind makes at any of [`BENCH_SIZES`], rounded up to the next 0.05: counts are the same on every
/// run, so it needs no margin. The time budget is the worst ratio of the kind in the CI runs so far and a tenth more, rounded up
/// to the next 0.05: ratios differ between machines, so they come from the runner that enforces them. Lower a budget
/// when a change makes the kind cheaper.
pub struct Path {
    pub name: &'static str,
    method: &'static str,
    uri: &'static str,
    content_type: &'static str,
    accept: &'static str,
    body: Bytes,
    /// At most how many times the allocations of the baseline.
    #[allow(dead_code, reason = "only the allocation bench reads it")]
    pub max_allocs: f64,
    /// At most how many times the time of the baseline in a release build.
    #[allow(dead_code, reason = "only the time bench reads it")]
    pub max_time: f64,
}

/// Every kind of path at `size`. Set [`SIZE`] to it before the routers serve.
pub fn paths(size: usize) -> Vec<Path> {
    let path = |name, method, uri, accept, (max_allocs, max_time)| Path {
        name,
        method,
        uri,
        content_type: "",
        accept,
        body: Bytes::new(),
        max_allocs,
        max_time,
    };
    vec![
        Path {
            content_type: "application/octet-stream",
            body: upload_body(size),
            ..path("upload", "PUT", "/up/files/a", "*/*", (1.15, 2.0))
        },
        Path {
            content_type: SCAN_TYPE,
            body: scan_body(size),
            ..path("multipart_b64", "POST", "/mp/scans", "*/*", (1.3, 1.55))
        },
        path("download", "GET", "/dl/exports/x", "application/zip", (1.3, 2.25)),
        path("json_get", "GET", "/dl/exports/x/manifest", "application/json", (1.1, 1.55)),
        Path {
            content_type: "application/json",
            body: manifest_body(),
            ..path("json_put", "PUT", "/dl/exports/x/manifest", "*/*", (1.2, 1.55))
        },
        path("sse", "GET", "/sse/tiles/stream", "text/event-stream", (0.8, 0.95)),
        path("jsonl", "GET", "/sse/tiles/lines", "application/jsonl", (1.0, 1.75)),
        Path {
            content_type: "application/xml",
            body: book_body(size),
            ..path("xml", "POST", "/xml/books", "application/xml", (0.75, 1.15))
        },
        Path {
            content_type: "application/x-www-form-urlencoded",
            body: member_body(),
            ..path("form", "POST", "/val/members", "*/*", (1.15, 1.95))
        },
    ]
}

/// The request with its body in frames, as a socket delivers it. One `Bytes` would cost the server nothing to read.
fn request(path: &Path) -> Request<Body> {
    let body = path.body.clone();
    let frames: Vec<_> = (0..body.len().div_ceil(FRAME))
        .map(|i| Ok::<_, std::io::Error>(body.slice(i * FRAME..((i + 1) * FRAME).min(body.len()))))
        .collect();
    let mut request = Request::builder().method(path.method).uri(path.uri).header(header::ACCEPT, path.accept);
    if !path.content_type.is_empty() {
        request = request.header(header::CONTENT_TYPE, path.content_type);
    }
    // No body is no body: hyper gives a `GET` an empty one.
    let body = if frames.is_empty() { Body::empty() } else { Body::from_stream(futures_util::stream::iter(frames)) };
    request.body(body).unwrap_or_default()
}

/// Serves one request and reads its whole response. Its status and the frames of its body.
pub async fn serve(router: &Router, path: &Path) -> (u16, usize) {
    let Ok(resp) = router.clone().oneshot(request(path)).await;
    let status = resp.status().as_u16();
    let mut body = resp.into_body();
    let mut frames = 0;
    while let Some(Ok(frame)) = body.frame().await {
        std::hint::black_box(frame);
        frames += 1;
    }
    (status, frames)
}

/// What a side answers in full: its status, its `Content-Type` and its body.
#[allow(dead_code, reason = "the roundtrip bench compares what the clients end with")]
async fn answer(router: &Router, path: &Path) -> (u16, String, Vec<u8>) {
    let Ok(resp) = router.clone().oneshot(request(path)).await;
    let status = resp.status().as_u16();
    let content_type = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_owned();
    let mut body = resp.into_body();
    let mut bytes = Vec::new();
    while let Some(Ok(frame)) = body.frame().await {
        if let Ok(data) = frame.into_data() {
            bytes.extend_from_slice(&data);
        }
    }
    (status, content_type, bytes)
}

/// Both sides succeed with the same answer: the same status, `Content-Type` and body bytes. Otherwise their costs
/// compare different work.
#[allow(dead_code, reason = "the roundtrip bench compares what the clients end with")]
pub async fn same_answer(generated: &Router, baseline: &Router, path: &Path) -> Result<(), String> {
    let (ours, theirs) = (answer(generated, path).await, answer(baseline, path).await);
    let at = format!("{} at {}", path.name, table::size(size()));
    if ours.0 >= 300 || theirs.0 >= 300 {
        return Err(format!("{at}: answered {}, its baseline {}", ours.0, theirs.0));
    }
    if ours.0 != theirs.0 || ours.1 != theirs.1 {
        return Err(format!("{at}: answered {} {:?}, its baseline {} {:?}", ours.0, ours.1, theirs.0, theirs.1));
    }
    if ours.2 != theirs.2 {
        let first = ours.2.iter().zip(&theirs.2).take_while(|(a, b)| a == b).count();
        let (a, b) = (ours.2.len(), theirs.2.len());
        return Err(format!("{at}: bodies differ from byte {first} ({a} bytes, its baseline {b})"));
    }
    Ok(())
}

// The allocations of this thread, the bytes it holds and the most it held. Requests are served on one thread.
thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

/// Whether allocations are counted: only while [`heap`] measures one request. Timed rounds pay one atomic load per
/// allocation and no thread-local, so counting does not tilt the time of the side that allocates more.
static COUNTING: AtomicBool = AtomicBool::new(false);

fn held(change: isize, allocation: bool) {
    if !COUNTING.load(Ordering::Relaxed) {
        return;
    }
    let _ = LIVE.try_with(|live| {
        let now = live.get() + change;
        live.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
    if allocation {
        let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
    }
}

fn signed(n: usize) -> isize {
    isize::try_from(n).unwrap_or(isize::MAX)
}

struct Counting;

// SAFETY: every call goes to `System` as it is.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        held(signed(layout.size()), true);
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc`.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        held(-signed(layout.size()), false);
        // SAFETY: the caller keeps the contract of `GlobalAlloc::dealloc`.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        held(signed(layout.size()), true);
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc_zeroed`.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        held(signed(new_size) - signed(layout.size()), true);
        // SAFETY: the caller keeps the contract of `GlobalAlloc::realloc`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Tries of one request whose allocations and heap count: the fewest of them. A first request also builds what is
/// kept for the next.
const TRIES: usize = 3;

/// The allocations and the peak live heap in KiB of one request: the fewest of a few.
#[allow(dead_code, reason = "the time bench does not count allocations")]
pub async fn heap(router: &Router, path: &Path) -> (u64, isize) {
    heap_of(async || {
        serve(router, path).await;
    })
    .await
}

/// [`heap`] of any call.
#[allow(dead_code, reason = "the time bench does not count allocations")]
pub async fn heap_of(once: impl AsyncFn()) -> (u64, isize) {
    let (mut allocs, mut peak) = (u64::MAX, isize::MAX);
    for _ in 0..TRIES {
        let before = ALLOCS.with(Cell::get);
        let base = LIVE.with(Cell::get);
        PEAK.with(|p| p.set(base));
        COUNTING.store(true, Ordering::Relaxed);
        once().await;
        COUNTING.store(false, Ordering::Relaxed);
        allocs = allocs.min(ALLOCS.with(Cell::get) - before);
        peak = peak.min(PEAK.with(Cell::get) - base);
    }
    (allocs, peak >> 10)
}

/// The time of one call of `once` in a round of `n`.
async fn round(once: &impl AsyncFn(), n: u32) -> Duration {
    let started = Instant::now();
    for _ in 0..n {
        once().await;
    }
    started.elapsed() / n
}

/// What [`times`] measured: each side's median time of one request and the median ratio of the two.
#[allow(dead_code, reason = "the allocation bench does not time")]
#[derive(Clone, Copy)]
pub struct Timing {
    pub generated: Duration,
    pub baseline: Duration,
    /// How many times the time of its baseline the generated path takes.
    pub ratio: f64,
}

/// Times `rounds` pairs of rounds: one of the generated side, then one of its baseline. A round takes about `length`
/// for the baseline. The ratio is the median of the ratios of each pair: a stall of the machine slows the two rounds of
/// a pair alike or one pair only, so it barely moves the median. A ratio of the two fastest rounds, each from a pair of
/// its own, took one lucky round of a side as the whole truth and failed now and then on a busy CI runner.
#[allow(dead_code, reason = "the allocation bench does not time")]
pub async fn times(generated: &Router, baseline: &Router, path: &Path, rounds: usize, length: Duration) -> Timing {
    let head = async || {
        std::hint::black_box(serve(generated, path).await);
    };
    let base = async || {
        std::hint::black_box(serve(baseline, path).await);
    };
    times_of(&head, &base, rounds, length).await
}

/// [`times`] of any two calls: `head`, the generated side, and `base`, its baseline.
#[allow(dead_code, reason = "the allocation bench does not time")]
pub async fn times_of(head: &impl AsyncFn(), base: &impl AsyncFn(), rounds: usize, length: Duration) -> Timing {
    let once = round(base, 1).await.max(Duration::from_nanos(1));
    let n = u32::try_from(length.as_nanos() / once.as_nanos()).unwrap_or(1).clamp(1, 5000);
    let mut pairs = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        pairs.push((round(head, n).await, round(base, n).await));
    }
    let median = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        values.get(values.len() / 2).copied().unwrap_or_default()
    };
    let seconds =
        |side: fn(&(Duration, Duration)) -> Duration| median(pairs.iter().map(|p| side(p).as_secs_f64()).collect());
    Timing {
        generated: Duration::from_secs_f64(seconds(|p| p.0)),
        baseline: Duration::from_secs_f64(seconds(|p| p.1)),
        ratio: median(pairs.iter().map(|(g, b)| g.as_secs_f64() / b.as_secs_f64().max(f64::MIN_POSITIVE)).collect()),
    }
}

/// The runtime that requests are served on. One thread: the allocations that it counts are the request's.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a tokio runtime")
}

/// The generated servers and their baselines with [`SIZE`] set to `size`.
pub fn at(size: usize) -> (Router, Router) {
    SIZE.store(size, Ordering::Relaxed);
    (generated(), baseline())
}
