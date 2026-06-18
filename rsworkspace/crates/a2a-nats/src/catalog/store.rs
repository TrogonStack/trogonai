use a2a_pack::{AgentCardSource, accept_agent_card_on_read};
use bytes::Bytes;
use futures::TryStreamExt;
use serde_json::Value;
use trogon_nats::jetstream::{
    JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys,
};

use crate::agent_id::A2aAgentId;
use crate::catalog::import_gate::{ImportGate, ImportGateError, ImportedAccountName, SpiceDbPrincipal};

pub type BoxedKvError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum CatalogStoreError {
    #[error("failed to serialize agent card: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error("failed to deserialize agent card: {0}")]
    Deserialize(#[source] serde_json::Error),
    /// AgentCard document failed bundled JSON-Schema validation ([`a2a_pack`]).
    #[error("AgentCard rejected by JSON Schema: {0}")]
    AgentCardSchema(#[source] a2a_pack::AgentCardValidateError),
    #[error("{0}")]
    ImportGate(#[source] ImportGateError),
    /// Wraps the underlying KV failure as a boxed source so callers keep the
    /// source-chain and can downcast to the concrete JetStream / async-nats
    /// error type instead of pattern-matching on a stringified message.
    #[error("KV store error: {0}")]
    Kv(#[source] BoxedKvError),
    /// Catalog KV key segment failed `A2aAgentId` validation.
    #[error("invalid catalog KV key: {0}")]
    InvalidKey(String),
}

enum PutAttemptError {
    Conflict,
    Fatal(CatalogStoreError),
}

fn deserialize_validated_agent_card(parsed: Value) -> Result<a2a::agent_card::AgentCard, CatalogStoreError> {
    a2a_pack::validate_agent_card_value(&parsed).map_err(CatalogStoreError::AgentCardSchema)?;
    serde_json::from_value::<a2a::agent_card::AgentCard>(parsed).map_err(CatalogStoreError::Deserialize)
}

pub trait CatalogStore: Send + Sync + 'static {
    fn put_card(
        &self,
        agent_id: &A2aAgentId,
        card: &a2a::agent_card::AgentCard,
    ) -> impl std::future::Future<Output = Result<(), CatalogStoreError>> + Send;

    fn get_card(
        &self,
        agent_id: &A2aAgentId,
    ) -> impl std::future::Future<Output = Result<Option<a2a::agent_card::AgentCard>, CatalogStoreError>> + Send;
}

#[derive(Clone)]
pub struct KvCatalogStore<K> {
    store: K,
}

impl<K> KvCatalogStore<K> {
    pub fn new(store: K) -> Self {
        Self { store }
    }
}

impl<K> KvCatalogStore<K>
where
    K: JetStreamKvGet
        + JetStreamKvEntry
        + JetStreamKvCreate
        + JetStreamKeyValueUpdate
        + JetStreamKvKeys
        + Send
        + Sync
        + Clone
        + 'static,
{
    pub async fn list_cards(&self) -> Result<Vec<(A2aAgentId, a2a::agent_card::AgentCard)>, CatalogStoreError> {
        let keys: Vec<String> = self
            .store
            .keys()
            .await
            .map_err(|e| CatalogStoreError::Kv(Box::new(e)))?
            .try_collect()
            .await
            .map_err(|e| CatalogStoreError::Kv(Box::new(e)))?;
        let mut keys = keys;
        keys.sort_unstable();

        let mut out = Vec::new();
        for key in keys {
            let agent_id = A2aAgentId::new(&key).map_err(|_| {
                CatalogStoreError::InvalidKey(format!("catalog KV key `{key}` is not a valid A2aAgentId segment"))
            })?;
            if let Some(card) = self.get_card(&agent_id).await? {
                out.push((agent_id, card));
            }
        }
        Ok(out)
    }

    // TODO(card-source-account): hydrate import provenance from catalog wire decoding instead of the caller-supplied resolver.
    pub async fn list_cards_gated<G, F>(
        &self,
        gate: &G,
        principal: &SpiceDbPrincipal,
        imported_source: F,
    ) -> Result<Vec<a2a::agent_card::AgentCard>, CatalogStoreError>
    where
        G: ImportGate + Sync,
        F: Fn(&A2aAgentId, &a2a::agent_card::AgentCard) -> Option<ImportedAccountName> + Send + Sync,
    {
        let pairs = self.list_cards().await?;

        let mut gated = Vec::new();
        for (agent_id, card) in pairs {
            match imported_source(&agent_id, &card) {
                None => gated.push(card),
                Some(imported_from) => {
                    if gate
                        .permit(principal, &imported_from, &agent_id)
                        .await
                        .map_err(CatalogStoreError::ImportGate)?
                    {
                        let value = serde_json::to_value(&card).map_err(CatalogStoreError::Serialize)?;
                        if accept_agent_card_on_read(&value, AgentCardSource::FederatedImport) {
                            gated.push(card);
                        }
                    }
                }
            }
        }

        Ok(gated)
    }
}

impl<K> CatalogStore for KvCatalogStore<K>
where
    K: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Send + Sync + Clone + 'static,
{
    async fn put_card(
        &self,
        agent_id: &A2aAgentId,
        card: &a2a::agent_card::AgentCard,
    ) -> Result<(), CatalogStoreError> {
        let value_json: Value = serde_json::to_value(card).map_err(CatalogStoreError::Serialize)?;
        a2a_pack::validate_agent_card_value(&value_json).map_err(CatalogStoreError::AgentCardSchema)?;
        let value: Bytes = serde_json::to_vec(&value_json)
            .map(Bytes::from)
            .map_err(CatalogStoreError::Serialize)?;
        let key = agent_id.as_str().to_owned();

        // Bounded retry loop so a concurrent registrar publishing the same key
        // between our `entry()` read and our `create`/`update` write can't make
        // re-registration fail permanently. Three attempts is enough to absorb
        // the two race shapes (`WrongLastRevision` against a stale revision and
        // `AlreadyExists` against a re-created key) without spinning on a
        // persistent backend failure.
        const MAX_PUT_ATTEMPTS: u32 = 3;
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let entry = self
                .store
                .entry(key.clone())
                .await
                .map_err(|e| CatalogStoreError::Kv(Box::new(e)))?;

            // JetStream KV returns the latest revision even when it's a delete or
            // purge tombstone — `update` against that revision would fail with
            // "wrong last sequence", so re-registration after a delete must go
            // through `create` to restore the key.
            let live_revision = entry.and_then(|e| match e.operation {
                async_nats::jetstream::kv::Operation::Put => Some(e.revision),
                async_nats::jetstream::kv::Operation::Delete | async_nats::jetstream::kv::Operation::Purge => None,
            });

            let outcome = match live_revision {
                Some(revision) => self
                    .store
                    .update(&key, value.clone(), revision)
                    .await
                    .map(|_| ())
                    .map_err(|e| match e.kind() {
                        async_nats::jetstream::kv::UpdateErrorKind::WrongLastRevision => PutAttemptError::Conflict,
                        _ => PutAttemptError::Fatal(CatalogStoreError::Kv(Box::new(e))),
                    }),
                None => self
                    .store
                    .create(&key, value.clone())
                    .await
                    .map(|_| ())
                    .map_err(|e| match e.kind() {
                        async_nats::jetstream::kv::CreateErrorKind::AlreadyExists => PutAttemptError::Conflict,
                        _ => PutAttemptError::Fatal(CatalogStoreError::Kv(Box::new(e))),
                    }),
            };

            match outcome {
                Ok(()) => return Ok(()),
                Err(PutAttemptError::Fatal(e)) => return Err(e),
                Err(PutAttemptError::Conflict) if attempt < MAX_PUT_ATTEMPTS => continue,
                Err(PutAttemptError::Conflict) => {
                    return Err(CatalogStoreError::Kv(Box::<dyn std::error::Error + Send + Sync>::from(
                        "catalog KV write lost a revision race after retries",
                    )));
                }
            }
        }
    }

    async fn get_card(&self, agent_id: &A2aAgentId) -> Result<Option<a2a::agent_card::AgentCard>, CatalogStoreError> {
        let key = agent_id.as_str().to_owned();
        match self
            .store
            .get(key)
            .await
            .map_err(|e| CatalogStoreError::Kv(Box::new(e)))?
        {
            None => Ok(None),
            Some(bytes) => {
                let parsed: serde_json::Value =
                    serde_json::from_slice(&bytes).map_err(CatalogStoreError::Deserialize)?;
                Ok(Some(deserialize_validated_agent_card(parsed)?))
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
