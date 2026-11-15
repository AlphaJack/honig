//! Hand-written checks for the `xml` fixture: 3.2 `nodeType: cdata` and `none`.

mod support;

use e2e::v32_xml::api::{self, operations as ops, types};

#[test]
fn cdata_and_inline_nodes_on_the_wire() {
    let book = types::Book {
        id: 1,
        lang: None,
        title: "Dune".to_owned(),
        subtitle: api::Nullable::Absent,
        authors: vec![],
        tags: None,
        price: None,
        sequel: None,
        blurb: Some(types::Blurb { format: None, text: "<b>sand</b> ]]> worms".to_owned() }),
        meta: Some(types::Meta { isbn: "978".to_owned(), pages: Some(412) }),
    };
    let wire = String::from_utf8(api::runtime::xml_encode(&book).expect("encodes")).expect("utf-8");
    assert!(wire.contains("<blurb><![CDATA[<b>sand</b> ]]]]><![CDATA[> worms]]></blurb>"), "{wire}");
    assert!(wire.contains("<isbn>978</isbn><pages>412</pages>") && !wire.contains("<meta"), "inline: {wire}");
    let back: types::Book = api::runtime::xml_decode(200, wire.as_bytes()).expect("decodes");
    assert_eq!(back.blurb, book.blurb);
    assert_eq!(back.meta, book.meta);
}

/// Stores nothing: answers `add_book` with the book it got. The server writes it in the media type `Accept` chose.
struct Books;

impl api::Api for Books {
    type Principal = ();

    async fn add_book(
        &self,
        p: ops::add_book::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::add_book::Response, api::ApiError> {
        Ok(ops::add_book::Response::Created(p.body))
    }
}

fn book() -> types::Book {
    serde_json::from_value(serde_json::json!({"id": 1, "title": "Dune", "authors": []})).expect("a book")
}

/// The request that the generated client sends for `add_book` with `accept` in place of its own `Accept`.
fn request(accept: &str) -> api::Request {
    let sent = std::sync::Arc::new(std::sync::Mutex::new(None));
    let keep = std::sync::Arc::clone(&sent);
    let client = api::Client::new(support::loopback(move |req: api::Request| {
        *keep.lock().unwrap() = Some(req);
        std::future::ready(api::runtime::new_response(500))
    }));
    let _ = api::block_on(client.add_book(ops::add_book::Params::new(book())));
    let mut req = sent.lock().unwrap().take().expect("sent");
    req.headers_mut().remove("accept");
    req.headers_mut().append("accept", honig::http::HeaderValue::try_from(accept.to_owned()).unwrap());
    req
}

fn served(accept: &str) -> api::Response {
    let service = api::Server::new(Books).into_service();
    api::block_on(service.handle(request(accept)))
}

/// One `Book` in either media type. The server writes the one that `Accept` prefers. 406 when it takes neither.
#[test]
fn the_server_writes_the_media_type_accept_prefers() {
    assert_eq!(
        served("application/xml").headers().get("content-type").map_or("", |v| v.to_str().unwrap_or("")),
        "application/xml"
    );
    assert_eq!(
        served("application/json").headers().get("content-type").map_or("", |v| v.to_str().unwrap_or("")),
        "application/json"
    );
    assert_eq!(
        served("application/xml;q=0.1, application/json")
            .headers()
            .get("content-type")
            .map_or("", |v| v.to_str().unwrap_or("")),
        "application/json"
    );
    assert_eq!(
        served("*/*").headers().get("content-type").map_or("", |v| v.to_str().unwrap_or("")),
        "application/xml",
        "a tie goes to the description's order"
    );
    assert_eq!(served("text/html").status().as_u16(), 406);
}

/// Keeps the media type the server handed `add_book` in `params.media`.
#[derive(Default)]
struct MediaSeen(std::sync::Mutex<Option<ops::add_book::Media>>);

impl api::Api for MediaSeen {
    type Principal = ();

    async fn add_book(
        &self,
        p: ops::add_book::Params,
        _: &api::Context<'_>,
    ) -> Result<ops::add_book::Response, api::ApiError> {
        *self.0.lock().unwrap() = Some(p.media);
        Ok(ops::add_book::Response::Created(p.body))
    }
}

/// The handler sees in `params.media` the media type that `Accept` chose. Its answer is written in it.
#[test]
fn the_handler_sees_the_media_type_accept_chose() {
    for (accept, want) in
        [("application/xml", ops::add_book::Media::Xml), ("application/json", ops::add_book::Media::Json)]
    {
        let seen = std::sync::Arc::new(MediaSeen::default());
        let service = api::Server::new(std::sync::Arc::clone(&seen)).into_service();
        api::block_on(service.handle(request(accept)));
        assert_eq!(*seen.0.lock().unwrap(), Some(want), "{accept}");
    }
}

/// The client prefers a media type, accepts the other and decodes the same `Book` from either.
#[test]
fn the_client_reads_one_type_from_either_media_type() {
    for (media, accept) in [
        (ops::add_book::Media::Xml, "application/xml, application/json;q=0.9"),
        (ops::add_book::Media::Json, "application/json, application/xml;q=0.9"),
    ] {
        let service = api::Server::new(Books).into_service();
        let client = api::Client::new(support::loopback(move |req: api::Request| {
            assert_eq!(
                req.headers().get("accept").and_then(|v| v.to_str().ok()),
                Some(accept),
                "the client prefers the media type it was given"
            );
            service.handle(req)
        }));
        let params = ops::add_book::Params { media, ..ops::add_book::Params::new(book()) };
        let created = api::block_on(client.add_book(params)).unwrap();
        assert_eq!(created, ops::add_book::Response::Created(book()), "{media:?}");
    }
}
