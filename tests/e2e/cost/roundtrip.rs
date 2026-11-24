//! The cost of a roundtrip: business code calls a client, the request crosses a server to its handler and the answer
//! comes back decoded into a value. The generated client over the generated server (`head`) against a hand-rolled
//! client over the hand-rolled baseline (`base`). Each client sends through the axum router of its server, in the
//! process: no socket adds noise. The values that the clients send are built once, before any call, as business code
//! keeps them.

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use e2e::{
    v32_download as dl, v32_multipart as mp, v32_sse_items as sse, v32_uploads as up, v32_validation as val,
    v32_xml as xml,
};

use super::cost;

/// What a client ends with, the same for both sides. Only [`same_answer`] compares it.
#[derive(Debug, PartialEq)]
pub enum Seen {
    /// A status without a body.
    Done,
    /// The bytes and chunks that an upload counted.
    Counted(i64, i64),
    Bytes(Vec<u8>),
    Names(Vec<String>),
    Tiles(Vec<(String, i64)>),
    /// The id, title and authors of a book.
    Book(i64, String, Vec<(String, Option<i64>)>),
}

/// One kind of roundtrip and its budgets against its baseline, set as `cost::Path` sets them.
pub struct Trip {
    pub name: &'static str,
    /// At most how many times the allocations of the baseline.
    #[allow(dead_code, reason = "only the allocation bench uses it")]
    pub max_allocs: f64,
    /// At most how many times the time of the baseline in a release build.
    #[allow(dead_code, reason = "only the time bench uses it")]
    pub max_time: f64,
}

const fn trip(name: &'static str, (max_allocs, max_time): (f64, f64)) -> Trip {
    Trip { name, max_allocs, max_time }
}

/// Every kind of roundtrip: the kinds of path of `cost::paths`.
pub const TRIPS: [Trip; 9] = [
    trip("upload", (1.45, 2.5)),
    trip("multipart_b64", (2.0, 2.4)),
    trip("download", (1.6, 2.85)),
    trip("json_get", (1.3, 1.9)),
    trip("json_put", (1.35, 1.75)),
    trip("sse", (1.25, 1.45)),
    trip("jsonl", (1.1, 2.2)),
    trip("xml", (0.7, 1.2)),
    trip("form", (1.8, 2.75)),
];

type Over = honig::TowerTransport<Router>;

/// The generated clients, each over the generated router under the prefix of its fixture.
pub struct Head {
    up: up::api::Client<Over>,
    mp: mp::api::Client<Over>,
    dl: dl::api::Client<Over>,
    sse: sse::api::Client<Over>,
    xml: xml::api::Client<Over>,
    val: val::api::Client<Over>,
    put_file: up::api::operations::put_file::Params,
    upload_scan: mp::api::operations::upload_scan::Params,
    add_book: xml::api::operations::add_book::Params,
    add_member: val::api::operations::add_member::Params,
    replace_manifest: dl::api::operations::replace_manifest::Params,
}

/// The hand-rolled client: the baseline router and the values that it sends.
pub struct Base {
    router: Router,
    upload: Bytes,
    scan: Vec<u8>,
    book: cost::Book,
    member: Member,
    manifest: Vec<String>,
}

#[derive(serde::Serialize)]
struct Member {
    name: String,
    age: i64,
    role: &'static str,
}

#[derive(serde::Deserialize)]
struct Counted {
    bytes: i64,
    chunks: i64,
}

#[derive(serde::Deserialize)]
struct Tile {
    tile: String,
    rows: i64,
}

fn authors(size: usize) -> impl Iterator<Item = (String, i64)> {
    (0..(size / cost::AUTHOR).max(1)).map(|i| (format!("Author {i}"), 1900 + i64::try_from(i % 100).unwrap_or(0)))
}

