//! Hand-written checks for the `kitchen-sink` fixture.

mod support;

use std::sync::Mutex;

use e2e::v32_kitchen_sink::api::{self, Client, operations as ops, types};

#[test]
#[allow(deprecated, reason = "`note` is deprecated in the description on purpose")]
fn form_fields_follow_their_encoding_style() {
    let seen = std::sync::Arc::new(Mutex::new(String::new()));
    let saw = std::sync::Arc::clone(&seen);
    let client = Client::new(support::loopback(move |req: api::Request| {
        let seen = std::sync::Arc::clone(&saw);
        async move {
            // The own server of the operation and not the server of the API.
            assert_eq!(req.uri().to_string(), "https://forms.example.com/forms");
            if let Ok(mut s) = seen.lock() {
                *s = String::from_utf8_lossy(req.body().bytes()).into_owned();
            }
            api::runtime::new_response(204)
        }
    }));
    let body = types::SubmitFormBody {
        tags: vec!["a".to_owned(), "b".to_owned()],
        filter: Some(types::SubmitFormBodyFilter { a: "x".to_owned(), b: None }),
        note: Some("a/b?c".to_owned()),
        priority: types::SubmitFormBodyPriority::N2,
        blob: Some(api::Binary::from(vec![0xde, 0xad])),
    };
    api::block_on(client.submit_form(ops::submit_form::Params { body })).unwrap_or_else(|e| panic!("{e}"));
    let wire = seen.lock().map(|s| s.clone()).unwrap_or_default();
    assert!(wire.contains("tags=a|b") || wire.contains("tags=a%7Cb"), "pipeDelimited: {wire}");
    assert!(wire.contains("filter%5Ba%5D=x") || wire.contains("filter[a]=x"), "deepObject: {wire}");
    assert!(wire.contains("note=a/b?c"), "allowReserved: {wire}");
    assert!(wire.contains("blob=3q0"), "format: byte is base64: {wire}");
    assert!(wire.contains("priority=2"), "integer enums travel as numbers: {wire}");
}

#[allow(deprecated, reason = "`note` is deprecated in the description on purpose")]
fn form(priority: types::SubmitFormBodyPriority) -> ops::submit_form::Params {
    let body = types::SubmitFormBody { tags: vec!["a".to_owned()], filter: None, note: None, priority, blob: None };
    ops::submit_form::Params { body }
}

#[test]
fn integer_enum_values_are_checked() {
    let mut out = vec![];
    api::Validate::validate(&form(types::SubmitFormBodyPriority::N2), &api::InstancePath::Root, &mut out);
    assert!(out.is_empty(), "{out:?}");
    assert_eq!(types::SubmitFormBodyPriority::try_from(2), Ok(types::SubmitFormBodyPriority::N2));
    assert_eq!(types::SubmitFormBodyPriority::try_from(7), Err("`7` is not one of `1`, `2`, `3`".to_owned()));
    assert!(serde_json::from_str::<types::SubmitFormBodyPriority>("7").is_err());
}

#[test]
fn path_servers_expand_their_variables() {
    assert_eq!(
        api::runtime::operation_path(ops::put_blob::SERVERS, ops::put_blob::SERVER_VARIABLES, "/blobs/1".to_owned()),
        "https://eu.blobs.example.com/blobs/1"
    );
    assert_eq!(api::runtime::split_target("https://eu.blobs.example.com/blobs/1?x=1"), ("/blobs/1", "x=1"));
}

/// The servers of the API by index with their variables at a chosen value or their default.
#[test]
fn api_servers_expand_their_variables() {
    assert_eq!(Client::<()>::server_url(0, &[]).as_deref(), Ok("https://eu.example.com/v1"));
    assert_eq!(Client::<()>::server_url(0, &[("region", "us")]).as_deref(), Ok("https://us.example.com/v1"));
    assert_eq!(Client::<()>::server_url(1, &[]), Err("no server 1".to_owned()));
}

