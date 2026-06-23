use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::jetstream::mocks::MockJetStreamPublisher;
    use crate::jetstream::mocks::MockObjectStore;

    #[derive(Clone)]
    struct DynamicMaxPayload {
        server_limit: Arc<AtomicUsize>,
    }

    impl DynamicMaxPayload {
        fn new(server_limit: usize) -> Self {
            Self {
                server_limit: Arc::new(AtomicUsize::new(server_limit)),
            }
        }

        fn set_server_limit(&self, server_limit: usize) {
            self.server_limit.store(server_limit, Ordering::Relaxed);
        }
    }

    impl MaxPayloadLimit for DynamicMaxPayload {
        fn max_payload(&self) -> MaxPayload {
            MaxPayload::from_server_limit(self.server_limit.load(Ordering::Relaxed))
        }
    }

    #[tokio::test]
    async fn small_payload_publishes_directly() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 512]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].payload.len(), 512);
        assert!(store.stored_objects().is_empty());
    }

    #[tokio::test]
    async fn large_payload_stores_in_object_store_and_publishes_claim() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 2048]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].payload.is_empty());
        assert_eq!(
            messages[0].headers.get(HEADER_CLAIM_CHECK).unwrap().as_str(),
            CLAIM_CHECK_VERSION
        );
        assert_eq!(
            messages[0].headers.get(HEADER_CLAIM_BUCKET).unwrap().as_str(),
            "test-bucket"
        );
        let key = messages[0].headers.get(HEADER_CLAIM_KEY).unwrap().as_str().to_string();
        assert!(key.starts_with("test.subject/"));

        let stored = store.stored_objects();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].0, key);
        assert_eq!(stored[0].1.len(), 2048);
    }

    #[tokio::test]
    async fn payload_at_exact_threshold_publishes_directly() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 1024]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        assert_eq!(publisher.published_messages()[0].payload.len(), 1024);
        assert!(store.stored_objects().is_empty());
    }

    #[tokio::test]
    async fn publish_uses_current_max_payload_limit() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let max_payload = DynamicMaxPayload::new(1024 + PROTOCOL_OVERHEAD);
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            max_payload.clone(),
        );

        let direct = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 512]),
                Duration::from_secs(5),
            )
            .await;
        assert!(direct.is_ok());

        max_payload.set_server_limit(256 + PROTOCOL_OVERHEAD);
        let claimed = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 512]),
                Duration::from_secs(5),
            )
            .await;
        assert!(claimed.is_ok());

        let messages = publisher.published_messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].payload.len(), 512);
        assert!(messages[1].payload.is_empty());
        assert_eq!(store.stored_objects().len(), 1);
    }

    #[tokio::test]
    async fn object_store_failure_returns_store_failed() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        store.fail_next_put();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store,
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                HeaderMap::new(),
                Bytes::from(vec![0u8; 2048]),
                Duration::from_secs(5),
            )
            .await;

        assert!(!outcome.is_ok());
        assert!(publisher.published_messages().is_empty());
    }

    #[tokio::test]
    async fn large_payload_preserves_original_headers() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store,
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let mut headers = HeaderMap::new();
        headers.insert("X-Custom", "value");

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                headers,
                Bytes::from(vec![0u8; 2048]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        let msg = &publisher.published_messages()[0];
        assert_eq!(msg.headers.get("X-Custom").unwrap().as_str(), "value");
        assert!(msg.headers.get(HEADER_CLAIM_CHECK).is_some());
    }

    #[tokio::test]
    async fn resolve_claim_returns_payload_when_not_a_claim() {
        let store = MockObjectStore::new();
        let headers = HeaderMap::new();
        let payload = Bytes::from("raw data");

        let result = resolve_claim(&headers, payload.clone(), &store).await;
        assert_eq!(result.unwrap(), payload);
    }

    #[tokio::test]
    async fn resolve_claim_fetches_from_store() {
        let store = MockObjectStore::new();
        let expected = Bytes::from("large payload data");
        store.seed("test.subject/some-id", expected.clone());

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        headers.insert(HEADER_CLAIM_BUCKET, "test-bucket");
        headers.insert(HEADER_CLAIM_KEY, "test.subject/some-id");

        let result = resolve_claim(&headers, Bytes::new(), &store).await;
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn resolve_claim_errors_on_missing_key_header() {
        let store = MockObjectStore::new();
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);

        let result = resolve_claim(&headers, Bytes::new(), &store).await;
        assert!(matches!(result, Err(ClaimResolveError::MissingKey)));
    }

    #[tokio::test]
    async fn small_payload_strips_claim_headers() {
        let publisher = MockJetStreamPublisher::new();
        let store = MockObjectStore::new();
        let cc = ClaimCheckPublisher::new(
            publisher.clone(),
            store.clone(),
            "test-bucket".to_string(),
            MaxPayload::from_server_limit(1024 + PROTOCOL_OVERHEAD),
        );

        let mut headers = HeaderMap::new();
        headers.insert("X-Custom", "keep");
        headers.insert(HEADER_CLAIM_CHECK, CLAIM_CHECK_VERSION);
        headers.insert(HEADER_CLAIM_BUCKET, "rogue-bucket");
        headers.insert(HEADER_CLAIM_KEY, "rogue-key");

        let outcome = cc
            .publish_event(
                "test.subject".to_string(),
                headers,
                Bytes::from(vec![0u8; 512]),
                Duration::from_secs(5),
            )
            .await;

        assert!(outcome.is_ok());
        let msg = &publisher.published_messages()[0];
        assert_eq!(msg.headers.get("X-Custom").unwrap().as_str(), "keep");
        assert!(msg.headers.get(HEADER_CLAIM_CHECK).is_none());
        assert!(msg.headers.get(HEADER_CLAIM_BUCKET).is_none());
        assert!(msg.headers.get(HEADER_CLAIM_KEY).is_none());
        assert!(store.stored_objects().is_empty());
    }
