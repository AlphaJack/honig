//! The generated client against the server of `main.rs`: it creates a product and fetches it back.

#![expect(clippy::disallowed_methods, reason = "runs the server of the example")]

use shop::Catalog;
use shop::api::{self, operations as ops, types};

// ANCHOR: client
// ANCHOR: new-client
/// A client of the catalog at `base_url` with the API key of the staff.
fn staff_client(base_url: &str) -> api::Client<honig::ReqwestTransport> {
    let transport = honig::ReqwestTransport::new(base_url).expect("a base URL");
    api::Client::new(transport).with_api_key(shop::API_KEY)
}
// ANCHOR_END: new-client

// ANCHOR: calls
/// Adds `product` and returns the first product of the catalog, or `None` when the catalog has none.
async fn create_and_read(
    client: &api::Client<impl api::Transport>,
    product: types::Product,
) -> Result<Option<types::Product>, api::ClientError> {
    client.create_product(ops::create_product::Params::new(product)).await?;
    Ok(match client.get_product(ops::get_product::Params::new(0)).await? {
        ops::get_product::Response::Ok(product) => Some(product),
        ops::get_product::Response::NotFound(_) => None,
    })
}
// ANCHOR_END: calls
// ANCHOR_END: client

#[tokio::test]
async fn the_client_creates_and_reads_a_product_from_the_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = api::Server::new(Catalog::default()).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let lamp = types::Product::Physical(types::Physical::new("Desk lamp".to_owned()));
    assert_eq!(create_and_read(&staff_client(&base_url), lamp.clone()).await.unwrap(), Some(lamp));
}

/// The server accepts only the shop's key: a request with another one is refused.
#[tokio::test]
async fn a_wrong_key_is_refused() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = api::Server::new(Catalog::default()).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let client = api::Client::new(honig::ReqwestTransport::new(base_url).expect("a base URL")).with_api_key("guess");
    let answer = client.get_product(ops::get_product::Params::new(0)).await;
    assert!(
        matches!(&answer, Err(api::ClientError::UnexpectedStatus { status, .. }) if status.as_u16() == 401),
        "{answer:?}"
    );
}

/// A photo goes up as the image it is and comes back the same, with its version as `ETag`. A replacement that names
/// another version is refused.
#[tokio::test]
async fn a_photo_comes_back_as_it_went_up() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = api::Server::new(Catalog::default()).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let client = staff_client(&base_url);
    let lamp = types::Product::Physical(types::Physical::new("Desk lamp".to_owned()));
    client.create_product(ops::create_product::Params::new(lamp)).await.unwrap();
    let png = || api::Blob::from(vec![0x89, b'P', b'N', b'G']).with_media("image/png");
    let stored = client.put_product_photo(ops::put_product_photo::Params::new(0, png())).await.unwrap();
    assert!(matches!(stored, ops::put_product_photo::Response::NoContent));
    let got = client.get_product_photo(ops::get_product_photo::Params::new(0)).await.unwrap();
    let ops::get_product_photo::Response::Ok { body, e_tag } = got else { panic!("no photo") };
    assert_eq!((body.media(), e_tag.as_deref()), (Some("image/png"), Some("\"v1\"")));
    assert_eq!(body.collect().await.unwrap(), [0x89, b'P', b'N', b'G']);
    let stale = ops::put_product_photo::Params {
        if_match: Some("\"v0\"".to_owned()),
        ..ops::put_product_photo::Params::new(0, png())
    };
    let refused = client.put_product_photo(stale).await.unwrap();
    assert!(matches!(refused, ops::put_product_photo::Response::PreconditionFailed));
}

/// A partner reads the catalog and cannot add to it.
#[tokio::test]
async fn a_partner_reads_and_cannot_add() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = api::Server::new(Catalog::default()).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let client = api::Client::new(honig::ReqwestTransport::new(base_url).unwrap()).with_api_key(shop::PARTNER_KEY);
    let lamp = types::Product::Physical(types::Physical::new("Desk lamp".to_owned()));
    let refused = client.create_product(ops::create_product::Params::new(lamp)).await.unwrap();
    let ops::create_product::Response::Forbidden(error) = refused else { panic!("a partner added a product") };
    assert_eq!(error, types::Error::new("only staff members can add products".to_owned()));
    let missing = client.get_product(ops::get_product::Params::new(0)).await.unwrap();
    let ops::get_product::Response::NotFound(error) = missing else { panic!("the product exists") };
    assert_eq!(error, types::Error::new("no such product".to_owned()));
}

/// The shop takes the API key of the staff that it is given and no longer the key of the example.
#[tokio::test]
async fn the_shop_takes_the_key_it_is_given() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = api::Server::new(Catalog::default().with_api_key("s3cret".to_owned())).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let get = |key: &'static str| {
        let client = api::Client::new(honig::ReqwestTransport::new(&base_url).unwrap()).with_api_key(key);
        async move { client.get_product(ops::get_product::Params::new(0)).await }
    };
    assert!(matches!(get("s3cret").await, Ok(ops::get_product::Response::NotFound(_))));
    assert!(matches!(get(shop::API_KEY).await, Err(api::ClientError::UnexpectedStatus { .. })));
}

/// The first line that `serve` prints, with `SHOP_API_KEY` set to `key` or not set.
fn first_line_of_serve(key: Option<&str>) -> String {
    use std::io::BufRead;
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_shop"));
    command.args(["--addr", "127.0.0.1:0", "serve"]).env_remove("SHOP_API_KEY").stdout(std::process::Stdio::piped());
    if let Some(key) = key {
        command.env("SHOP_API_KEY", key);
    }
    let mut server = command.spawn().unwrap();
    let mut line = String::new();
    std::io::BufReader::new(server.stdout.take().unwrap()).read_line(&mut line).unwrap();
    server.kill().unwrap();
    server.wait().unwrap();
    line
}

/// Without `SHOP_API_KEY` the server says the key that it takes. With it the server does not print the key.
#[test]
fn serve_prints_the_key_only_when_none_is_set() {
    assert_eq!(first_line_of_serve(None), "API key is 'secret'\n");
    assert!(first_line_of_serve(Some("s3cret")).starts_with("serving the catalog on "));
}

/// `add` refuses an argument that describes the other kind of product, before it sends anything.
#[test]
fn add_refuses_an_argument_of_the_other_kind() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_shop"))
        .args(["add", "E-book", "--kind", "digital", "--sku", "LMP-0001"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("--sku and --fragile describe a physical product"), "{err}");
}