/// The `createProduct` request a server received: the callback URLs read its body and its query.
fn received() -> api::Request {
    let mut req = api::runtime::new_request(
        "POST",
        "/products".to_owned(),
        vec!["url=https%3A%2F%2Fhooks.example.com%2Fshared".to_owned()],
    )
    .unwrap();
    req.headers_mut()
        .insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("application/json"));
    *req.body_mut() =
        honig::Body::from(br#"{"name":"Lamp","kind":"book","callbackUrl":"https://hooks.example.com/adopt"}"#.to_vec());
    req
}

#[test]
fn callback_urls_evaluate_their_expressions_against_the_request() {
    let req = received();
    let product: types::ProductRequest = serde_json::from_slice(req.body().bytes()).unwrap_or_else(|e| panic!("{e}"));
    let params = ops::create_product::Params::new(ops::create_product::Body::Json(product));
    let exchange = api::Exchange { request: Some(&req), ..api::Exchange::default() };
    assert_eq!(
        ops::create_product::callbacks::on_adopt_url(&params, &exchange).as_deref(),
        Some("https://hooks.example.com/adopt"),
        "`{{$request.body#/callbackUrl}}`"
    );
    assert_eq!(
        ops::create_product::callbacks::shared_url(&params, &exchange).as_deref(),
        Some("https://hooks.example.com/shared"),
        "`{{$request.query.url}}`, from `components/callbacks`"
    );
    let bare = api::Exchange::default();
    assert_eq!(ops::create_product::callbacks::on_adopt_url(&params, &bare), None, "no request, no URL");
}

#[test]
fn a_closed_enum_refuses_unknown_values_and_writes_every_value_back() {
    let kind = types::ProductKind::Book;
    assert_eq!(serde_json::from_str::<types::ProductKind>("\"book\"").ok(), Some(kind));
    assert_eq!(serde_json::to_string(&kind).ok(), Some("\"book\"".to_owned()));
    assert_eq!(kind.to_string(), "book");
    assert_eq!("book".parse::<types::ProductKind>(), Ok(kind));
    let err = serde_json::from_str::<types::ProductKind>("\"bird\"").expect_err("not listed");
    assert!(err.to_string().starts_with("`bird` is not one of `"), "{err}");
    assert!(types::ProductKind::try_from("bird").is_err());
}

#[test]
fn a_pattern_newtype_reads_back_the_text_it_was_built_from() {
    let name = types::ProductName::try_from("Lamp the Book").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(name.as_str(), "Lamp the Book");
    assert_eq!(AsRef::<str>::as_ref(&name), "Lamp the Book");
    assert_eq!(name.to_string(), "Lamp the Book");
    assert_eq!(name.into_inner(), "Lamp the Book");
    assert!(types::ProductName::try_from("Rex1").is_err(), "breaks `^[A-Za-z ]+$`");
}

