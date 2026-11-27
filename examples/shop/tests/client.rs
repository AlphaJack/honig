//! The generated client against the server of `main.rs`: it creates a product and fetches it back.

use shop::Catalog;
use shop::api::{self, operations as ops, types};

// ANCHOR: client
async fn create_and_read(base_url: &str, product: types::Product) -> Result<types::Product, api::ClientError> {
    let client =
        api::Client::new(honig::ReqwestTransport::new(base_url).expect("a base URL")).with_api_key(shop::API_KEY);
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
    let app = api::Server::new(Catalog::default()).into_axum();
    tokio::spawn(async move { axum::serve(listener, app).await });
    let lamp = types::Product::Physical(types::Physical::new("Desk lamp".to_owned()));
    assert_eq!(create_and_read(&base_url, lamp.clone()).await.unwrap(), lamp);
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
    let client = api::Client::new(honig::ReqwestTransport::new(base_url).unwrap()).with_api_key(shop::API_KEY);
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