/// Both clients with [`cost::SIZE`] set to `size`.
pub fn at(size: usize) -> (Head, Base) {
    let (generated, baseline) = cost::at(size);
    let over = |prefix: &str| honig::TowerTransport::new(generated.clone(), prefix).expect("a base URL");
    let raw = cost::upload_body(size);
    let book = xml::api::types::Book::new(
        1,
        "Dune".to_owned(),
        authors(size).map(|(name, born)| xml::api::types::Author { name, born: Some(born) }).collect(),
    );
    let user = val::api::types::User {
        name: "Alice".try_into().expect("a valid name"),
        handle: None,
        age: Some(30.try_into().expect("a valid age")),
        score: None,
        tags: None,
        role: val::api::types::UserRole::Admin,
        email: None,
        site: None,
        meta: None,
        loyalty: None,
        note: val::api::runtime::Nullable::Absent,
    };
    let head = Head {
        up: up::api::Client::new(over("/up")),
        mp: mp::api::Client::new(over("/mp")),
        dl: dl::api::Client::new(over("/dl")),
        sse: sse::api::Client::new(over("/sse")),
        xml: xml::api::Client::new(over("/xml")),
        val: val::api::Client::new(over("/val")),
        put_file: up::api::operations::put_file::Params::new("a".to_owned(), raw.clone().into()),
        upload_scan: mp::api::operations::upload_scan::Params::new(mp::api::types::UploadScanBody {
            label: None,
            scan: raw.clone().into(),
        }),
        add_book: xml::api::operations::add_book::Params {
            body: book,
            media: xml::api::operations::add_book::Media::Xml,
        },
        replace_manifest: dl::api::operations::replace_manifest::Params::new("x".to_owned(), cost::manifest()),
        add_member: val::api::operations::add_member::Params::new(
            val::api::operations::add_member::Body::XWwwFormUrlencoded(user),
        ),
    };
    let base = Base {
        router: baseline,
        upload: raw.clone(),
        scan: raw.to_vec(),
        book: cost::Book {
            namespace: "https://example.com/books".to_owned(),
            id: 1,
            title: "Dune".to_owned(),
            authors: cost::Authors {
                author: authors(size).map(|(name, born)| cost::Author { name, born: Some(born) }).collect(),
            },
        },
        member: Member { name: "Alice".to_owned(), age: 30, role: "admin" },
        manifest: cost::manifest(),
    };
    (head, base)
}

fn book(id: i64, title: String, authors: impl Iterator<Item = (String, Option<i64>)>) -> Seen {
    Seen::Book(id, title, authors.collect())
}

/// One roundtrip of the generated client.
pub async fn head(c: &Head, trip: &str) -> Result<Seen, String> {
    let failed = |e: honig::ClientError| e.to_string();
    Ok(match trip {
        "upload" => match c.up.put_file(&c.put_file).await.map_err(failed)? {
            up::api::operations::put_file::Response::Ok(ok) => Seen::Counted(ok.bytes, ok.chunks),
        },
        "multipart_b64" => match c.mp.upload_scan(&c.upload_scan).await.map_err(failed)? {
            mp::api::operations::upload_scan::Response::NoContent => Seen::Done,
        },
        "download" => {
            let params = dl::api::operations::get_export::Params { id: "x".to_owned() };
            match c.dl.get_export(params).await.map_err(failed)? {
                dl::api::operations::get_export::Response::Ok(blob) => {
                    Seen::Bytes(blob.collect().await.map_err(failed)?)
                }
                dl::api::operations::get_export::Response::NotFound => return Err("not found".to_owned()),
            }
        }
        "json_get" => {
            let params = dl::api::operations::get_manifest::Params::new("x".to_owned());
            match c.dl.get_manifest(params).await.map_err(failed)? {
                dl::api::operations::get_manifest::Response::Ok { body, .. } => Seen::Names(body),
                dl::api::operations::get_manifest::Response::NotFound => return Err("not found".to_owned()),
            }
        }
        "json_put" => match c.dl.replace_manifest(&c.replace_manifest).await.map_err(failed)? {
            dl::api::operations::replace_manifest::Response::NoContent => Seen::Done,
            dl::api::operations::replace_manifest::Response::PreconditionFailed => {
                return Err("precondition failed".to_owned());
            }
        },
        "sse" => match c.sse.tile_stream(sse::api::operations::tile_stream::Params {}).await.map_err(failed)? {
            sse::api::operations::tile_stream::Response::Ok(items) => {
                Seen::Tiles(items.collect().await.map_err(failed)?.into_iter().map(|t| (t.tile, t.rows)).collect())
            }
        },
        "jsonl" => match c.sse.tile_lines(sse::api::operations::tile_lines::Params {}).await.map_err(failed)? {
            sse::api::operations::tile_lines::Response::Ok(items) => {
                Seen::Tiles(items.collect().await.map_err(failed)?.into_iter().map(|t| (t.tile, t.rows)).collect())
            }
        },
        "xml" => match c.xml.add_book(&c.add_book).await.map_err(failed)? {
            xml::api::operations::add_book::Response::Created(b) => {
                book(b.id, b.title, b.authors.into_iter().map(|a| (a.name, a.born)))
            }
        },
        "form" => match c.val.add_member(&c.add_member).await.map_err(failed)? {
            val::api::operations::add_member::Response::Created => Seen::Done,
        },
        other => return Err(format!("no roundtrip {other}")),
    })
}

