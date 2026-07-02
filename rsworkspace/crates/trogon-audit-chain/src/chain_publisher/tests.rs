use super::*;
use trogon_nats::jetstream::MockJetStreamPublisher;

fn event(json: &str) -> CanonicalEventBytes {
    CanonicalEventBytes::from_json_slice(json.as_bytes()).expect("valid json fixture")
}

#[tokio::test]
async fn first_publish_chains_from_genesis() {
    let mock = MockJetStreamPublisher::new();
    let publisher = ChainPublisher::new(mock.clone());

    publisher
        .publish("audit.subject", HeaderMap::new(), event(r#"{"a":1}"#))
        .await
        .expect("publish should succeed");

    let published = mock.published_messages();
    assert_eq!(published.len(), 1);
    let headers = &published[0].headers;
    assert_eq!(
        headers
            .get(crate::chain_headers::CHAIN_PREVIOUS_HASH_HEADER)
            .map(|v| v.as_str()),
        Some(ChainHash::genesis().as_str())
    );
    let expected_hash = extend(&ChainHash::genesis(), &event(r#"{"a":1}"#));
    assert_eq!(
        headers.get(crate::chain_headers::CHAIN_HASH_HEADER).map(|v| v.as_str()),
        Some(expected_hash.as_str())
    );
}

#[tokio::test]
async fn second_publish_chains_from_first_hash() {
    let mock = MockJetStreamPublisher::new();
    let publisher = ChainPublisher::new(mock.clone());

    publisher
        .publish("audit.subject", HeaderMap::new(), event(r#"{"a":1}"#))
        .await
        .expect("first publish should succeed");
    publisher
        .publish("audit.subject", HeaderMap::new(), event(r#"{"a":2}"#))
        .await
        .expect("second publish should succeed");

    let published = mock.published_messages();
    assert_eq!(published.len(), 2);

    let first_hash = extend(&ChainHash::genesis(), &event(r#"{"a":1}"#));
    let second_headers = &published[1].headers;
    assert_eq!(
        second_headers
            .get(crate::chain_headers::CHAIN_PREVIOUS_HASH_HEADER)
            .map(|v| v.as_str()),
        Some(first_hash.as_str())
    );
}

#[tokio::test]
async fn current_tip_advances_after_successful_publish() {
    let mock = MockJetStreamPublisher::new();
    let publisher = ChainPublisher::new(mock);

    assert_eq!(publisher.current_tip(), ChainHash::genesis());

    publisher
        .publish("audit.subject", HeaderMap::new(), event(r#"{"a":1}"#))
        .await
        .expect("publish should succeed");

    let expected_hash = extend(&ChainHash::genesis(), &event(r#"{"a":1}"#));
    assert_eq!(publisher.current_tip(), expected_hash);
}

#[tokio::test]
async fn tip_does_not_advance_when_publish_fails() {
    let mock = MockJetStreamPublisher::new();
    mock.fail_next_js_publish();
    let publisher = ChainPublisher::new(mock);

    let result = publisher
        .publish("audit.subject", HeaderMap::new(), event(r#"{"a":1}"#))
        .await;
    assert!(result.is_err());
    assert_eq!(publisher.current_tip(), ChainHash::genesis());
}

#[tokio::test]
async fn resume_starts_chain_from_supplied_tip() {
    let mock = MockJetStreamPublisher::new();
    let resumed_tip = ChainHash::digest(b"resumed");
    let publisher = ChainPublisher::resume(mock.clone(), resumed_tip.clone());

    publisher
        .publish("audit.subject", HeaderMap::new(), event(r#"{"a":1}"#))
        .await
        .expect("publish should succeed");

    let published = mock.published_messages();
    let headers = &published[0].headers;
    assert_eq!(
        headers
            .get(crate::chain_headers::CHAIN_PREVIOUS_HASH_HEADER)
            .map(|v| v.as_str()),
        Some(resumed_tip.as_str())
    );
}

#[tokio::test]
async fn caller_supplied_headers_are_preserved_alongside_chain_headers() {
    let mock = MockJetStreamPublisher::new();
    let publisher = ChainPublisher::new(mock.clone());

    let mut headers = HeaderMap::new();
    headers.insert("Trogon-Event-Type", "agent.decision.recorded");

    publisher
        .publish("audit.subject", headers, event(r#"{"a":1}"#))
        .await
        .expect("publish should succeed");

    let published = mock.published_messages();
    let headers = &published[0].headers;
    assert_eq!(
        headers.get("Trogon-Event-Type").map(|v| v.as_str()),
        Some("agent.decision.recorded")
    );
    assert!(headers.get(crate::chain_headers::CHAIN_HASH_HEADER).is_some());
}
