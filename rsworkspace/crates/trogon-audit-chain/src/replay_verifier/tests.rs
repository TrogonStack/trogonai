use std::future::IntoFuture;

use super::*;
use async_nats::HeaderMap;
use async_nats::jetstream::message::OutboundMessage;
use trogon_nats::jetstream::{JetStreamPublishMessage, MockJetStreamPublishMessage};

use crate::chain_hash::ChainHash;
use crate::chain_headers::write_chain_headers;
use crate::extend::extend;

fn event(json: &str) -> CanonicalEventBytes {
    CanonicalEventBytes::from_json_slice(json.as_bytes()).expect("valid json fixture")
}

async fn publish_chained(mock: &MockJetStreamPublishMessage, tip: &mut ChainHash, json: &str) {
    let event_bytes = event(json);
    let hash = extend(tip, &event_bytes);

    let mut headers = HeaderMap::new();
    write_chain_headers(&mut headers, tip, &hash);

    let outbound = OutboundMessage {
        subject: "audit.chain".into(),
        headers: Some(headers),
        payload: bytes::Bytes::copy_from_slice(event_bytes.as_bytes()),
    };

    mock.publish_message(outbound)
        .await
        .expect("mock publish should succeed")
        .into_future()
        .await
        .expect("mock ack should succeed");
    *tip = hash;
}

#[tokio::test]
async fn verifies_valid_replayed_chain() {
    let mock = MockJetStreamPublishMessage::new();
    let mut tip = ChainHash::genesis();
    publish_chained(&mock, &mut tip, r#"{"a":1}"#).await;
    publish_chained(&mock, &mut tip, r#"{"a":2}"#).await;
    publish_chained(&mock, &mut tip, r#"{"a":3}"#).await;

    let result = verify_stream_chain(&mock).await;
    assert!(result.is_ok(), "expected chain to verify, got {result:?}");
}

#[tokio::test]
async fn empty_stream_verifies() {
    let mock = MockJetStreamPublishMessage::new();
    let result = verify_stream_chain(&mock).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn detects_missing_chain_headers() {
    let mock = MockJetStreamPublishMessage::new();
    let outbound = OutboundMessage {
        subject: "audit.chain".into(),
        headers: None,
        payload: bytes::Bytes::from_static(br#"{"a":1}"#),
    };
    mock.publish_message(outbound)
        .await
        .expect("mock publish should succeed")
        .into_future()
        .await
        .expect("mock ack should succeed");

    let result = verify_stream_chain(&mock).await;
    assert!(matches!(result, Err(ReplayVerifyError::Header { .. })));
}

#[tokio::test]
async fn detects_tampered_payload_after_replay() {
    let mock = MockJetStreamPublishMessage::new();
    let mut tip = ChainHash::genesis();
    publish_chained(&mock, &mut tip, r#"{"a":1}"#).await;
    publish_chained(&mock, &mut tip, r#"{"a":2}"#).await;

    // Simulate a rewrite of the stored payload for sequence 1 without
    // recomputing its chain headers, by re-publishing a fresh mock seeded
    // with a hand-crafted mismatched message at that position.
    let tampered_mock = MockJetStreamPublishMessage::new();
    let genesis = ChainHash::genesis();
    let real_hash_1 = extend(&genesis, &event(r#"{"a":1}"#));
    let mut headers = HeaderMap::new();
    write_chain_headers(&mut headers, &genesis, &real_hash_1);
    let tampered_outbound = OutboundMessage {
        subject: "audit.chain".into(),
        headers: Some(headers),
        payload: bytes::Bytes::from_static(br#"{"a":999}"#),
    };
    tampered_mock
        .publish_message(tampered_outbound)
        .await
        .expect("mock publish should succeed")
        .into_future()
        .await
        .expect("mock ack should succeed");

    let result = verify_stream_chain(&tampered_mock).await;
    assert!(matches!(
        result,
        Err(ReplayVerifyError::Break(ChainBreak::HashMismatch { .. }))
    ));
}

#[tokio::test]
async fn detects_forged_previous_hash_after_replay() {
    let mock = MockJetStreamPublishMessage::new();
    let genesis = ChainHash::genesis();
    let forged_previous = ChainHash::digest(b"forged");
    let event_bytes = event(r#"{"a":1}"#);
    let forged_hash = extend(&forged_previous, &event_bytes);

    let mut headers = HeaderMap::new();
    write_chain_headers(&mut headers, &forged_previous, &forged_hash);
    let outbound = OutboundMessage {
        subject: "audit.chain".into(),
        headers: Some(headers),
        payload: bytes::Bytes::copy_from_slice(event_bytes.as_bytes()),
    };
    mock.publish_message(outbound)
        .await
        .expect("mock publish should succeed")
        .into_future()
        .await
        .expect("mock ack should succeed");

    let result = verify_stream_chain(&mock).await;
    assert!(matches!(
        result,
        Err(ReplayVerifyError::Break(ChainBreak::PreviousHashMismatch { .. }))
    ));
    let _ = genesis;
}