#[test]
fn each_scheme_hands_the_operation_its_typed_credential() {
    use api::runtime::{KeyIn, Scheme};
    use api::server::{Credential, credentials};
    /// A scheme, the header that satisfies it and the credential that the operation sees.
    type Case = (Scheme, Option<(&'static str, &'static str)>, Credential<'static>);
    let cases: [Case; 7] = [
        (Scheme::Bearer("bearer"), Some(("Authorization", "Bearer jwt")), Credential::Bearer("jwt")),
        (
            Scheme::Basic("basic"),
            Some(("Authorization", "Basic dTpwOnc=")),
            Credential::Basic { user: "u", password: "p:w" },
        ),
        (Scheme::Basic("basic"), Some(("Authorization", "Basic dQ==")), Credential::Basic { user: "u", password: "" }),
        (Scheme::ApiKey("apiKey", KeyIn::Header, "X-Key"), Some(("X-Key", "k1")), Credential::ApiKey("k1")),
        (Scheme::MutualTls("mtls"), None, Credential::Mtls),
        (Scheme::Bearer("oidc"), Some(("Authorization", "Bearer id")), Credential::Oidc("id")),
        (Scheme::Bearer("oauth"), Some(("Authorization", "Bearer at")), Credential::Oauth("at")),
    ];
    for (scheme, header, want) in cases {
        let mut req = api::runtime::new_request("GET", "/products".to_owned(), vec![]).unwrap();
        if let Some((name, value)) = header {
            req.headers_mut().append(
                honig::http::HeaderName::try_from(name).unwrap(),
                honig::http::HeaderValue::try_from(value.to_owned()).unwrap(),
            );
        }
        let requirements: &[&[api::runtime::Requirement]] = &[&[(scheme, &[])]];
        let satisfied = api::runtime::check_auth(req.headers(), &[], requirements)
            .unwrap_or_else(|| panic!("{scheme:?} satisfied"));
        let event = api::RequestEvent::new(Some(api::Operation::ListProducts), "GET");
        let ctx = api::Context::new(&req, api::Operation::ListProducts, satisfied, &event);
        assert_eq!(credentials(&ctx), [want], "{scheme:?}");
    }
}

/// Answers every operation from what it received and keeps the last parameters that it decoded. A client to server
/// round trip then shows that the server read the request that the client wrote.
#[derive(Default)]
struct Products {
    seen: Mutex<Option<Box<dyn std::any::Any + Send>>>,
}

impl Products {
    fn keep<P: Clone + Send + 'static>(&self, p: &P) {
        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some(Box::new(p.clone()));
        }
    }

    /// The parameters that the server decoded last if they are a `P`.
    fn last<P: Clone + 'static>(&self) -> Option<P> {
        self.seen.lock().ok()?.as_ref()?.downcast_ref::<P>().cloned()
    }
}

fn lamp() -> types::Product {
    let name = types::ProductName::try_from("Lamp").unwrap_or_else(|e| panic!("{e}"));
    types::Product::new(7.try_into().unwrap_or_else(|e| panic!("{e}")), name, types::ProductKind::Book)
}

type Ctx<'a> = api::Context<'a, Option<()>>;
type Answer<R> = Result<R, api::ApiError>;

impl api::Api for Products {
    type Principal = ();

    async fn submit_form(&self, p: ops::submit_form::Params, _: &Ctx<'_>) -> Answer<ops::submit_form::Response> {
        self.keep(&p);
        Ok(ops::submit_form::Response::NoContent)
    }

    async fn put_blob(&self, p: ops::put_blob::Params, _: &Ctx<'_>) -> Answer<ops::put_blob::Response> {
        let bytes = p.body.collect().await?;
        let size = i64::try_from(bytes.len()).ok();
        Ok(ops::put_blob::Response::Ok { body: bytes.into(), x_size: size, x_quota: types::PutBlobOkXQuota::new(10) })
    }

    async fn list_products(&self, p: ops::list_products::Params, _: &Ctx<'_>) -> Answer<ops::list_products::Response> {
        self.keep(&p);
        let body = match p.media {
            ops::list_products::Media::Json => ops::list_products::OkBody::Json(vec![lamp()]),
            ops::list_products::Media::Jsonl => ops::list_products::OkBody::Jsonl(vec![lamp()].into()),
        };
        Ok(ops::list_products::Response::Ok { body, x_next: Some("next".to_owned()), x_total: Some(1) })
    }

    async fn create_product(
        &self,
        p: ops::create_product::Params,
        _: &api::Context<'_>,
    ) -> Answer<ops::create_product::Response> {
        self.keep(&p);
        Ok(ops::create_product::Response::Created)
    }

    async fn search_products(
        &self,
        p: ops::search_products::Params,
        _: &Ctx<'_>,
    ) -> Answer<ops::search_products::Response> {
        self.keep(&p);
        Ok(ops::search_products::Response::Ok(vec![lamp()]))
    }

    async fn copy_products(&self, p: ops::copy_products::Params, _: &Ctx<'_>) -> Answer<ops::copy_products::Response> {
        self.keep(&p);
        Ok(ops::copy_products::Response::NoContent)
    }

    async fn get_product(&self, p: ops::get_product::Params, _: &Ctx<'_>) -> Answer<ops::get_product::Response> {
        self.keep(&p);
        Ok(ops::get_product::Response::Ok(types::Product {
            id: p
                .product_id
                .try_into()
                .map_err(|e: String| api::ApiError::new(api::http::StatusCode::BAD_REQUEST, e))?,
            ..lamp()
        }))
    }

