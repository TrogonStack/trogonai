    use super::*;
    use crate::catalog::import_gate::AllowAllImportGate;
    use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

    fn agent_id(s: &str) -> A2aAgentId {
        A2aAgentId::new(s).unwrap()
    }

    fn card(name: &str) -> a2a::agent_card::AgentCard {
        a2a::agent_card::AgentCard {
            name: name.to_string(),
            description: String::new(),
            version: String::new(),
            supported_interfaces: vec![a2a::agent_card::AgentInterface {
                url: "https://example.com/a2a".to_string(),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.2.0".to_string(),
                tenant: None,
            }],
            capabilities: a2a::agent_card::AgentCapabilities::default(),
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
        }
    }

    #[tokio::test]
    async fn put_creates_when_entry_absent() {
        let kv = MockJetStreamKvStore::new();
        let store = KvCatalogStore::new(kv.clone());
        store.put_card(&agent_id("bot"), &card("bot")).await.unwrap();
        let calls = kv.create_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "bot");
    }

    #[tokio::test]
    async fn put_updates_when_entry_present() {
        use async_nats::jetstream::kv::Operation;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 3, Operation::Put);
        let store = KvCatalogStore::new(kv.clone());
        store.put_card(&agent_id("bot"), &card("bot-v2")).await.unwrap();
        let updates = kv.update_calls();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].0, "bot");
        assert_eq!(updates[0].2, 3);
    }

    #[tokio::test]
    async fn put_creates_when_latest_entry_is_a_delete_tombstone() {
        use async_nats::jetstream::kv::Operation;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(bytes::Bytes::new(), 7, Operation::Delete);
        let store = KvCatalogStore::new(kv.clone());
        store.put_card(&agent_id("bot"), &card("bot")).await.unwrap();
        assert_eq!(kv.update_calls().len(), 0, "must not update against a delete tombstone");
        let creates = kv.create_calls();
        assert_eq!(creates.len(), 1);
        assert_eq!(creates[0].0, "bot");
    }

    #[tokio::test]
    async fn put_creates_when_latest_entry_is_a_purge_tombstone() {
        use async_nats::jetstream::kv::Operation;
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_entry(bytes::Bytes::new(), 9, Operation::Purge);
        let store = KvCatalogStore::new(kv.clone());
        store.put_card(&agent_id("bot"), &card("bot")).await.unwrap();
        assert_eq!(kv.update_calls().len(), 0);
        assert_eq!(kv.create_calls().len(), 1);
    }

    #[tokio::test]
    async fn put_retries_when_concurrent_writer_races_create() {
        use async_nats::jetstream::kv;

        let kv_store = MockJetStreamKvStore::new();
        // Two attempts: first sees no entry → create races and loses (AlreadyExists);
        // second sees a Put entry at revision 5 → update succeeds.
        kv_store.enqueue_entry_none();
        kv_store.enqueue_create_result(Err(kv::CreateErrorKind::AlreadyExists));
        kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 5, kv::Operation::Put);
        kv_store.enqueue_update_result(Ok(6));

        let store = KvCatalogStore::new(kv_store.clone());
        store.put_card(&agent_id("bot"), &card("bot")).await.unwrap();
        assert_eq!(kv_store.create_calls().len(), 1);
        assert_eq!(kv_store.update_calls().len(), 1);
    }

    #[tokio::test]
    async fn put_retries_when_concurrent_writer_races_update() {
        use async_nats::jetstream::kv;

        let kv_store = MockJetStreamKvStore::new();
        kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 3, kv::Operation::Put);
        kv_store.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
        kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 4, kv::Operation::Put);
        kv_store.enqueue_update_result(Ok(5));

        let store = KvCatalogStore::new(kv_store.clone());
        store.put_card(&agent_id("bot"), &card("bot")).await.unwrap();
        assert_eq!(kv_store.update_calls().len(), 2);
        assert_eq!(kv_store.update_calls()[1].2, 4);
    }

    #[tokio::test]
    async fn put_propagates_non_conflict_update_error_without_retry() {
        use async_nats::jetstream::kv;
        let kv_store = MockJetStreamKvStore::new();
        kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 3, kv::Operation::Put);
        kv_store.enqueue_update_result(Err(kv::UpdateErrorKind::TimedOut));
        let store = KvCatalogStore::new(kv_store.clone());
        let err = store.put_card(&agent_id("bot"), &card("bot")).await.unwrap_err();
        assert!(matches!(err, CatalogStoreError::Kv(_)));
        assert_eq!(kv_store.update_calls().len(), 1, "fatal update errors must not retry");
    }

    #[tokio::test]
    async fn put_propagates_non_conflict_create_error_without_retry() {
        use async_nats::jetstream::kv;
        let kv_store = MockJetStreamKvStore::new();
        kv_store.enqueue_create_result(Err(kv::CreateErrorKind::InvalidKey));
        let store = KvCatalogStore::new(kv_store.clone());
        let err = store.put_card(&agent_id("bot"), &card("bot")).await.unwrap_err();
        assert!(matches!(err, CatalogStoreError::Kv(_)));
        assert_eq!(kv_store.create_calls().len(), 1, "fatal create errors must not retry");
    }

    #[tokio::test]
    async fn put_gives_up_when_revision_race_persists_past_retry_budget() {
        use async_nats::jetstream::kv;

        let kv_store = MockJetStreamKvStore::new();
        for _ in 0..3 {
            kv_store.enqueue_entry(bytes::Bytes::from(b"{}".to_vec()), 1, kv::Operation::Put);
            kv_store.enqueue_update_result(Err(kv::UpdateErrorKind::WrongLastRevision));
        }

        let store = KvCatalogStore::new(kv_store.clone());
        let err = store.put_card(&agent_id("bot"), &card("bot")).await.unwrap_err();
        assert!(matches!(err, CatalogStoreError::Kv(_)));
        assert_eq!(kv_store.update_calls().len(), 3);
    }

    #[tokio::test]
    async fn get_returns_none_when_absent() {
        let kv = MockJetStreamKvStore::new();
        let store = KvCatalogStore::new(kv);
        let result = store.get_card(&agent_id("missing")).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_returns_card_when_present() {
        let kv = MockJetStreamKvStore::new();
        let c = card("planner");
        let bytes = serde_json::to_vec(&c).unwrap();
        kv.enqueue_get_some(bytes::Bytes::from(bytes));
        let store = KvCatalogStore::new(kv);
        let result = store.get_card(&agent_id("planner")).await.unwrap();
        assert_eq!(result.unwrap().name, "planner");
    }

    #[tokio::test]
    async fn get_returns_deserialize_error_on_bad_bytes() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_get_some(bytes::Bytes::from(b"not-json".to_vec()));
        let store = KvCatalogStore::new(kv);
        let err = store.get_card(&agent_id("bot")).await.unwrap_err();
        assert!(matches!(err, CatalogStoreError::Deserialize(_)));
    }

    #[tokio::test]
    async fn get_returns_schema_error_when_json_object_missing_agent_card_required_fields() {
        let kv = MockJetStreamKvStore::new();
        kv.enqueue_get_some(bytes::Bytes::from("{}".to_string()));
        let store = KvCatalogStore::new(kv);
        let err = store.get_card(&agent_id("bot")).await.unwrap_err();
        assert!(matches!(err, CatalogStoreError::AgentCardSchema(_)));
    }

    #[tokio::test]
    async fn put_returns_schema_error_when_card_missing_required_publishable_fields() {
        let kv = MockJetStreamKvStore::new();
        let store = KvCatalogStore::new(kv);
        let mut c = card("semi");
        c.supported_interfaces[0].url.clear();
        let err = store.put_card(&agent_id("semi"), &c).await.unwrap_err();
        assert!(matches!(err, CatalogStoreError::AgentCardSchema(_)));
    }

    #[test]
    fn error_display_kv() {
        let inner: BoxedKvError = Box::new(std::io::Error::other("conn failed"));
        let e = CatalogStoreError::Kv(inner);
        assert!(e.to_string().contains("KV store error"));
        assert!(e.to_string().contains("conn failed"));
    }

    #[test]
    fn error_kv_preserves_source_chain() {
        use std::error::Error;
        let inner: BoxedKvError = Box::new(std::io::Error::other("nats down"));
        let e = CatalogStoreError::Kv(inner);
        let src = e.source().expect("Kv error must expose its source");
        assert!(src.to_string().contains("nats down"));
    }

    #[test]
    fn error_invalid_key_has_no_source() {
        use std::error::Error;
        let e = CatalogStoreError::InvalidKey("bad.segment".into());
        assert!(e.to_string().contains("invalid catalog KV key"));
        assert!(e.source().is_none());
    }

    #[test]
    fn error_display_serialize() {
        let inner = serde_json::from_str::<String>("x").unwrap_err();
        let e = CatalogStoreError::Serialize(inner);
        assert!(e.to_string().contains("serialize agent card"));
    }

    #[test]
    fn error_source_serialize() {
        use std::error::Error;
        let inner = serde_json::from_str::<String>("x").unwrap_err();
        let e = CatalogStoreError::Serialize(inner);
        assert!(e.source().is_some());
    }

    #[test]
    fn error_display_and_source_covers_every_variant() {
        use std::error::Error;
        let bad = serde_json::from_str::<String>("x").unwrap_err();
        let ser = CatalogStoreError::Serialize(serde_json::from_str::<String>("x").unwrap_err());
        let de = CatalogStoreError::Deserialize(bad);
        let schema = a2a_pack::validate_agent_card_value(&serde_json::json!({})).unwrap_err();
        let schema_err = CatalogStoreError::AgentCardSchema(schema);
        let gate = CatalogStoreError::ImportGate(ImportGateError::Gateway("x".into()));

        assert!(ser.to_string().contains("serialize"));
        assert!(de.to_string().contains("deserialize"));
        assert!(schema_err.to_string().contains("JSON Schema"));
        assert!(!gate.to_string().is_empty());

        assert!(de.source().is_some());
        assert!(schema_err.source().is_some());
        assert!(gate.source().is_some());
    }

    #[tokio::test]
    async fn list_cards_returns_sorted_pairs() {
        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["beta".into(), "alpha".into()]));
        let alpha = serde_json::to_vec(&card("alpha")).unwrap();
        let beta = serde_json::to_vec(&card("beta")).unwrap();
        kv.enqueue_get_some(bytes::Bytes::from(alpha));
        kv.enqueue_get_some(bytes::Bytes::from(beta));

        let store = KvCatalogStore::new(kv);
        let pairs = store.list_cards().await.unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0.as_str(), "alpha");
        assert_eq!(pairs[1].0.as_str(), "beta");
    }

    #[tokio::test]
    async fn list_cards_rejects_invalid_kv_keys() {
        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["bad.segment".into()]));
        let store = KvCatalogStore::new(kv);
        let err = store.list_cards().await.unwrap_err();
        assert!(matches!(err, CatalogStoreError::InvalidKey(_)));
    }

    #[tokio::test]
    async fn list_cards_gated_allow_all_matches_ungated() {
        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["alpha".into(), "beta".into()]));
        for name in ["alpha", "beta", "alpha", "beta"] {
            let bytes = serde_json::to_vec(&card(name)).unwrap();
            kv.enqueue_get_some(bytes::Bytes::from(bytes));
        }
        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");
        let ungated = store.list_cards().await.unwrap();
        let gated = store
            .list_cards_gated(&AllowAllImportGate, &principal, |_id, _card| None)
            .await
            .unwrap();
        assert_eq!(ungated.len(), gated.len());
    }

    #[tokio::test]
    async fn list_cards_gated_allow_keeps_imported_when_card_accepts_federated_source() {
        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["peer".into()]));
        let bytes = serde_json::to_vec(&card("peer")).unwrap();
        kv.enqueue_get_some(bytes::Bytes::from(bytes));

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");
        let gated = store
            .list_cards_gated(&AllowAllImportGate, &principal, |_id, _card| {
                Some(ImportedAccountName::new("peer-account"))
            })
            .await
            .unwrap();
        assert_eq!(gated.len(), 1);
        assert_eq!(gated[0].name, "peer");
    }

    #[tokio::test]
    async fn list_cards_gated_deny_drops_imports_keeps_local() {
        use async_trait::async_trait;

        struct DenyAll;
        #[async_trait]
        impl ImportGate for DenyAll {
            async fn permit(
                &self,
                _p: &SpiceDbPrincipal,
                _i: &ImportedAccountName,
                _a: &A2aAgentId,
            ) -> Result<bool, ImportGateError> {
                Ok(false)
            }
        }

        let kv = MockJetStreamKvStore::new();
        kv.set_keys_result(Ok(vec!["peer".into(), "local".into()]));
        // list_cards sorts the keys so gets are issued in sorted order: local, peer.
        for name in ["local", "peer"] {
            let bytes = serde_json::to_vec(&card(name)).unwrap();
            kv.enqueue_get_some(bytes::Bytes::from(bytes));
        }

        let store = KvCatalogStore::new(kv);
        let principal = SpiceDbPrincipal::new("user:fixture");
        let gated = store
            .list_cards_gated(&DenyAll, &principal, |id, _card| {
                if id.as_str() == "local" {
                    None
                } else {
                    Some(ImportedAccountName::new("peer-account"))
                }
            })
            .await
            .unwrap();
        assert_eq!(gated.len(), 1);
        assert_eq!(gated[0].name, "local");
    }
