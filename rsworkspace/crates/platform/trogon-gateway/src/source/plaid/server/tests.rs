use super::*;
use crate::source::plaid::test_support::{FakeFailure, FakeKeyFetcher, TestKeyPair, hs256_token};
use crate::source::plaid::{PlaidApiBaseUrl, PlaidClientId, PlaidSecret};
use axum::body::Body;
use axum::http::Request;
use tower::ServiceExt;
use tracing_subscriber::util::SubscriberInitExt;
use trogon_nats::jetstream::StreamMaxAge;
use trogon_nats::jetstream::{ClaimBucket, ClaimBucketBinding};
use trogon_nats::jetstream::{
    ClaimCheckPublisher, MaxPayload, MockJetStreamContext, MockJetStreamPublisher, MockObjectStore,
};
use trogon_std::time::FixedEpochClock;

const TEST_NOW: u64 = 1_700_000_000;
const KEY_ID: &str = "kid-1";
const WEBHOOK_BODY: &[u8] = br#"{"webhook_type":"TRANSACTIONS","webhook_code":"SYNC_UPDATES_AVAILABLE","item_id":"item-123","environment":"production"}"#;

fn wrap_publisher(publisher: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        publisher,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

fn test_config() -> PlaidConfig {
    PlaidConfig {
        client_id: PlaidClientId::new("client-1").unwrap(),
        secret: PlaidSecret::new("secret-1").unwrap(),
        api_base_url: PlaidApiBaseUrl::new("https://sandbox.plaid.com").unwrap(),
        subject_prefix: NatsToken::new("plaid").unwrap(),
        stream_name: NatsToken::new("PLAID").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        timestamp_tolerance: NonZeroDuration::from_secs(300).unwrap(),
    }
}

fn tracing_guard() -> tracing::subscriber::DefaultGuard {
    tracing_subscriber::fmt().with_test_writer().set_default()
}

fn app(publisher: MockJetStreamPublisher, fetcher: FakeKeyFetcher) -> Router {
    router_with_clock(
        wrap_publisher(publisher),
        &test_config(),
        FixedEpochClock::from_secs(TEST_NOW),
        fetcher,
    )
}

fn fixture() -> (TestKeyPair, FakeKeyFetcher) {
    let pair = TestKeyPair::generate(KEY_ID);
    let fetcher = FakeKeyFetcher::default().with_key(KEY_ID, pair.verification_key(None));
    (pair, fetcher)
}

fn request(body: &[u8], token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("POST").uri("/webhook");
    if let Some(token) = token {
        builder = builder.header(HEADER_VERIFICATION, token);
    }
    builder.body(Body::from(body.to_vec())).unwrap()
}

fn signed_request(pair: &TestKeyPair, body: &[u8]) -> Request<Body> {
    request(body, Some(&pair.token(TEST_NOW, body)))
}

fn header<'a>(headers: &'a async_nats::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).map(|v| v.as_str())
}

fn assert_unroutable(publisher: &MockJetStreamPublisher, expected_reason: &str, body: &[u8], token: &str) {
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1, "expected exactly one unroutable publish");
    assert_eq!(messages[0].subject, "plaid.unroutable");
    assert_eq!(
        header(&messages[0].headers, NATS_HEADER_REJECT_REASON),
        Some(expected_reason)
    );
    assert_eq!(
        header(&messages[0].headers, "Nats-Msg-Id"),
        Some(PlaidDeliveryId::of(token).as_str())
    );
    assert_eq!(messages[0].payload.as_ref(), body);
}

#[tokio::test]
async fn provision_creates_stream_with_dedup_window_spanning_both_sides_of_the_tolerance() {
    let _guard = tracing_guard();
    let js = MockJetStreamContext::new();

    provision(&js, &test_config()).await.unwrap();

    let streams = js.created_streams();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0].name, "PLAID");
    assert_eq!(streams[0].subjects, vec!["plaid.>"]);
    assert_eq!(streams[0].max_age, Duration::from_secs(3600));
    assert_eq!(streams[0].duplicate_window, Duration::from_secs(600));
}

