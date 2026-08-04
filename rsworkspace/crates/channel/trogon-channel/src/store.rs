use crate::conversation::{ConversationId, ConversationRecord};
use crate::endpoint::{Endpoint, PrincipalId};
use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use tracing::info;
use trogon_nats::jetstream::{is_create_key_value_already_exists, is_get_key_value_not_found};
use trogon_std::NowV7;

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;

#[derive(Debug, thiserror::Error)]
pub enum ChannelStoreError {
    #[error("failed to open KV bucket {bucket}: {source}")]
    OpenBucket {
        bucket: String,
        #[source]
        source: async_nats::jetstream::context::KeyValueError,
    },
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
    /// The conversation record was written but binding its endpoint failed. The
    /// record has been rolled back, so the store is as it was before the call.
    #[error("failed to bind endpoint {endpoint} to new conversation {conversation}: {source}")]
    BindEndpoint {
        endpoint: String,
        conversation: ConversationId,
        #[source]
        source: async_nats::jetstream::kv::PutError,
    },
    /// Binding failed and so did the rollback, so the conversation record is
    /// still in the bucket with nothing pointing at it. Both failures are kept
    /// typed: the operator needs the key to sweep, and the cause to know why.
    #[error(
        "failed to bind endpoint {endpoint} to new conversation {conversation} ({bind_error}), \
         and removing the now-unreachable conversation record failed too: {source}"
    )]
    OrphanedConversation {
        endpoint: String,
        conversation: ConversationId,
        bind_error: async_nats::jetstream::kv::PutError,
        #[source]
        source: async_nats::jetstream::kv::DeleteError,
    },
}

/// What we know about a principal beyond its id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrincipalRecord {
    pub display_name: Option<String>,
}

/// The four registries behind conversations, all JetStream KV, all owned by
/// exactly one worker, the bridge. Config
/// files never hold this state; the admin surface that seeds/mutates it is
/// out of band by design.
pub struct ChannelStore {
    principals: jetstream::kv::Store,
    endpoints: jetstream::kv::Store,
    bindings: jetstream::kv::Store,
    conversations: jetstream::kv::Store,
}

/// Opens a bucket, creating it only when JetStream says it is not there.
///
/// A get that failed for any other reason (a request timeout, a denied
/// `STREAM.INFO`) is surfaced instead of being read as absence. The bucket
/// probably does exist in that case, and `STREAM.CREATE` silently applies some
/// divergent fields as an in-place update of the existing stream, so falling
/// through would let a momentary read failure reconfigure live storage.
async fn ensure_bucket(js: &jetstream::Context, bucket: String) -> Result<jetstream::kv::Store, ChannelStoreError> {
    match js.get_key_value(&bucket).await {
        Ok(store) => return Ok(store),
        Err(source) if is_get_key_value_not_found(&source) => {}
        Err(source) => return Err(ChannelStoreError::OpenBucket { bucket, source }),
    }

    info!(bucket = %bucket, "Creating channel KV bucket");
    match js
        .create_key_value(jetstream::kv::Config {
            bucket: bucket.clone(),
            history: 5,
            storage: jetstream::stream::StorageType::File,
            ..Default::default()
        })
        .await
    {
        Ok(store) => Ok(store),
        // Another replica created it between the get and the create, which is
        // the one already-exists that means the bucket is ready rather than
        // that provisioning went wrong.
        Err(source) if is_create_key_value_already_exists(&source) => match js.get_key_value(&bucket).await {
            Ok(store) => Ok(store),
            Err(source) => Err(ChannelStoreError::OpenBucket { bucket, source }),
        },
        Err(source) => Err(ChannelStoreError::CreateBucket { bucket, source }),
    }
}

impl ChannelStore {
    pub async fn ensure(js: &jetstream::Context, prefix: &str) -> Result<Self, ChannelStoreError> {
        Ok(Self {
            principals: ensure_bucket(js, format!("channel_principals_{prefix}")).await?,
            endpoints: ensure_bucket(js, format!("channel_endpoints_{prefix}")).await?,
            bindings: ensure_bucket(js, format!("channel_bindings_{prefix}")).await?,
            conversations: ensure_bucket(js, format!("channel_conversations_{prefix}")).await?,
        })
    }

    /// Identity: which principal owns this endpoint. `None` means the
    /// endpoint is unknown and the bridge must reject the message; this is
    /// the access-control mechanism.
    pub async fn principal_for(&self, endpoint: &Endpoint) -> Result<Option<PrincipalId>, ChannelStoreError> {
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
    ) -> Result<(), ChannelStoreError> {
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
    ) -> Result<Option<(ConversationId, ConversationRecord)>, ChannelStoreError> {
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
    ///
    /// The record has to be written before the binding, so that no binding is
    /// ever briefly visible pointing at a record that does not exist. That
    /// leaves the opposite exposure: a binding write that fails would strand a
    /// record nothing can reach. It is rolled back before the error returns,
    /// because each attempt generates a fresh id, so without the rollback every
    /// redelivery of one message would leave another unreachable record behind.
    pub async fn create_conversation(
        &self,
        endpoint: &Endpoint,
        record: &ConversationRecord,
        ids: &impl NowV7,
    ) -> Result<ConversationId, ChannelStoreError> {
        let id = ConversationId::generate(ids);
        let conversation = serde_json::to_vec(record)?;
        let binding = serde_json::to_vec(&id)?;

        self.conversations.put(id.as_str(), conversation.into()).await?;

        let Err(source) = self.bindings.put(endpoint.kv_key(), binding.into()).await else {
            return Ok(id);
        };

        match self.conversations.delete(id.as_str()).await {
            Ok(()) => Err(ChannelStoreError::BindEndpoint {
                endpoint: endpoint.kv_key(),
                conversation: id,
                source,
            }),
            Err(cleanup) => Err(ChannelStoreError::OrphanedConversation {
                endpoint: endpoint.kv_key(),
                conversation: id,
                bind_error: source,
                source: cleanup,
            }),
        }
    }

    /// Update a conversation record in place (session replacement, activity).
    pub async fn update_conversation(
        &self,
        id: &ConversationId,
        record: &ConversationRecord,
    ) -> Result<(), ChannelStoreError> {
        self.conversations
            .put(id.as_str(), serde_json::to_vec(record)?.into())
            .await?;
        Ok(())
    }
}
