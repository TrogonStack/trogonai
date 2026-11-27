use super::*;
use twilight_http::request::TryIntoRequest;

#[tokio::test]
async fn a_reply_pings_nobody_it_mentions() {
    trogon_std::install_default_crypto_provider();
    let http = Client::new("token".to_string());
    let request = reply(&http, Id::new(1), "@everyone @here <@&2> <@3>")
        .try_into_request()
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(request.body().unwrap()).unwrap();

    assert_eq!(body["content"], "@everyone @here <@&2> <@3>");
    assert_eq!(body["allowed_mentions"], serde_json::json!({ "parse": [] }));
}