#[tokio::test]
async fn valid_webhook_publishes_raw_body_with_headers_and_delivery_msg_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let token = pair.token(TEST_NOW, WEBHOOK_BODY);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(WEBHOOK_BODY, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "plaid.TRANSACTIONS.SYNC_UPDATES_AVAILABLE");
    assert_eq!(messages[0].payload.as_ref(), WEBHOOK_BODY);
    assert_eq!(
        header(&messages[0].headers, NATS_HEADER_EVENT_TYPE),
        Some("TRANSACTIONS.SYNC_UPDATES_AVAILABLE")
    );
    assert_eq!(header(&messages[0].headers, NATS_HEADER_ITEM_ID), Some("item-123"));
    assert_eq!(
        header(&messages[0].headers, "Nats-Msg-Id"),
        Some(PlaidDeliveryId::of(&token).as_str())
    );
}

#[tokio::test]
async fn static_mode_fetches_the_key_with_the_configured_secret() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();

    let resp = app(publisher.clone(), fetcher.clone())
        .oneshot(signed_request(&pair, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(fetcher.secrets(), vec!["secret-1"]);
}

#[tokio::test]
async fn item_id_header_is_omitted_when_the_payload_has_none() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = br#"{"webhook_type":"ITEM","webhook_code":"ERROR"}"#;

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, body))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages[0].subject, "plaid.ITEM.ERROR");
    assert!(header(&messages[0].headers, NATS_HEADER_ITEM_ID).is_none());
}

#[tokio::test]
async fn invalid_item_id_is_dropped_but_the_event_still_publishes() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = br#"{"webhook_type":"ITEM","webhook_code":"ERROR","item_id":"has space"}"#;

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, body))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 1);
    assert!(header(&messages[0].headers, NATS_HEADER_ITEM_ID).is_none());
}

