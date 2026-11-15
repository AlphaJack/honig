//! Response links: typed structs that fill the linked operation's parameters from a request and its response.

use e2e::v32_shop::api::operations as ops;

#[test]
fn create_product_links_carry_the_new_id() {
    // ANCHOR: links
    let request = ops::create_product::Params { body: None, ..Default::default() };
    let body = serde_json::json!({"id": 7, "name": "Desk lamp"});
    let update = ops::create_product::links::UpdateProduct::new(&request, &body);
    assert_eq!(update.product_id, Some(7));
    // ANCHOR_END: links
    let get = ops::create_product::links::GetProduct::new(&request, &body);
    assert_eq!(get.product_id, Some(7));
}

#[test]
fn list_links_read_pointers_requests_and_constants() {
    let request = ops::get_all_data::Params { limit: 25, offset: 50, ..Default::default() };
    let body = serde_json::json!({"data": [{"id": 3}]});
    let first = ops::get_all_data::links::GetFirstProduct::new(&request, &body);
    assert_eq!(first.product_id, Some(3));
    let page = ops::get_all_data::links::FirstPage::new(&request, &body);
    assert_eq!((page.limit, page.offset), (Some(25), Some(0)));
}

#[test]
fn missing_values_stay_none() {
    let request = ops::get_all_data::Params::default();
    let first = ops::get_all_data::links::GetFirstProduct::new(&request, &serde_json::json!({"data": []}));
    assert_eq!(first.product_id, None);
}

#[test]
fn mixed_multipart_sends_one_part_per_item() {
    use e2e::v32_shop::api::{self, types};
    let meta = types::PhotoBatchMeta { photographer: "Studio North".to_owned(), taken_at: None, count: None };
    let body = (meta, vec![api::Binary::from(vec![0xff, 0xd8, 0x00]), api::Binary::from(vec![1, 2])]);
    let types = [("0", "application/json"), ("*", "image/jpeg"), ("prefix", "1")];
    let (ct, bytes) = api::runtime::mixed_encode(&body, "multipart/mixed", &types).unwrap_or_else(|e| panic!("{e}"));
    assert!(ct.starts_with("multipart/mixed; boundary="));
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("Content-Type: image/jpeg").count(), 2, "{text}");
    assert_eq!(text.matches("Content-Type: application/json").count(), 1, "{text}");
    assert!(text.contains(&serde_json::json!({"photographer": "Studio North"}).to_string()), "{text}");
    assert!(bytes.windows(3).any(|w| w == [0xff, 0xd8, 0x00]), "raw image bytes, not base64");
    let back: (types::PhotoBatchMeta, Vec<api::Binary>) =
        api::runtime::mixed_decode(200, &ct, &bytes, Some(1)).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(back, body);
}