    async fn put_product(&self, p: ops::put_product::Params, _: &Ctx<'_>) -> Answer<ops::put_product::Response> {
        self.keep(&p);
        Ok(ops::put_product::Response::Ok)
    }

    async fn delete_product(
        &self,
        p: ops::delete_product::Params,
        _: &Ctx<'_>,
    ) -> Answer<ops::delete_product::Response> {
        self.keep(&p);
        Ok(ops::delete_product::Response::NoContent)
    }

    async fn options_product(
        &self,
        p: ops::options_product::Params,
        _: &Ctx<'_>,
    ) -> Answer<ops::options_product::Response> {
        self.keep(&p);
        Ok(ops::options_product::Response::Ok)
    }

    async fn head_product(&self, p: ops::head_product::Params, _: &Ctx<'_>) -> Answer<ops::head_product::Response> {
        self.keep(&p);
        Ok(ops::head_product::Response::Ok)
    }

    async fn patch_product(&self, p: ops::patch_product::Params, _: &Ctx<'_>) -> Answer<ops::patch_product::Response> {
        self.keep(&p);
        Ok(ops::patch_product::Response::Ok)
    }

    async fn trace_product(&self, p: ops::trace_product::Params, _: &Ctx<'_>) -> Answer<ops::trace_product::Response> {
        self.keep(&p);
        Ok(ops::trace_product::Response::Ok)
    }

    async fn search(&self, p: ops::search::Params, _: &Ctx<'_>) -> Answer<ops::search::Response> {
        self.keep(&p);
        Ok(ops::search::Response::Ok(vec![types::SearchOkItem::new(types::SearchOkItemData(lamp()))].into()))
    }
}

/// A client whose requests reach `products` through the kitchen-sink server.
#[allow(deprecated, reason = "`oauth` is deprecated in the description on purpose")]
fn served(products: &std::sync::Arc<Products>) -> Client<impl api::Transport> {
    let service = api::Server::new(std::sync::Arc::clone(products)).into_service();
    Client::new(service).with_oauth("t")
}

#[test]
#[allow(deprecated, reason = "`note` is deprecated in the description on purpose")]
fn the_server_decodes_the_form_the_client_encodes() {
    let products = std::sync::Arc::new(Products::default());
    let body = types::SubmitFormBody {
        tags: vec!["a".to_owned(), "b".to_owned()],
        filter: Some(types::SubmitFormBodyFilter { a: "x".to_owned(), b: Some("y z".to_owned()) }),
        note: Some("a/b?c".to_owned()),
        priority: types::SubmitFormBodyPriority::N3,
        blob: Some(api::Binary::from(vec![0xde, 0xad])),
    };
    let sent = ops::submit_form::Params { body };
    let got = api::block_on(served(&products).submit_form(sent.clone()));
    assert_eq!(got.ok(), Some(ops::submit_form::Response::NoContent));
    assert_eq!(products.last(), Some(sent));
}

