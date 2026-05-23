use std::fmt;

use bytes::Bytes;
use futures::TryStreamExt;
use serde_json::Value;
use trogon_nats::jetstream::{
    JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys,
};

use crate::agent_id::A2aAgentId;
use crate::catalog::import_gate::{ImportGate, ImportGateError, ImportedAccountName, SpiceDbPrincipal};

#[derive(Debug)]
pub enum CatalogStoreError {
    Serialize(serde_json::Error),
    Deserialize(serde_json::Error),
    /// AgentCard document failed bundled JSON-Schema validation ([`a2a_pack`]).
    AgentCardSchema(a2a_pack::AgentCardValidateError),
    ImportGate(ImportGateError),
    Kv(String),
}

impl fmt::Display for CatalogStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(e) => write!(f, "failed to serialize agent card: {e}"),
            Self::Deserialize(e) => write!(f, "failed to deserialize agent card: {e}"),
            Self::AgentCardSchema(e) => write!(f, "AgentCard rejected by JSON Schema: {e}"),
            Self::ImportGate(e) => write!(f, "{e}"),
            Self::Kv(msg) => write!(f, "KV store error: {msg}"),
        }
    }
}

impl std::error::Error for CatalogStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(e) | Self::Deserialize(e) => Some(e),
            Self::AgentCardSchema(e) => Some(e),
            Self::ImportGate(e) => Some(e),
            Self::Kv(_) => None,
        }
    }
}

fn deserialize_validated_agent_card(parsed: Value) -> Result<a2a_types::AgentCard, CatalogStoreError> {
    a2a_pack::validate_agent_card_value(&parsed).map_err(CatalogStoreError::AgentCardSchema)?;
    serde_json::from_value::<a2a_types::AgentCard>(parsed).map_err(CatalogStoreError::Deserialize)
}

pub trait CatalogStore: Send + Sync + 'static {
    fn put_card(
        &self,
        agent_id: &A2aAgentId,
        card: &a2a_types::AgentCard,
    ) -> impl std::future::Future<Output = Result<(), CatalogStoreError>> + Send;

    fn get_card(
        &self,
        agent_id: &A2aAgentId,
    ) -> impl std::future::Future<Output = Result<Option<a2a_types::AgentCard>, CatalogStoreError>> + Send;
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
    pub async fn list_cards(&self) -> Result<Vec<(A2aAgentId, a2a_types::AgentCard)>, CatalogStoreError> {
        let keys: Vec<String> = self
            .store
            .keys()
            .await
            .map_err(|e| CatalogStoreError::Kv(e.to_string()))?
            .try_collect()
            .await
            .map_err(|e| CatalogStoreError::Kv(e.to_string()))?;
        let mut keys = keys;
        keys.sort_unstable();

        let mut out = Vec::new();
        for key in keys {
            let agent_id = A2aAgentId::new(&key).map_err(|_| {
                CatalogStoreError::Kv(format!("catalog KV key `{key}` is not a valid A2aAgentId segment"))
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
    ) -> Result<Vec<a2a_types::AgentCard>, CatalogStoreError>
    where
        G: ImportGate + Sync,
        F: Fn(&A2aAgentId, &a2a_types::AgentCard) -> Option<ImportedAccountName> + Send + Sync,
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
                        gated.push(card);
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
    async fn put_card(&self, agent_id: &A2aAgentId, card: &a2a_types::AgentCard) -> Result<(), CatalogStoreError> {
        let value_json: Value = serde_json::to_value(card).map_err(CatalogStoreError::Serialize)?;
        a2a_pack::validate_agent_card_value(&value_json).map_err(CatalogStoreError::AgentCardSchema)?;
        let value: Bytes = serde_json::to_vec(&value_json)
            .map(Bytes::from)
            .map_err(CatalogStoreError::Serialize)?;
        let key = agent_id.as_str().to_owned();

        let entry = self
            .store
            .entry(key.clone())
            .await
            .map_err(|e| CatalogStoreError::Kv(e.to_string()))?;

        match entry {
            Some(e) => {
                self.store
                    .update(&key, value, e.revision)
                    .await
                    .map_err(|e| CatalogStoreError::Kv(e.to_string()))?;
            }
            None => {
                self.store
                    .create(&key, value)
                    .await
                    .map_err(|e| CatalogStoreError::Kv(e.to_string()))?;
            }
        }

        Ok(())
    }

    async fn get_card(&self, agent_id: &A2aAgentId) -> Result<Option<a2a_types::AgentCard>, CatalogStoreError> {
        let key = agent_id.as_str().to_owned();
        match self
            .store
            .get(key)
            .await
            .map_err(|e| CatalogStoreError::Kv(e.to_string()))?
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
    use trogon_nats::jetstream::mocks::MockJetStreamKvStore;

    fn agent_id(s: &str) -> A2aAgentId {
        A2aAgentId::new(s).unwrap()
    }

    fn card(name: &str) -> a2a_types::AgentCard {
        a2a_types::AgentCard {
            name: name.to_string(),
            supported_interfaces: vec![a2a_types::AgentInterface {
                url: "https://example.com/a2a".to_string(),
                protocol_binding: "JSONRPC".to_string(),
                protocol_version: "0.2.0".to_string(),
                tenant: String::new(),
            }],
            ..Default::default()
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
        let e = CatalogStoreError::Kv("conn failed".into());
        assert!(e.to_string().contains("KV store error"));
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
    fn error_source_kv() {
        use std::error::Error;
        let e = CatalogStoreError::Kv("x".into());
        assert!(e.source().is_none());
    }
}
