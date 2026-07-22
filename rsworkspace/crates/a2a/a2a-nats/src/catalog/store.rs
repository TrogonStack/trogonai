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
mod tests;