/// Every method of `/products/{productId}`, `COPY` and the path parameter reach their operation.
#[test]
fn each_method_reaches_its_operation() {
    let products = std::sync::Arc::new(Products::default());
    let client = served(&products);
    assert_eq!(
        api::block_on(client.get_product(ops::get_product::Params { product_id: 9 })).ok(),
        Some(ops::get_product::Response::Ok(types::Product {
            id: 9.try_into().unwrap_or_else(|e| panic!("{e}")),
            ..lamp()
        }))
    );
    assert_eq!(products.last(), Some(ops::get_product::Params { product_id: 9 }));
    assert_eq!(
        api::block_on(client.put_product(ops::put_product::Params { product_id: 9 })).ok(),
        Some(ops::put_product::Response::Ok)
    );
    assert_eq!(products.last(), Some(ops::put_product::Params { product_id: 9 }));
    assert_eq!(
        api::block_on(client.delete_product(ops::delete_product::Params { product_id: 9 })).ok(),
        Some(ops::delete_product::Response::NoContent)
    );
    assert_eq!(products.last(), Some(ops::delete_product::Params { product_id: 9 }));
    assert_eq!(
        api::block_on(client.options_product(ops::options_product::Params { product_id: 9 })).ok(),
        Some(ops::options_product::Response::Ok)
    );
    assert_eq!(products.last(), Some(ops::options_product::Params { product_id: 9 }));
    assert_eq!(
        api::block_on(client.head_product(ops::head_product::Params { product_id: 9 })).ok(),
        Some(ops::head_product::Response::Ok)
    );
    assert_eq!(products.last(), Some(ops::head_product::Params { product_id: 9 }));
    assert_eq!(
        api::block_on(client.patch_product(ops::patch_product::Params { product_id: 9 })).ok(),
        Some(ops::patch_product::Response::Ok)
    );
    assert_eq!(products.last(), Some(ops::patch_product::Params { product_id: 9 }));
    assert_eq!(
        api::block_on(client.trace_product(ops::trace_product::Params { product_id: 9 })).ok(),
        Some(ops::trace_product::Response::Ok)
    );
    assert_eq!(products.last(), Some(ops::trace_product::Params { product_id: 9 }));
    let copy = ops::copy_products::Params { x_trace: Some("01234567-89ab-4def-8123-456789abcdef".to_owned()) };
    assert_eq!(api::block_on(client.copy_products(copy.clone())).ok(), Some(ops::copy_products::Response::NoContent));
    assert_eq!(products.last(), Some(copy));
}

/// The wide event of a method that the description adds (`COPY`) names that method and not `OTHER`.
#[test]
#[allow(deprecated, reason = "`oauth` is deprecated in the description on purpose")]
fn the_event_names_an_additional_method() {
    let events = std::sync::Arc::new(Mutex::new(vec![]));
    let sink = std::sync::Arc::clone(&events);
    let service = api::Server::new(Products::default())
        .on_event(move |event| sink.lock().unwrap().push(event.fields()))
        .into_service();
    let client = Client::new(service).with_oauth("t");
    let copy = ops::copy_products::Params { x_trace: None };
    assert_eq!(api::block_on(client.copy_products(copy)).ok(), Some(ops::copy_products::Response::NoContent));
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["method"], "COPY");
}

/// `listProducts` in each media type that `Accept` can choose with its query, cookie and header parameters.
#[test]
#[allow(deprecated, reason = "`X-Rate` is deprecated in the description on purpose")]
fn list_products_answers_in_the_media_type_asked_for() {
    let products = std::sync::Arc::new(Products::default());
    let client = served(&products);
    for (media, body) in [
        (ops::list_products::Media::Json, ops::list_products::OkBody::Json(vec![lamp()])),
        (ops::list_products::Media::Jsonl, ops::list_products::OkBody::Jsonl(vec![lamp()].into())),
    ] {
        let sent = ops::list_products::Params {
            limit: Some(5.try_into().unwrap_or_else(|e| panic!("{e}"))),
            tags: Some(vec!["a".to_owned(), "b c".to_owned()]),
            filter: Some([("color".to_owned(), "red".to_owned())].into()),
            session: Some("s1".to_owned()),
            x_rate: Some(3),
            x_trace: Some("01234567-89ab-4def-8123-456789abcdef".to_owned()),
            media,
            ..ops::list_products::Params::new()
        };
        let got = api::block_on(client.list_products(sent.clone()));
        let want = ops::list_products::Response::Ok { body, x_next: Some("next".to_owned()), x_total: Some(1) };
        assert_eq!(got.ok(), Some(want), "{media:?}");
        assert_eq!(products.last(), Some(sent), "{media:?}");
    }
}

