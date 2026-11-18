//! Hand-written checks for the `multipart` fixture. They run against the module that `tests/regen.rs` keeps fresh.

mod support;

use std::future::Future;
use std::sync::{Arc, Mutex};

use e2e::v32_multipart::api::{self, Client, operations as ops, types};

/// Keeps the params that the server decoded per operation.
#[derive(Default)]
struct Store {
    upload: Mutex<Option<ops::upload::Params>>,
    icon: Mutex<Option<ops::put_icon::Params>>,
    scan: Mutex<Option<ops::upload_scan::Params>>,
    deploy: Mutex<Option<ops::deploy::Params>>,
}

impl api::Api for Store {
    type Principal = ();

    fn upload(
        &self,
        p: ops::upload::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::upload::Response, api::ApiError>> + Send {
        *self.upload.lock().unwrap() = Some(p);
        std::future::ready(Ok(ops::upload::Response::Created))
    }
    fn deploy(
        &self,
        p: ops::deploy::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::deploy::Response, api::ApiError>> + Send {
        *self.deploy.lock().unwrap() = Some(p);
        std::future::ready(Ok(ops::deploy::Response::NoContent))
    }
    fn upload_scan(
        &self,
        p: ops::upload_scan::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::upload_scan::Response, api::ApiError>> + Send {
        *self.scan.lock().unwrap() = Some(p);
        std::future::ready(Ok(ops::upload_scan::Response::NoContent))
    }
    fn put_icon(
        &self,
        p: ops::put_icon::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::put_icon::Response, api::ApiError>> + Send {
        *self.icon.lock().unwrap() = Some(p);
        std::future::ready(Ok(ops::put_icon::Response::NoContent))
    }
}

/// A client whose requests reach `store` through the server.
fn served(store: &Arc<Store>) -> Client<impl api::Transport> {
    let service = api::Server::new(Arc::clone(store)).into_service();
    Client::new(service)
}

/// A part chosen as the second listed type (`image/jpeg`) reaches the handler as the bytes sent.
#[test]
fn the_server_reads_a_part_sent_as_a_listed_type() {
    let store = Arc::new(Store::default());
    let body = types::PutIconBody { name: Some("logo".to_owned()), icon: api::Binary::from(vec![0xFF, 0xD8, 0xFF]) };
    let sent = ops::put_icon::Params {
        part_types: api::PartTypes::from([("icon", "image/jpeg")]),
        ..ops::put_icon::Params::new(body.clone())
    };
    api::block_on(served(&store).put_icon(sent)).unwrap();
    let got = store.icon.lock().unwrap().take().expect("handler ran");
    assert_eq!(got.body, body);
}

/// The server decodes a base64-encoded part (see `a_base64_field_travels_transfer_encoded`) back to its bytes.
#[test]
fn a_base64_part_arrives_decoded() {
    let scan = types::UploadScanBody { label: Some("page 1".to_owned()), scan: api::Binary::from(vec![0, 1, 2, 0xFF]) };
    let store = Arc::new(Store::default());
    api::block_on(served(&store).upload_scan(ops::upload_scan::Params::new(scan.clone()))).unwrap();
    assert_eq!(store.scan.lock().unwrap().take().map(|p| p.body), Some(scan));
}

/// A schemaless `multipart/form-data` body. Each part is a field of the map. Also no body at all.
#[test]
fn a_schemaless_form_reaches_the_handler_as_a_map() {
    let store = Arc::new(Store::default());
    let fields: std::collections::BTreeMap<String, serde_json::Value> =
        [("env".to_owned(), serde_json::json!("prod")), ("note".to_owned(), serde_json::json!("a b"))].into();
    let sent = ops::deploy::Params { body: Some(fields) };
    api::block_on(served(&store).deploy(sent.clone())).unwrap();
    assert_eq!(store.deploy.lock().unwrap().take(), Some(sent));
    api::block_on(served(&store).deploy(ops::deploy::Params::new())).unwrap();
    assert_eq!(store.deploy.lock().unwrap().take(), Some(ops::deploy::Params::new()));
}

/// `icon: {}` with `contentType: image/png, image/jpeg` is bytes. It is sent as the first type
/// unless `part_types` picks another listed one. An unlisted one is refused.
#[test]
fn a_part_goes_out_as_the_listed_type_the_caller_picks() {
    let sent: Arc<Mutex<Vec<u8>>> = Arc::default();
    let keep = sent.clone();
    let client = Client::new(support::loopback(move |req: api::Request| {
        *keep.lock().unwrap() = req.body().bytes().to_vec();
        async { api::runtime::new_response(204) }
    }));
    let body = types::PutIconBody { name: None, icon: api::Binary::from(vec![0xFF, 0xD8, 0xFF]) };
    let send =
        |p: ops::put_icon::Params| api::block_on(client.put_icon(p)).map(|_| part_head(&sent.lock().unwrap(), "icon"));
    let first = send(ops::put_icon::Params::new(body.clone())).unwrap();
    assert!(first.contains("Content-Type: image/png"), "{first}");
    let jpeg = ops::put_icon::Params {
        part_types: api::PartTypes::from([("icon", "image/jpeg")]),
        ..ops::put_icon::Params::new(body.clone())
    };
    let chosen = send(jpeg).unwrap();
    assert!(chosen.contains("Content-Type: image/jpeg"), "{chosen}");
    let gif = ops::put_icon::Params {
        part_types: api::PartTypes::from([("icon", "image/gif")]),
        ..ops::put_icon::Params::new(body)
    };
    assert!(send(gif).is_err(), "image/gif is not listed");
}

fn params() -> ops::upload::Params {
    ops::upload::Params {
        image_x_checksum: "sha1-abcd".try_into().expect("4 characters or more"),
        meta_x_revision: Some(7),
        body: types::UploadBody {
            image: api::Binary::from(vec![0x89, b'P', b'N', b'G']),
            meta: types::UploadBodyMeta { title: "gadget".to_owned(), width: None },
            tags: None,
            bundle: Some(types::UploadBodyBundle {
                note: Some("# scan".to_owned()),
                scan: Some(api::Binary::from(vec![0x89, b'P'])),
            }),
            pages: Some(vec!["one".to_owned(), "two".to_owned()]),
        },
    }
}

/// One part of a raw multipart body. Its head is found by `name="<part>"`.
fn part_head(body: &[u8], part: &str) -> String {
    let text = String::from_utf8_lossy(body);
    let start = text.find(&format!("name=\"{part}\"")).expect("part is sent");
    text[start..].split("\r\n\r\n").next().unwrap_or_default().to_owned()
}

#[test]
fn part_headers_travel_in_their_part() {
    let sent: Arc<Mutex<Vec<u8>>> = Arc::default();
    let keep = sent.clone();
    let client = Client::new(support::loopback(move |req: api::Request| {
        *keep.lock().unwrap() = req.body().bytes().to_vec();
        async { api::runtime::new_response(201) }
    }));
    api::block_on(client.upload(params())).unwrap();
    let body = sent.lock().unwrap().clone();
    let image = part_head(&body, "image");
    assert!(image.contains("X-Checksum: sha1-abcd"), "{image}");
    assert!(!image.contains("X-Revision"), "{image}");
    let meta = part_head(&body, "meta");
    assert!(meta.contains("X-Revision: 7"), "{meta}");
}

#[test]
fn server_decodes_part_headers() {
    let store = Arc::new(Store::default());
    let server = store.clone();
    let client = Client::new(support::loopback(move |req: api::Request| {
        let server = server.clone();
        async move { api::Server::new(std::sync::Arc::clone(&server)).into_service().handle(req).await }
    }));
    api::block_on(client.upload(params())).unwrap();
    let got = store.upload.lock().unwrap().take().expect("handler ran");
    assert_eq!(got.image_x_checksum, "sha1-abcd");
    assert_eq!(got.meta_x_revision, Some(7));
}

#[test]
fn missing_required_part_header_is_rejected() {
    // The body as the client encodes it without any part header.
    let (ct, body) =
        api::runtime::multipart_encode(&params().body, &[("image", "image/png"), ("meta", "application/json")])
            .unwrap();
    let mut req = api::runtime::new_request("POST", "/photos".to_owned(), vec![]).unwrap();
    req.headers_mut().insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::try_from(ct).unwrap());
    *req.body_mut() = honig::Body::from(body);
    let resp = api::block_on(api::Server::new(Store::default()).into_service().handle(req));
    assert_eq!(resp.status().as_u16(), 400, "{}", String::from_utf8_lossy(resp.body().bytes()));
}

#[test]
fn a_multipart_part_nests_its_own_parts() {
    let sent: Arc<Mutex<Vec<u8>>> = Arc::default();
    let keep = sent.clone();
    let client = Client::new(support::loopback(move |req: api::Request| {
        *keep.lock().unwrap() = req.body().bytes().to_vec();
        async { api::runtime::new_response(201) }
    }));
    api::block_on(client.upload(params())).unwrap();
    let body = String::from_utf8_lossy(&sent.lock().unwrap()).into_owned();
    // `bundle` is one `multipart/mixed` part whose own parts follow the nested `encoding`.
    let bundle = part_head(body.as_bytes(), "bundle");
    assert!(bundle.contains("Content-Type: multipart/mixed; boundary="), "{bundle}");
    assert!(body.contains("name=\"note\"\r\nContent-Type: text/markdown"), "{body}");
    assert!(body.contains("Content-Type: image/png"), "{body}");
    // `pages`: one positional part per item. The first goes by `prefixEncoding` and the rest by `itemEncoding`.
    assert!(body.contains("Content-Type: text/markdown\r\n\r\none\r\n"), "{body}");
    assert!(body.contains("Content-Type: text/plain\r\n\r\ntwo\r\n"), "{body}");
}

#[test]
fn nested_parts_decode_back() {
    let store = Arc::new(Store::default());
    let server = store.clone();
    let client = Client::new(support::loopback(move |req: api::Request| {
        let server = server.clone();
        async move { api::Server::new(std::sync::Arc::clone(&server)).into_service().handle(req).await }
    }));
    api::block_on(client.upload(params())).unwrap();
    let got = store.upload.lock().unwrap().take().expect("handler ran");
    assert_eq!(got.body.bundle, params().body.bundle);
    assert_eq!(got.body.pages, params().body.pages);
}

/// `contentEncoding: base64` on a multipart field: the part goes out base64 with its `Content-Transfer-Encoding`.
#[test]
fn a_base64_field_travels_transfer_encoded() {
    let sent: Arc<Mutex<Vec<u8>>> = Arc::default();
    let keep = sent.clone();
    let client = Client::new(support::loopback(move |req: api::Request| {
        *keep.lock().unwrap() = req.body().bytes().to_vec();
        async { api::runtime::new_response(204) }
    }));
    let body = types::UploadScanBody { label: None, scan: api::Binary::from(b"hello".to_vec()) };
    api::block_on(client.upload_scan(ops::upload_scan::Params::new(body))).unwrap();
    let text = String::from_utf8_lossy(&sent.lock().unwrap()).into_owned();
    let scan = part_head(text.as_bytes(), "scan");
    assert!(scan.contains("Content-Transfer-Encoding: base64"), "{scan}");
    assert!(text.contains("\r\n\r\naGVsbG8=\r\n"), "{text}");
}

/// A part header has the limits of its schema: `X-Checksum` needs at least 4 characters. Only a valid one is a value
/// of its type.
#[test]
fn a_part_header_is_checked_against_its_schema() {
    assert_eq!(types::UploadImageXChecksum::try_from("abcd").map(String::from), Ok("abcd".to_owned()));
    assert_eq!(types::UploadImageXChecksum::try_from("abc"), Err("at least 4 characters required, found 3".to_owned()));
}
