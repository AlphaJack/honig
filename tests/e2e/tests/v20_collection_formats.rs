//! Hand-written checks for the `collection-formats` fixture (Swagger 2.0).

use std::future::Future;
use std::sync::Mutex;

use e2e::v20_collection_formats::api::{self, operations as ops};

/// Keeps the params that the server decoded per operation.
#[derive(Default)]
struct Seen {
    items: Mutex<Option<ops::list_items::Params>>,
    upload: Mutex<Option<ops::upload::Params>>,
    tag: Mutex<Option<ops::tag::Params>>,
    rows: Mutex<Option<ops::rows::Params>>,
}

impl api::Api for Seen {
    fn list_items(
        &self,
        p: ops::list_items::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::list_items::Response, api::ApiError>> + Send {
        if let Ok(mut seen) = self.items.lock() {
            *seen = Some(p);
        }
        std::future::ready(Ok(ops::list_items::Response::Ok(vec![])))
    }
    fn upload(
        &self,
        p: ops::upload::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::upload::Response, api::ApiError>> + Send {
        if let Ok(mut seen) = self.upload.lock() {
            *seen = Some(p);
        }
        std::future::ready(Ok(ops::upload::Response::NoContent))
    }

    fn tag(
        &self,
        p: ops::tag::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::tag::Response, api::ApiError>> + Send {
        if let Ok(mut seen) = self.tag.lock() {
            *seen = Some(p);
        }
        std::future::ready(Ok(ops::tag::Response::NoContent))
    }

    fn rows(
        &self,
        p: ops::rows::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::rows::Response, api::ApiError>> + Send {
        if let Ok(mut seen) = self.rows.lock() {
            *seen = Some(p);
        }
        std::future::ready(Ok(ops::rows::Response::NoContent))
    }

    fn list_keys(
        &self,
        _: ops::list_keys::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::list_keys::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::list_keys::Response::Ok(keys())))
    }
}

/// What `listKeys` answers: one key with every property and one with only `id`.
fn keys() -> Vec<api::types::Key> {
    serde_json::from_value(serde_json::json!([
        {"id": "k1", "lastSeen": "2024-01-31T13:45:30Z", "owner": {}},
        {"id": "k2"},
    ]))
    .expect("keys")
}

/// A client whose requests reach `seen` through the server.
fn served(seen: &std::sync::Arc<Seen>) -> api::Client<impl api::Transport> {
    let service = api::Server::new(std::sync::Arc::clone(seen)).into_service();
    api::Client::new(service)
}

/// Every `collectionFormat` in the path, the query and a header. What the client joins the server splits back.
#[test]
fn each_collection_format_round_trips() {
    let seen = std::sync::Arc::new(Seen::default());
    let words = || Some(vec!["a".to_owned(), "b c".to_owned()]);
    let sent = ops::list_items::Params {
        ids: vec![1, 2, 3],
        csv: words(),
        ssv: Some(vec!["a".to_owned(), "b".to_owned()]),
        tsv: words(),
        pipes: words(),
        multi: Some(vec![4, 5]),
        x_tags: Some(vec!["x".to_owned(), "y".to_owned()]),
        flag: Some(true),
        page: Some(2),
    };
    let got = api::block_on(served(&seen).list_items(sent.clone()));
    assert_eq!(got.ok(), Some(ops::list_items::Response::Ok(vec![])));
    assert_eq!(seen.items.lock().ok().and_then(|mut s| s.take()), Some(sent));
}

/// A 2.0 `formData` file and field arrive as the `multipart/form-data` body they were upgraded to.
#[test]
fn form_data_parameters_arrive_as_a_multipart_body() {
    let seen = std::sync::Arc::new(Seen::default());
    let body = api::types::UploadBody { file: api::Binary::from(vec![0, 1, 0xFF]), note: Some("a note".to_owned()) };
    let sent = ops::upload::Params::new(body);
    let got = api::block_on(served(&seen).upload(sent.clone()));
    assert_eq!(got.ok(), Some(ops::upload::Response::NoContent));
    assert_eq!(seen.upload.lock().ok().and_then(|mut s| s.take()), Some(sent));
}

/// A 2.0 response `schema` becomes the typed body the client decodes.
#[test]
fn a_response_schema_is_the_typed_body() {
    let got = api::block_on(served(&std::sync::Arc::new(Seen::default())).list_keys(ops::list_keys::Params::new()));
    assert_eq!(got.ok(), Some(ops::list_keys::Response::Ok(keys())));
}

/// Swagger 2.0 has no `nullable`. So an optional property that a service sends as `null` decodes as absent. A required
/// one still refuses it.
#[test]
fn an_optional_property_sent_as_null_is_absent() {
    let key: api::types::Key =
        serde_json::from_str(r#"{"id": "k1", "lastSeen": null, "owner": null}"#).expect("null is absent");
    assert_eq!((key.last_seen, key.owner), (None, None));
    assert!(serde_json::from_str::<api::types::Key>(r#"{"id": null}"#).is_err(), "required");
}

#[test]
fn allow_empty_value_reads_a_bare_flag() {
    let seen = std::sync::Arc::new(Seen::default());
    let req =
        api::runtime::new_request("GET", "/items/1".to_owned(), vec!["flag".to_owned(), "page=".to_owned()]).unwrap();
    let r = api::block_on(api::Server::new(std::sync::Arc::clone(&seen)).into_service().handle(req));
    assert_eq!(r.status().as_u16(), 200, "{}", String::from_utf8_lossy(r.body().bytes()));
    let p = seen.items.lock().ok().and_then(|mut s| s.take()).expect("the handler ran");
    assert_eq!((p.flag, p.page), (Some(true), None));
}

/// A form array is on the wire as its `collectionFormat` says, `csv` when it has none, and the server splits it back.
#[test]
fn a_form_array_keeps_its_collection_format() {
    let body = api::types::TagBody { ids: Some(vec![1, 2]), pipes: Some(vec!["a".to_owned(), "b".to_owned()]) };
    let sent = ops::tag::Params { body: Some(body) };
    let seen = std::sync::Arc::new(Seen::default());
    let got = api::block_on(served(&seen).tag(sent.clone()));
    assert_eq!(got.ok(), Some(ops::tag::Response::NoContent));
    assert_eq!(seen.tag.lock().ok().and_then(|mut s| s.take()), Some(sent.clone()));
    let wire = std::sync::Arc::new(Mutex::new(String::new()));
    let kept = std::sync::Arc::clone(&wire);
    let capture = tower::service_fn(move |req: api::Request| {
        if let Ok(mut w) = kept.lock() {
            *w = String::from_utf8_lossy(req.body().bytes()).into_owned();
        }
        std::future::ready(Ok::<_, api::ApiError>(api::runtime::new_response(204)))
    });
    api::block_on(api::Client::new(capture).tag(sent)).ok();
    assert_eq!(*wire.lock().unwrap(), "ids=1,2&pipes=a%7Cb");
}

/// A header or path array in `ssv`, `pipes` and `tsv` goes out with its separator and the server splits it back.
#[test]
fn a_header_or_path_array_keeps_its_collection_format() {
    let two = || vec!["a".to_owned(), "b".to_owned()];
    let sent = ops::rows::Params {
        ssv: two(),
        pipes: two(),
        tsv: two(),
        x_ssv: Some(two()),
        x_pipes: Some(two()),
        x_tsv: Some(two()),
    };
    let seen = std::sync::Arc::new(Seen::default());
    let got = api::block_on(served(&seen).rows(sent.clone()));
    assert_eq!(got.ok(), Some(ops::rows::Response::NoContent));
    assert_eq!(seen.rows.lock().ok().and_then(|mut s| s.take()), Some(sent.clone()));
    let wire = std::sync::Arc::new(Mutex::new(String::new()));
    let kept = std::sync::Arc::clone(&wire);
    let capture = tower::service_fn(move |req: api::Request| {
        let header = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_owned();
        if let Ok(mut w) = kept.lock() {
            *w = format!("{} {:?} {:?} {:?}", req.uri().path(), header("x-ssv"), header("x-pipes"), header("x-tsv"));
        }
        std::future::ready(Ok::<_, api::ApiError>(api::runtime::new_response(204)))
    });
    api::block_on(api::Client::new(capture).rows(sent)).ok();
    assert_eq!(*wire.lock().unwrap(), r#"/rows/a%20b/a%7Cb/a%09b "a b" "a|b" "a\tb""#);
}