/// `createProduct` takes JSON, `multipart/form-data` and `multipart/mixed` bodies.
#[test]
fn create_product_reads_each_body_media_type() {
    let products = std::sync::Arc::new(Products::default());
    let client = served(&products);
    let product: types::ProductRequest =
        serde_json::from_value(serde_json::json!({"name": "Lamp", "kind": "book"})).unwrap_or_else(|e| panic!("{e}"));
    let form =
        types::CreateProductBodyRequest { meta: Some(product.clone()), photo: Some(api::Binary::from(vec![1, 2, 3])) };
    let part: std::collections::BTreeMap<String, serde_json::Value> = [("a".to_owned(), serde_json::json!(1))].into();
    for body in [
        ops::create_product::Body::Json(product),
        ops::create_product::Body::FormData(form),
        ops::create_product::Body::Mixed(vec![part]),
    ] {
        let sent = ops::create_product::Params::new(body);
        let got = api::block_on(client.create_product(sent.clone()));
        assert_eq!(got.ok(), Some(ops::create_product::Response::Created), "{sent:?}");
        assert_eq!(products.last(), Some(sent));
    }
}

/// `QUERY` carries a body, and `search` answers with server-sent events.
#[test]
fn query_and_event_stream_operations_round_trip() {
    let products = std::sync::Arc::new(Products::default());
    let client = served(&products);
    let sent = ops::search_products::Params {
        body: Some([("kind".to_owned(), serde_json::json!("book"))].into()),
        ..ops::search_products::Params::new()
    };
    assert_eq!(
        api::block_on(client.search_products(sent.clone())).ok(),
        Some(ops::search_products::Response::Ok(vec![lamp()]))
    );
    assert_eq!(products.last(), Some(sent));
    let q = types::SearchQ { term: Some("lamp".to_owned()), page: Some(2), filters: None };
    let sent = ops::search::Params { q: Some(q) };
    let want = ops::search::Response::Ok(vec![types::SearchOkItem::new(types::SearchOkItemData(lamp()))].into());
    assert_eq!(api::block_on(client.search(sent.clone())).ok(), Some(want));
    assert_eq!(products.last(), Some(sent));
}

/// `putBlob` goes to its path's own server and streams raw bytes both ways.
#[test]
fn a_blob_goes_to_its_own_server_and_back() {
    let products = std::sync::Arc::new(Products::default());
    let client = served(&products);
    let sent = ops::put_blob::Params::new("1".to_owned(), api::Blob::from(vec![1, 2, 3]));
    let got = api::block_on(client.put_blob(sent));
    let want = ops::put_blob::Response::Ok {
        body: vec![1, 2, 3].into(),
        x_size: Some(3),
        x_quota: types::PutBlobOkXQuota::new(10),
    };
    assert_eq!(got.ok(), Some(want));
}

mod malformed;

static FIXTURE: malformed::Fixture = malformed::Fixture {
    routes: &[
        (ops::submit_form::METHOD, ops::submit_form::PATH),
        (ops::put_blob::METHOD, ops::put_blob::PATH),
        (ops::list_products::METHOD, ops::list_products::PATH),
        (ops::create_product::METHOD, ops::create_product::PATH),
        (ops::search_products::METHOD, ops::search_products::PATH),
        (ops::copy_products::METHOD, ops::copy_products::PATH),
        (ops::get_product::METHOD, ops::get_product::PATH),
        (ops::put_product::METHOD, ops::put_product::PATH),
        (ops::delete_product::METHOD, ops::delete_product::PATH),
        (ops::options_product::METHOD, ops::options_product::PATH),
        (ops::head_product::METHOD, ops::head_product::PATH),
        (ops::patch_product::METHOD, ops::patch_product::PATH),
        (ops::trace_product::METHOD, ops::trace_product::PATH),
        (ops::search::METHOD, ops::search::PATH),
    ],
    segments: &["7"],
    query_names: &["tags", "limit", "legacy", "filter", "filter[a]", "filter[b]", "empty", "q", "url", "kind", "page"],
    header_names: &["X-Key", "X-Trace", "X-Rate"],
};

proptest::proptest! {
    #![proptest_config(malformed::config())]

    #[test]
    fn malformed_requests_get_a_client_error(case in malformed::case(&FIXTURE)) {
        malformed::check(api::Server::new(Products::default()).into_service(), &case)?;
    }
}
