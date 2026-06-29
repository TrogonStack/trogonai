use async_nats::{HeaderMap, Subject};
use bytes::Bytes;
use trogon_nats::mocks::MockNatsClient;

use super::*;

#[tokio::test]
async fn reply_error_publishes_body_and_headers_to_reply_subject() {
    let nats = MockNatsClient::new();
    let mut headers = HeaderMap::new();
    headers.insert("Jsonrpc-Error-Code", "-32801");
    let body = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"r","error":{"code":-32801,"message":"denied"}}"#);
    reply_error(&nats, Subject::from("_INBOX.reply"), headers.clone(), body.clone()).await;
    let subjects = nats.published_messages();
    assert_eq!(subjects, vec!["_INBOX.reply".to_owned()]);
    let payloads = nats.published_payloads();
    assert_eq!(payloads, vec![body]);
    let published_headers = nats.published_headers();
    assert_eq!(published_headers.len(), 1);
    assert_eq!(
        published_headers[0].get("Jsonrpc-Error-Code").map(|v| v.as_str()),
        Some("-32801"),
    );
}