/// Sends `request` to the baseline router and collects its whole body. Any status but `want` fails.
async fn call(router: &Router, request: Request<Body>, want: u16) -> Result<Bytes, String> {
    let Ok(resp) = router.clone().oneshot(request).await;
    let status = resp.status().as_u16();
    let body = resp.into_body().collect().await.map_err(|e| e.to_string())?.to_bytes();
    if status == want { Ok(body) } else { Err(format!("answered {status}")) }
}

fn get(uri: &str, accept: &str) -> Request<Body> {
    Request::get(uri).header(header::ACCEPT, accept).body(Body::empty()).unwrap_or_default()
}

fn post(method: &str, uri: &str, content_type: &str, body: impl Into<Body>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type)
        .body(body.into())
        .unwrap_or_default()
}

fn json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|e| e.to_string())
}

fn tiles<'a>(records: impl Iterator<Item = &'a [u8]>) -> Result<Seen, String> {
    let mut tiles = vec![];
    for record in records {
        let t: Tile = serde_json::from_slice(record).map_err(|e| e.to_string())?;
        tiles.push((t.tile, t.rows));
    }
    Ok(Seen::Tiles(tiles))
}

/// One roundtrip of the hand-rolled client: serde and `http` requests, as a client written without a generator.
pub async fn base(c: &Base, trip: &str) -> Result<Seen, String> {
    let r = &c.router;
    Ok(match trip {
        "upload" => {
            let body = call(r, post("PUT", "/up/files/a", "application/octet-stream", c.upload.clone()), 200).await?;
            let counted: Counted = json(&body)?;
            Seen::Counted(counted.bytes, counted.chunks)
        }
        "multipart_b64" => {
            call(r, post("POST", "/mp/scans", cost::SCAN_TYPE, cost::scan_form(&c.scan)), 204).await?;
            Seen::Done
        }
        "download" => Seen::Bytes(call(r, get("/dl/exports/x", "application/zip"), 200).await?.to_vec()),
        "json_get" => Seen::Names(json(&call(r, get("/dl/exports/x/manifest", "application/json"), 200).await?)?),
        "json_put" => {
            let body = serde_json::to_vec(&c.manifest).map_err(|e| e.to_string())?;
            call(r, post("PUT", "/dl/exports/x/manifest", "application/json", body), 204).await?;
            Seen::Done
        }
        "sse" => {
            let body = call(r, get("/sse/tiles/stream", "text/event-stream"), 200).await?;
            tiles(body.split(|&b| b == b'\n').filter_map(|line| line.strip_prefix(b"data: ")))?
        }
        "jsonl" => {
            let body = call(r, get("/sse/tiles/lines", "application/jsonl"), 200).await?;
            tiles(body.split(|&b| b == b'\n').filter(|line| !line.is_empty()))?
        }
        "xml" => {
            let xml = quick_xml::se::to_string(&c.book).map_err(|e| e.to_string())?;
            let sent = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{xml}");
            let body = call(r, post("POST", "/xml/books", "application/xml", sent), 201).await?;
            let text = std::str::from_utf8(&body).map_err(|e| e.to_string())?;
            let b: cost::Book = quick_xml::de::from_str(text).map_err(|e| e.to_string())?;
            book(b.id, b.title, b.authors.author.into_iter().map(|a| (a.name, a.born)))
        }
        "form" => {
            let form = serde_urlencoded::to_string(&c.member).map_err(|e| e.to_string())?;
            call(r, post("POST", "/val/members", "application/x-www-form-urlencoded", form), 201).await?;
            Seen::Done
        }
        other => return Err(format!("no roundtrip {other}")),
    })
}

/// Both clients end with the same value. Otherwise their costs compare different work.
pub async fn same_answer(h: &Head, b: &Base, trip: &str) -> Result<(), String> {
    let at = format!("{trip} at {}", bench_support::table::size(cost::SIZE.load(std::sync::atomic::Ordering::Relaxed)));
    let (ours, theirs) = (head(h, trip).await, base(b, trip).await);
    match (ours, theirs) {
        (Ok(ours), Ok(theirs)) if ours == theirs => Ok(()),
        (Ok(_), Ok(_)) => Err(format!("{at}: the clients ended with different values")),
        (ours, theirs) => Err(format!("{at}: {:?}, its baseline {:?}", ours.err(), theirs.err())),
    }
}
