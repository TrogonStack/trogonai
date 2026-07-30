use crate::conversation::{ConversationId, ConversationRecord};
use crate::endpoint::{Endpoint, PrincipalId};
use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, thiserror::Error)]
pub enum ChatStoreError {
    #[error("failed to create KV bucket {bucket}: {source}")]
    CreateBucket {
        bucket: String,
        #[source]
        source: async_nats::jetstream::context::CreateKeyValueError,
    },
    #[error("KV read failed: {0}")]
    Read(#[from] async_nats::jetstream::kv::EntryError),
    #[error("KV write failed: {0}")]
    Write(#[from] async_nats::jetstream::kv::PutError),
    #[error("stored record is not valid JSON: {0}")]
    Decode(#[from] serde_json::Error),
}

/// What we know about a principal beyond its id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrincipalRecord {
    pub display_name: Option<String>,
}

/// The four registries behind conversations, all JetStream KV, all owned by
/// exactly one worker (the bridge today, the router after extraction). Config
/// files never hold this state; the admin surface that seeds/mutates it is
/// out of band by design.
pub struct ChatStore {
    principals: jetstream::kv::Store,
    endpoints: jetstream::kv::Store,
    bindings: jetstream::kv::Store,
    conversations: jetstream::kv::Store,
}

async fn ensure_bucket(
    js: &jetstream::Context,
    bucket: String,
) -> Result<jetstream::kv::Store, ChatStoreError> {
    if let Ok(store) = js.get_key_value(&bucket).await {
        return Ok(store);
    }
    info!(bucket = %bucket, "Creating chat KV bucket");
    js.create_key_value(jetstream::kv::Config {
        bucket: bucket.clone(),
        history: 5,
        storage: jetstream::stream::StorageType::File,
        ..Default::default()
    })
    .await
    .map_err(|source| ChatStoreError::CreateBucket { bucket, source })
}

impl ChatStore {
    pub async fn ensure(js: &jetstream::Context, prefix: &str) -> Result<Self, ChatStoreError> {
        Ok(Self {
            principals: ensure_bucket(js, format!("chat_principals_{prefix}")).await?,
            endpoints: ensure_bucket(js, format!("chat_endpoints_{prefix}")).await?,
            bindings: ensure_bucket(js, format!("chat_bindings_{prefix}")).await?,
            conversations: ensure_bucket(js, format!("chat_conversations_{prefix}")).await?,
        })
    }

    /// Identity: which principal owns this endpoint. `None` means the
    /// endpoint is unknown and the bridge must reject the message; this is
    /// the access-control mechanism.
    pub async fn principal_for(&self, endpoint: &Endpoint) -> Result<Option<PrincipalId>, ChatStoreError> {
        match self.endpoints.get(endpoint.kv_key()).await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Register a principal and map an endpoint to it (idempotent).
    pub async fn link_endpoint(
        &self,
        principal: &PrincipalId,
        record: &PrincipalRecord,
        endpoint: &Endpoint,
    ) -> Result<(), ChatStoreError> {
        self.principals
            .put(principal.as_str(), serde_json::to_vec(record)?.into())
            .await?;
        self.endpoints
            .put(endpoint.kv_key(), serde_json::to_vec(principal)?.into())
            .await?;
        Ok(())
    }

    /// Binding: which conversation this endpoint currently feeds.
    pub async fn conversation_for(
        &self,
        endpoint: &Endpoint,
    ) -> Result<Option<(ConversationId, ConversationRecord)>, ChatStoreError> {
        let Some(bytes) = self.bindings.get(endpoint.kv_key()).await? else {
            return Ok(None);
        };
        let id: ConversationId = serde_json::from_slice(&bytes)?;
        match self.conversations.get(id.as_str()).await? {
            Some(bytes) => Ok(Some((id.clone(), serde_json::from_slice(&bytes)?))),
            None => Ok(None),
        }
    }

    /// Create a conversation and bind an endpoint to it. Routing policy runs
    /// before this call (it decided `record.agent_id`); after it, the binding
    /// is sticky.
    pub async fn create_conversation(
        &self,
        endpoint: &Endpoint,
        record: &ConversationRecord,
    ) -> Result<ConversationId, ChatStoreError> {
        let id = ConversationId::generate();
        self.conversations
            .put(id.as_str(), serde_json::to_vec(record)?.into())
            .await?;
        self.bindings
            .put(endpoint.kv_key(), serde_json::to_vec(&id)?.into())
            .await?;
        Ok(id)
    }

    /// Update a conversation record in place (session replacement, activity).
    pub async fn update_conversation(
        &self,
        id: &ConversationId,
        record: &ConversationRecord,
    ) -> Result<(), ChatStoreError> {
        self.conversations
            .put(id.as_str(), serde_json::to_vec(record)?.into())
            .await?;
        Ok(())
    }
}
