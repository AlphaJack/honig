//! A binary response streams chunk by chunk when the handler returns
//! a streamed `api::Blob`. So a large download never sits in memory whole. A manifest carries an `ETag`. The
//! server answers a `GET` that the client already has with `304`. It refuses a stale replace with `412`. A ranged
//! `GET` of an export whose length is known gets only those bytes with `206`. So a broken download resumes. An export
//! from `Blob::ranged` is opened at the range.

mod support;

use assert_matches::assert_matches;
use std::future::Future;
use std::sync::Mutex;

use e2e::v32_download::api::{self, Client, operations as ops};

const CHUNKS: [&[u8]; 3] = [b"PK\x03\x04", b"\x00\xff", b"end"];

/// The first bytes of a PNG file.
const PNG: &[u8] = b"\x89PNG\r\n";

/// The export `archive`: a file whose length is known.
const ARCHIVE: &[u8] = b"PK\x03\x04 orders.csv customers.csv";

/// The export server. The manifest of export `small` changes version with each replace.
#[derive(Default)]
struct Server {
    manifest: Mutex<(u32, Vec<String>)>,
}

/// When every manifest was last modified.
const LAST_MODIFIED: &str = "Sun, 06 Nov 2026 08:49:37 GMT";

impl api::Api for Server {
    // ANCHOR: download
    fn get_export(
        &self,
        p: ops::get_export::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::get_export::Response, api::ApiError>> + Send {
        let r = match p.id.as_str() {
            "big" => {
                ops::get_export::Response::Ok(api::Blob::stream(futures_util::stream::iter(CHUNKS.map(<[u8]>::to_vec))))
            }
            "small" => ops::get_export::Response::Ok(b"zip".to_vec().into()),
            // A file opened where the range starts. The bytes before it are never loaded.
            "archive" => ops::get_export::Response::Ok(api::Blob::ranged(ARCHIVE.len() as u64, |first, last| {
                let (first, last) = (usize::try_from(first).unwrap_or(0), usize::try_from(last).unwrap_or(0));
                futures_util::stream::iter(ARCHIVE.get(first..=last).map(<[u8]>::to_vec))
            })),
            _ => ops::get_export::Response::NotFound,
        };
        std::future::ready(Ok(r))
    }
    // ANCHOR_END: download

    // ANCHOR: media-range
    fn get_thumbnail(
        &self,
        p: ops::get_thumbnail::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::get_thumbnail::Response, api::ApiError>> + Send {
        // `image/*` declares a range: the blob says which image it is.
        let preview = api::Blob::from(PNG.to_vec());
        let preview = match p.id.as_str() {
            "png" => preview.with_media("image/png"),
            "zip" => preview.with_media("application/zip"),
            _ => preview,
        };
        std::future::ready(Ok(ops::get_thumbnail::Response::Ok(preview)))
    }
    // ANCHOR_END: media-range

    fn get_manifest(
        &self,
        p: ops::get_manifest::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::get_manifest::Response, api::ApiError>> + Send {
        let found = (p.id == "small").then(|| self.manifest.lock().map(|m| m.clone()));
        std::future::ready(match found {
            Some(Ok((version, files))) => Ok(ops::get_manifest::Response::Ok {
                body: files,
                e_tag: Some(format!("\"v{version}\"")),
                last_modified: Some(LAST_MODIFIED.to_owned()),
            }),
            Some(Err(_)) => {
                Err(api::ApiError::new(api::http::StatusCode::INTERNAL_SERVER_ERROR, "manifest lock poisoned"))
            }
            None => Ok(ops::get_manifest::Response::NotFound),
        })
    }

    // ANCHOR: preconditions
    async fn replace_manifest(
        &self,
        p: ops::replace_manifest::Params,
        ctx: &api::Context<'_>,
    ) -> Result<ops::replace_manifest::Response, api::ApiError> {
        let mut manifest = self
            .manifest
            .lock()
            .map_err(|_| api::ApiError::new(api::http::StatusCode::INTERNAL_SERVER_ERROR, "manifest lock poisoned"))?;
        let etag = format!("\"v{}\"", manifest.0);
        // 412 unless the client changed the version it received.
        ctx.preconditions(Some(api::CacheValidators { etag: Some(&etag), last_modified: Some(LAST_MODIFIED) }))?;
        *manifest = (manifest.0 + 1, p.body);
        Ok(ops::replace_manifest::Response::NoContent)
    }
    // ANCHOR_END: preconditions
}

/// A response the in-process server is still producing.
type Pending = std::pin::Pin<Box<dyn Future<Output = api::Response> + Send>>;

/// The server in process. Each request that it handles sees the same manifest.
fn server() -> impl Fn(api::Request) -> Pending + Send + Sync {
    let service = api::Server::new(Server::default()).into_service();
    move |req| service.handle(req)
}

fn client() -> Client<impl api::Transport> {
    Client::new(support::loopback(server()))
}

#[test]
fn the_server_streams_a_blob_without_buffering_it() {
    let req = api::runtime::new_request("GET", "/exports/big".to_owned(), vec![]).unwrap();
    let r = api::block_on(api::Server::new(Server::default()).into_service().handle(req));
    assert_eq!(r.status().as_u16(), 200);
    assert_eq!(r.headers().get("content-type").and_then(|v| v.to_str().ok()), Some("application/zip"));
    assert!(r.body().bytes().is_empty() && r.body().is_streamed(), "the chunks are still to come");
    assert_eq!(api::block_on(r.into_body().collect()).unwrap(), CHUNKS.concat());
}

#[test]
fn the_client_reads_a_blob_whole_or_as_chunks() {
    let c = client();
    let whole = api::block_on(c.get_export(ops::get_export::Params { id: "big".to_owned() })).unwrap();
    assert_eq!(whole, ops::get_export::Response::Ok(api::Blob::from(CHUNKS.concat())));
    let small = api::block_on(c.get_export(ops::get_export::Params { id: "small".to_owned() })).unwrap();
    assert_eq!(small, ops::get_export::Response::Ok(b"zip".to_vec().into()));
    let none = api::block_on(c.get_export(ops::get_export::Params { id: "none".to_owned() }));
    assert_eq!(none.ok(), Some(ops::get_export::Response::NotFound), "the declared 404, whole");
    let stream = api::block_on(c.get_export_stream(ops::get_export::Params { id: "big".to_owned() })).unwrap();
    let api::Streamed::Ok(resp) = stream else { panic!("200 streams: {stream:?}") };
    assert_eq!(api::block_on(resp.into_body().collect()).unwrap(), CHUNKS.concat());
    // The declared 404 arrives typed as from `get_export`.
    let missing = api::block_on(c.get_export_stream(ops::get_export::Params { id: "none".to_owned() }));
    assert_matches!(missing, Ok(api::Streamed::Whole(ops::get_export::Response::NotFound)), "{missing:?}");
}

/// A `GET` of the manifest with these request headers through the server.
fn get_manifest(server: &impl Fn(api::Request) -> Pending, headers: &[(&'static str, &'static str)]) -> api::Response {
    let mut req = api::runtime::new_request("GET", "/exports/small/manifest".to_owned(), vec![]).unwrap();
    for (k, v) in headers {
        req.headers_mut().append(
            honig::http::HeaderName::try_from(*k).unwrap(),
            honig::http::HeaderValue::try_from((*v).to_owned()).unwrap(),
        );
    }
    api::block_on(server(req))
}

#[test]
fn a_get_the_client_already_holds_is_answered_304_without_a_body() {
    let server = server();
    let fresh = get_manifest(&server, &[]);
    assert_eq!(
        (fresh.status().as_u16(), fresh.headers().get("etag").and_then(|v| v.to_str().ok())),
        (200, Some(r#""v0""#))
    );
    let held = get_manifest(&server, &[("If-None-Match", r#"W/"v0""#)]);
    assert_eq!(held.status().as_u16(), 304);
    assert_eq!(
        (held.headers().get("content-type").map_or("", |v| v.to_str().unwrap_or("")), held.body().bytes()),
        ("", &b""[..])
    );
    assert_eq!(held.headers().get("etag").and_then(|v| v.to_str().ok()), Some(r#""v0""#), "a 304 keeps the validators");
    assert_eq!(get_manifest(&server, &[("If-None-Match", r#""v9""#)]).status().as_u16(), 200);
    assert_eq!(get_manifest(&server, &[("If-Modified-Since", LAST_MODIFIED)]).status().as_u16(), 304);
    assert_eq!(get_manifest(&server, &[("If-Modified-Since", "Sat, 05 Nov 2026 08:49:37 GMT")]).status().as_u16(), 200);
}

#[test]
fn a_replace_of_a_version_the_client_did_not_read_is_refused_412() {
    let c = client();
    let replace = |if_match: &str| {
        let p = ops::replace_manifest::Params {
            if_match: Some(if_match.to_owned()),
            ..ops::replace_manifest::Params::new("small".to_owned(), vec!["a.csv".to_owned()])
        };
        api::block_on(c.replace_manifest(p)).unwrap()
    };
    assert_eq!(replace(r#""v0""#), ops::replace_manifest::Response::NoContent);
    assert_eq!(replace(r#""v0""#), ops::replace_manifest::Response::PreconditionFailed, "v0 is gone");
    assert_eq!(replace(r#"W/"v1""#), ops::replace_manifest::Response::PreconditionFailed, "If-Match is strong");
    assert_eq!(replace("*"), ops::replace_manifest::Response::NoContent);
}

/// A `GET` of an export with these request headers through the server. Its streamed body is collected.
fn get_export(id: &str, headers: &[(&'static str, &'static str)]) -> api::Response {
    let mut req = api::runtime::new_request("GET", format!("/exports/{id}"), vec![]).unwrap();
    for (k, v) in headers {
        req.headers_mut().append(
            honig::http::HeaderName::try_from(*k).unwrap(),
            honig::http::HeaderValue::try_from((*v).to_owned()).unwrap(),
        );
    }
    let (head, body) = api::block_on(server()(req)).into_parts();
    api::Response::from_parts(head, api::Body::new(api::block_on(body.collect()).unwrap()))
}

#[test]
fn a_ranged_get_of_a_binary_body_gets_those_bytes_alone() {
    let whole = get_export("small", &[]);
    assert_eq!(
        (whole.status().as_u16(), whole.headers().get("accept-ranges").and_then(|v| v.to_str().ok())),
        (200, Some("bytes"))
    );
    let part = get_export("small", &[("Range", "bytes=1-")]);
    assert_eq!((part.status().as_u16(), part.body().bytes()), (206, &b"ip"[..]));
    assert_eq!(part.headers().get("content-range").and_then(|v| v.to_str().ok()), Some("bytes 1-2/3"));
    assert_eq!(part.headers().get("content-type").and_then(|v| v.to_str().ok()), Some("application/zip"));
    let past = get_export("small", &[("Range", "bytes=3-")]);
    assert_eq!(
        (past.status().as_u16(), past.headers().get("content-range").and_then(|v| v.to_str().ok())),
        (416, Some("bytes */3"))
    );
    // A file of known length is opened at the range.
    let resumed = get_export("archive", &[("Range", "bytes=16-")]);
    assert_eq!((resumed.status().as_u16(), resumed.body().bytes()), (206, &b"customers.csv"[..]));
    assert_eq!(resumed.headers().get("content-range").and_then(|v| v.to_str().ok()), Some("bytes 16-28/29"));
    let archive = get_export("archive", &[]);
    assert_eq!(
        (
            archive.status().as_u16(),
            archive.body().bytes(),
            archive.headers().get("accept-ranges").and_then(|v| v.to_str().ok())
        ),
        (200, ARCHIVE, Some("bytes"))
    );
    // A stream of unknown length is sent whole and says nothing of ranges.
    let streamed = get_export("big", &[("Range", "bytes=1-")]);
    let whole = CHUNKS.concat();
    assert_eq!(
        (
            streamed.status().as_u16(),
            streamed.body().bytes(),
            streamed.headers().get("accept-ranges").and_then(|v| v.to_str().ok())
        ),
        (200, whole.as_slice(), None)
    );
    // JSON is not cut whatever the client asks.
    let manifest = get_manifest(&server(), &[("Range", "bytes=0-1")]);
    assert_eq!(
        (manifest.status().as_u16(), manifest.headers().get("accept-ranges").and_then(|v| v.to_str().ok())),
        (200, None)
    );
}

mod malformed;

static FIXTURE: malformed::Fixture = malformed::Fixture {
    routes: &[
        (ops::get_export::METHOD, ops::get_export::PATH),
        (ops::get_manifest::METHOD, ops::get_manifest::PATH),
        (ops::replace_manifest::METHOD, ops::replace_manifest::PATH),
    ],
    segments: &["small", "big"],
    query_names: &[],
    header_names: &[],
};

proptest::proptest! {
    #![proptest_config(malformed::config())]

    #[test]
    fn malformed_requests_get_a_client_error(case in malformed::case(&FIXTURE)) {
        malformed::check(api::Server::new(Server::default()).into_service(), &case)?;
    }
}

/// A body declared as a range (`image/*`) goes out as the media type of the blob when it fits the range, else as
/// `application/octet-stream`. The client gets the blob back with the type it came as.
#[test]
fn a_range_goes_out_as_the_media_type_of_the_blob() {
    let sent = |id: &str| {
        let req = api::runtime::new_request("GET", format!("/exports/{id}/thumbnail"), vec![]).unwrap();
        api::block_on(server()(req)).headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_owned)
    };
    assert_eq!(sent("png").as_deref(), Some("image/png"));
    assert_eq!(sent("zip").as_deref(), Some("application/octet-stream"), "not an image");
    assert_eq!(sent("plain").as_deref(), Some("application/octet-stream"), "no type");
    let got = api::block_on(client().get_thumbnail(ops::get_thumbnail::Params { id: "png".to_owned() })).unwrap();
    let ops::get_thumbnail::Response::Ok(blob) = got;
    assert_eq!(blob.media(), Some("image/png"));
}
