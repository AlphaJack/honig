//! A text body whose schema has limits: the server decodes it through the type of that schema and the client cannot
//! build one outside them.

mod support;

use std::future::Future;

use e2e::v32_text_bodies::api::{self, Client, operations as ops, types};

struct Notes;

impl api::Api for Notes {
    type Principal = ();

    fn add_note(
        &self,
        p: ops::add_note::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::add_note::Response, api::ApiError>> + Send {
        let loud = types::AddNoteOk::try_from(p.body.to_uppercase().as_str()).map_err(api::ApiError::from);
        std::future::ready(loud.map(ops::add_note::Response::Ok))
    }
    fn set_mood(
        &self,
        _: ops::set_mood::Params,
        _: &api::Context<'_>,
    ) -> impl Future<Output = Result<ops::set_mood::Response, api::ApiError>> + Send {
        std::future::ready(Ok(ops::set_mood::Response::NoContent))
    }
}

fn send(method: &'static str, path: &str, text: &str) -> u16 {
    let mut req = api::runtime::new_request(method, path.to_owned(), vec![]).unwrap();
    req.headers_mut().insert(honig::http::header::CONTENT_TYPE, honig::http::HeaderValue::from_static("text/plain"));
    *req.body_mut() = honig::Body::from(text.as_bytes().to_vec());
    api::block_on(api::Server::new(Notes).into_service().handle(req)).status().as_u16()
}

#[test]
fn a_text_body_outside_its_schema_is_refused() {
    assert_eq!(send("POST", "/notes", "hello"), 200);
    assert_eq!(send("POST", "/notes", "hello!"), 400, "6 characters, maxLength 5");
    assert_eq!(send("PUT", "/moods", "calm"), 204);
    assert_eq!(send("PUT", "/moods", "sad"), 400, "not one of the enum");
}

#[test]
fn the_client_builds_and_decodes_text_bodies_of_their_types() {
    assert!(types::AddNoteBody::try_from("hello!").is_err(), "the client cannot build a body outside its limits");
    let client = Client::new(support::loopback(|req: api::Request| api::Server::new(Notes).into_service().handle(req)));
    let body = types::AddNoteBody::try_from("hi!").expect("3 characters");
    let note = api::block_on(client.add_note(ops::add_note::Params::new(body))).expect("sent").into_result();
    assert_eq!(note.expect("200").as_ref(), "HI!");
    let mood = ops::set_mood::Params::new(types::SetMoodBody::Loud);
    api::block_on(client.set_mood(mood)).expect("sent").into_result().expect("204");
}
