use async_nats::{HeaderMap, Subject};
use bytes::Bytes;
use trogon_nats::mocks::{AdvancedMockNatsClient, MockNatsClient};

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

#[tokio::test]
async fn reply_error_swallows_publish_failure_so_dispatch_can_continue() {
    // A NATS publish failure from `reply_error` MUST stay on the
    // tracing channel rather than propagate, because the dispatch
    // path has nothing actionable to do with it: the caller has
    // already gone (or will time out) and other ingress requests
    // need to keep being served.
    let nats = AdvancedMockNatsClient::new();
    nats.fail_next_publish();
    let body = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"r","error":{"code":-1,"message":"x"}}"#);
    reply_error(&nats, Subject::from("_INBOX.reply"), HeaderMap::new(), body).await;
    // No assertion on tracing -- the contract is "function returns
    // without panicking on a publish failure". The fact that we
    // got here at all is the assertion.
    assert!(nats.published_messages().is_empty());
}