#[tokio::test]
async fn replays_of_one_signed_request_share_a_msg_id() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let router = app(publisher.clone(), fetcher.clone());
    let token = pair.token(TEST_NOW, WEBHOOK_BODY);

    for _ in 0..2 {
        let resp = router
            .clone()
            .oneshot(request(WEBHOOK_BODY, Some(&token)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        header(&messages[0].headers, "Nats-Msg-Id"),
        header(&messages[1].headers, "Nats-Msg-Id")
    );
    assert_eq!(fetcher.calls(), vec![KEY_ID]);
}

#[tokio::test]
async fn deliveries_with_identical_bodies_get_distinct_msg_ids() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let router = app(publisher.clone(), fetcher);
    let first = pair.token(TEST_NOW - 60, WEBHOOK_BODY);
    let second = pair.token(TEST_NOW, WEBHOOK_BODY);

    for token in [&first, &second] {
        let resp = router
            .clone()
            .oneshot(request(WEBHOOK_BODY, Some(token)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let messages = publisher.published_messages();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        header(&messages[0].headers, "Nats-Msg-Id"),
        Some(PlaidDeliveryId::of(&first).as_str())
    );
    assert_eq!(
        header(&messages[1].headers, "Nats-Msg-Id"),
        Some(PlaidDeliveryId::of(&second).as_str())
    );
    assert_ne!(
        header(&messages[0].headers, "Nats-Msg-Id"),
        header(&messages[1].headers, "Nats-Msg-Id")
    );
}

#[tokio::test]
async fn missing_header_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (_, fetcher) = fixture();

    let resp = app(publisher.clone(), fetcher.clone())
        .oneshot(request(WEBHOOK_BODY, None))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
    assert!(fetcher.calls().is_empty());
}

#[tokio::test]
async fn malformed_token_returns_401_without_fetching() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (_, fetcher) = fixture();

    let resp = app(publisher.clone(), fetcher.clone())
        .oneshot(request(WEBHOOK_BODY, Some("not-a-jwt")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
    assert!(fetcher.calls().is_empty());
}

#[tokio::test]
async fn non_es256_token_returns_401_without_fetching() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (_, fetcher) = fixture();
    let token = hs256_token(KEY_ID, TEST_NOW, WEBHOOK_BODY);

    let resp = app(publisher.clone(), fetcher.clone())
        .oneshot(request(WEBHOOK_BODY, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
    assert!(fetcher.calls().is_empty());
}

#[tokio::test]
async fn token_without_key_id_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let token = pair.token_without_key_id(TEST_NOW, WEBHOOK_BODY);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(WEBHOOK_BODY, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn unknown_key_id_returns_503() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let stranger = TestKeyPair::generate("kid-unknown");

    let resp = app(publisher.clone(), FakeKeyFetcher::default())
        .oneshot(signed_request(&stranger, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn unknown_key_ids_past_the_fetch_limit_return_429_with_retry_after_without_fetching() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let fetcher = FakeKeyFetcher::default();
    let router = app(publisher.clone(), fetcher.clone());

    let fetched = router
        .clone()
        .oneshot(signed_request(&TestKeyPair::generate("kid-unknown-1"), WEBHOOK_BODY))
        .await
        .unwrap();
    let throttled = router
        .oneshot(signed_request(&TestKeyPair::generate("kid-unknown-2"), WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(fetched.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(throttled.headers().get(header::RETRY_AFTER).unwrap(), "10");
    assert_eq!(fetcher.calls(), vec!["kid-unknown-1"]);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn signature_from_a_different_key_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (_, fetcher) = fixture();
    let impostor = TestKeyPair::generate(KEY_ID);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&impostor, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn stale_issued_at_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let token = pair.token(TEST_NOW - 301, WEBHOOK_BODY);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(WEBHOOK_BODY, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn issued_at_at_the_tolerance_boundary_is_accepted() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let token = pair.token(TEST_NOW - 300, WEBHOOK_BODY);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(WEBHOOK_BODY, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(publisher.published_messages().len(), 1);
}

#[tokio::test]
async fn body_that_differs_from_the_signed_hash_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let token = pair.token(TEST_NOW, WEBHOOK_BODY);
    let tampered = br#"{"webhook_type":"TRANSACTIONS","webhook_code":"DEFAULT_UPDATE"}"#;

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(tampered, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn expired_key_returns_401() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let pair = TestKeyPair::generate(KEY_ID);
    let fetcher = FakeKeyFetcher::default().with_key(KEY_ID, pair.verification_key(Some(TEST_NOW - 1)));

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn key_fetch_transport_failure_returns_503() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, _) = fixture();
    let fetcher = FakeKeyFetcher::default().failing(FakeFailure::Transport);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn key_fetch_server_error_returns_503() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, _) = fixture();
    let fetcher = FakeKeyFetcher::default().failing(FakeFailure::Unavailable);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn key_fetch_rejection_returns_503() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, _) = fixture();
    let fetcher = FakeKeyFetcher::default().failing(FakeFailure::Rejected);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(publisher.published_messages().is_empty());
}

#[tokio::test]
async fn invalid_json_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = b"not json";
    let token = pair.token(TEST_NOW, body);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(body, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "invalid_json", body, &token);
}

#[tokio::test]
async fn missing_webhook_type_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = br#"{"webhook_code":"ERROR"}"#;
    let token = pair.token(TEST_NOW, body);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(body, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "missing_webhook_type", body, &token);
}

#[tokio::test]
async fn missing_webhook_code_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = br#"{"webhook_type":"ITEM"}"#;
    let token = pair.token(TEST_NOW, body);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(body, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "missing_webhook_code", body, &token);
}

#[tokio::test]
async fn non_string_webhook_type_publishes_unroutable() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = br#"{"webhook_type":7,"webhook_code":"ERROR"}"#;
    let token = pair.token(TEST_NOW, body);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(body, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "missing_webhook_type", body, &token);
}

#[tokio::test]
async fn invalid_event_type_publishes_unroutable_and_returns_ok() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = br#"{"webhook_type":"IT EM","webhook_code":"ERROR"}"#;
    let token = pair.token(TEST_NOW, body);

    let resp = app(publisher.clone(), fetcher)
        .oneshot(request(body, Some(&token)))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_unroutable(&publisher, "invalid_event_type", body, &token);
}

#[tokio::test]
async fn publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let (pair, fetcher) = fixture();

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, WEBHOOK_BODY))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn unroutable_publish_failure_returns_500() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    publisher.fail_next_js_publish();
    let (pair, fetcher) = fixture();

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, b"not json"))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let _guard = tracing_guard();
    let publisher = MockJetStreamPublisher::new();
    let (pair, fetcher) = fixture();
    let body = vec![b'a'; HTTP_BODY_SIZE_MAX.as_usize() + 1];

    let resp = app(publisher.clone(), fetcher)
        .oneshot(signed_request(&pair, &body))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert!(publisher.published_messages().is_empty());
}
