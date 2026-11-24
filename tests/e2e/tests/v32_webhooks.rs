//! Hand-written checks for the `webhooks` fixture. What `WebhookClient` sends `server::webhook` receives.

use assert_matches::assert_matches;
use std::sync::{Arc, Mutex};

use e2e::v32_webhooks::api::{self, WebhookClient, operations as ops, types};

/// Keeps what it receives. Answers `newProduct` with `410 Gone` for a product named `stop`.
#[derive(Default)]
struct Receiver {
    products: Mutex<Vec<types::Product>>,
    events: Mutex<Vec<types::Event>>,
}

impl api::Webhooks for Receiver {
    async fn new_product(
        &self,
        p: ops::new_product::Params,
        _: &api::WebhookContext<'_>,
    ) -> Result<ops::new_product::Response, api::ApiError> {
        let gone = p.body.name == "stop";
        self.products.lock().unwrap().push(p.body);
        Ok(if gone { ops::new_product::Response::Gone } else { ops::new_product::Response::Ok })
    }

    async fn subscribe_on_event_post(
        &self,
        p: ops::subscribe_on_event_post::Params,
        _: &api::WebhookContext<'_>,
    ) -> Result<ops::subscribe_on_event_post::Response, api::ApiError> {
        self.events.lock().unwrap().push(p.body);
        Ok(ops::subscribe_on_event_post::Response::NoContent)
    }
}

/// A webhook client whose requests reach `receiver`'s server for webhook or callback `name`.
fn sender(receiver: &Arc<Receiver>, name: &'static str) -> WebhookClient<impl api::Transport> {
    let service = api::webhook(Arc::clone(receiver), name).into_service();
    WebhookClient::new(service)
}

#[test]
fn a_webhook_reaches_its_receiver_and_its_answer_comes_back() {
    let receiver = Arc::new(Receiver::default());
    let client = sender(&receiver, "newProduct");
    let lamp = types::Product { id: 1, name: "Lamp".to_owned() };
    let got = api::block_on(
        client.new_product("https://hooks.example/products", ops::new_product::Params::new(lamp.clone())),
    );
    assert_eq!(got.ok(), Some(ops::new_product::Response::Ok));
    let stop = types::Product { id: 2, name: "stop".to_owned() };
    let got = api::block_on(
        client.new_product("https://hooks.example/products", ops::new_product::Params::new(stop.clone())),
    );
    assert_eq!(got.ok(), Some(ops::new_product::Response::Gone), "410 is a declared answer, not an error");
    assert_eq!(*receiver.products.lock().unwrap(), [lamp, stop]);
}

#[test]
fn a_callback_reaches_its_receiver() {
    let receiver = Arc::new(Receiver::default());
    let client = sender(&receiver, "onEvent");
    let event: types::Event = serde_json::from_value(serde_json::json!({"kind": "created"})).unwrap();
    let got =
        api::block_on(client.subscribe_on_event_post(
            "https://app.example/cb",
            ops::subscribe_on_event_post::Params::new(event.clone()),
        ));
    assert_eq!(got.ok(), Some(ops::subscribe_on_event_post::Response::NoContent));
    assert_eq!(*receiver.events.lock().unwrap(), [event]);
}

/// A server from `server::webhook` serves its one webhook at whatever URL it is mounted on. The body of another
/// webhook is bad input for it and reaches no method.
#[test]
fn a_webhook_server_reads_every_request_as_its_own_webhook() {
    let receiver = Arc::new(Receiver::default());
    let client = sender(&receiver, "onEvent");
    let got = api::block_on(client.new_product(
        "https://hooks.example/products",
        ops::new_product::Params::new(types::Product { id: 1, name: "Lamp".to_owned() }),
    ));
    assert_matches!(
        got,
        Err(api::ClientError::UnexpectedStatus { status: api::http::StatusCode::BAD_REQUEST, .. }),
        "{got:?}"
    );
    assert!(receiver.products.lock().unwrap().is_empty() && receiver.events.lock().unwrap().is_empty());
}
