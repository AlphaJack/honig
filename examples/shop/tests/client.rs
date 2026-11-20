//! The generated client against the server of `main.rs`: it creates a product and reads it back.

use shop::Catalog;
use shop::api::{self, operations as ops, types};

// ANCHOR: client
async fn create_and_read(base_url: &str, product: types::Product) -> Result<types::Product, api::Error> {
    let client = api::Client::new(api::ReqwestTransport::new(base_url)).with_api_key(shop::API_KEY);
    client.create_product(ops::create_product::Params::new(product)).await?;
    match client.get_product(ops::get_product::Params::new(0)).await? {
        ops::get_product::Response::Ok(product) => Ok(product),
        ops::get_product::Response::NotFound(error) => panic!("{}", error.message),
    }
}
// ANCHOR_END: client

#[tokio::test]
async fn the_client_creates_and_reads_a_product_from_the_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = api::Server::new(Catalog::default()).authenticate(shop::check_key).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let lamp = types::Product::Physical(types::Physical::new(types::PhysicalKind, "Desk lamp".to_owned()));
    assert_eq!(create_and_read(&base_url, lamp.clone()).await.unwrap(), lamp);
}

/// The server takes only the shop's key: a request with another one is refused.
#[tokio::test]
async fn a_wrong_key_is_refused() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let app = api::Server::new(Catalog::default()).authenticate(shop::check_key).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let client = api::Client::new(api::ReqwestTransport::new(base_url)).with_api_key("guess");
    let answer = client.get_product(ops::get_product::Params::new(0)).await;
    assert!(
        matches!(&answer, Err(api::Error::UnexpectedStatus { status, .. }) if status.as_u16() == 401),
        "{answer:?}"
    );
}
