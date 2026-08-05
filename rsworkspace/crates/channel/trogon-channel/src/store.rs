use crate::conversation::{ConversationId, ConversationRecord};
use crate::endpoint::{Endpoint, PrincipalId};
use async_nats::jetstream;
use serde::{Deserialize, Serialize};
use tracing::info;
use trogon_nats::jetstream::{
    is_create_key_already_exists, is_create_key_value_already_exists, is_get_key_value_not_found,
};
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
        source: ReserveEndpointError,
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
        bind_error: ReserveEndpointError,
        #[source]
        source: async_nats::jetstream::kv::DeleteError,
    },
}

/// Why an endpoint could not be pointed at a new conversation. Four ways in:
/// an unbound endpoint is claimed with a create, one left pointing at a record
/// that is gone is re-pointed with a compare-and-swap, and a lost claim has to
/// be read and followed before either of those is decided.
#[derive(Debug, thiserror::Error)]
pub enum ReserveEndpointError {
    #[error(transparent)]
    Claim(#[from] async_nats::jetstream::kv::CreateError),
    /// The stale binding moved between the read and the swap, so another writer
    /// re-pointed the endpoint first.
    #[error(transparent)]
    Repoint(#[from] async_nats::jetstream::kv::UpdateError),
    /// The claim that won could not be read back, so there is no telling whether
    /// to yield to it or take it over.
    #[error("the claim on the endpoint could not be read back: {0}")]
    Inspect(#[from] async_nats::jetstream::kv::EntryError),
    /// The claim was read but the conversation it leads to was not.
    #[error(transparent)]
    Follow(#[from] BoundConversationError),
}

/// Why the conversation an encoded binding leads to could not be read. Narrower
/// than [`ChannelStoreError`] so that a reservation still holding an unbound
/// record can carry the cause into [`ReserveEndpointError`] on its way out,
/// rather than the two error types nesting inside one another.
#[derive(Debug, thiserror::Error)]
pub enum BoundConversationError {
    #[error("KV read failed: {0}")]
    Read(#[from] async_nats::jetstream::kv::EntryError),
    #[error("stored record is not valid JSON: {0}")]
    Decode(#[from] serde_json::Error),
}

impl From<BoundConversationError> for ChannelStoreError {
    fn from(error: BoundConversationError) -> Self {
        match error {
            BoundConversationError::Read(source) => Self::Read(source),
            BoundConversationError::Decode(source) => Self::Decode(source),
        }
    }
}

/// Which conversation an endpoint is bound to once
/// [`ChannelStore::create_conversation`] returns.
#[derive(Debug)]
pub enum EndpointBinding {
    /// The record handed in is the endpoint's conversation now.
    Created(ConversationId),
    /// The endpoint was claimed between the caller's lookup and this
    /// reservation, so the winner's conversation is the one the endpoint feeds
    /// and the record this call would have added has been rolled back. The
    /// caller has to continue with what comes back here: its own record is
    /// gone, and a second conversation on one endpoint would split the history
    /// a user sees as one chat.
    AlreadyBound(ConversationId, ConversationRecord),
}

impl EndpointBinding {
    /// The conversation this endpoint's next message belongs to, and the record
    /// to carry on with. `mine` is the record the caller built for the
    /// reservation, and it survives only if the reservation was won; losing the
    /// claim means continuing on the winner's record instead, so the caller
    /// cannot be left holding one that nothing routes to.
    ///
    /// Which way it went is reported here rather than by the caller, because
    /// only one of the two is a race worth reading in a log and every caller
    /// wants the same pair out of it.
    pub fn into_conversation(
        self,
        endpoint: &Endpoint,
        mine: ConversationRecord,
    ) -> (ConversationId, ConversationRecord) {
        match self {
            Self::Created(id) => {
                info!(conversation = %id, endpoint = %endpoint, agent = %mine.agent_id, "Created conversation");
                (id, mine)
            }
            Self::AlreadyBound(id, bound) => {
                info!(
                    conversation = %id,
                    endpoint = %endpoint,
                    agent = %bound.agent_id,
                    "Endpoint was bound while this conversation was being created; continuing on the one already bound"
                );
                (id, bound)
            }
        }
    }
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
        Ok(self
            .bound_conversation(self.bindings.get(endpoint.kv_key()).await?)
            .await?)
    }

    /// The conversation an encoded binding leads to. An endpoint with no binding
    /// and one bound to a record that is no longer there answer alike, because
    /// neither routes anywhere: the next message for that endpoint has to open a
    /// conversation either way.
    async fn bound_conversation(
        &self,
        binding: Option<impl AsRef<[u8]>>,
    ) -> Result<Option<(ConversationId, ConversationRecord)>, BoundConversationError> {
        let Some(bytes) = binding else {
            return Ok(None);
        };
        let id: ConversationId = serde_json::from_slice(bytes.as_ref())?;
        match self.conversations.get(id.as_str()).await? {
            Some(bytes) => Ok(Some((id, serde_json::from_slice(&bytes)?))),
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
    ///
    /// The binding is claimed rather than overwritten. Callers reach this after
    /// a lookup found the endpoint unbound, and two workers can hold that answer
    /// at once; an overwrite would let the loser's binding bury the winner's
    /// conversation, and the buried record is unreachable forever because
    /// nothing else knows its id. Losing the claim is therefore not a failure:
    /// the winner's conversation comes back instead ([`EndpointBinding`]).
    ///
    /// A claim also has to cope with a binding that points at a record that is
    /// no longer there (see `a_binding_with_no_conversation_record_reads_as_unbound`),
    /// which reads as unbound and so arrives here. That one is re-pointed on the
    /// revision it was read at, because an endpoint whose claim can only ever be
    /// refused is an endpoint no message can get through.
    pub async fn create_conversation(
        &self,
        endpoint: &Endpoint,
        record: &ConversationRecord,
        ids: &impl NowV7,
    ) -> Result<EndpointBinding, ChannelStoreError> {
        let id = ConversationId::generate(ids);
        let conversation = serde_json::to_vec(record)?;
        let binding = serde_json::to_vec(&id)?;

        self.conversations.put(id.as_str(), conversation.into()).await?;

        let taken = match self.bindings.create(endpoint.kv_key(), binding.clone().into()).await {
            Ok(_) => return Ok(EndpointBinding::Created(id)),
            Err(taken) if is_create_key_already_exists(&taken) => taken,
            Err(source) => return Err(self.unwind(endpoint, id, source.into()).await),
        };

        // Read the claim that won as an entry rather than a value: replacing a
        // stale one is only safe against the revision it was read at. A claim
        // that is gone by the time it is read is claimed at revision zero, which
        // the server honours only while the key is still absent, so the race
        // that emptied it cannot be lost twice.
        //
        // Both this read and the one that follows it unwind the record written
        // above, for the same reason the write failures do: the id is minted per
        // attempt, so a record left behind by a read that failed is one nothing
        // can ever reach again.
        let claimed = match self.bindings.entry(endpoint.kv_key()).await {
            Ok(claimed) => claimed,
            Err(source) => return Err(self.unwind(endpoint, id, source.into()).await),
        };
        let revision = claimed.as_ref().map_or(0, |claim| claim.revision);

        // Only a claim that leads to a conversation is one to yield to. One that
        // leads nowhere is taken over below, and so is a claim that is gone by
        // the time it is read: revision zero says the key must still be absent,
        // so the race that emptied it cannot be lost twice.
        let bound = match self.bound_conversation(claimed.map(|claim| claim.value)).await {
            Ok(bound) => bound,
            Err(source) => return Err(self.unwind(endpoint, id, source.into()).await),
        };

        if let Some((bound_id, bound_record)) = bound {
            return match self.conversations.delete(id.as_str()).await {
                Ok(()) => Ok(EndpointBinding::AlreadyBound(bound_id, bound_record)),
                Err(cleanup) => Err(ChannelStoreError::OrphanedConversation {
                    endpoint: endpoint.kv_key(),
                    conversation: id,
                    bind_error: taken.into(),
                    source: cleanup,
                }),
            };
        }

        info!(
            endpoint = %endpoint,
            conversation = %id,
            "The endpoint's claim leads to no conversation record; taking it over"
        );
        match self.bindings.update(endpoint.kv_key(), binding.into(), revision).await {
            Ok(_) => Ok(EndpointBinding::Created(id)),
            Err(source) => Err(self.unwind(endpoint, id, source.into()).await),
        }
    }

    /// Take back the record this call wrote, so a reservation that never
    /// happened leaves nothing behind. `refused` travels into the error because
    /// a rollback that fails too leaves the record for an operator to sweep, and
    /// both causes are what makes that actionable.
    async fn unwind(
        &self,
        endpoint: &Endpoint,
        conversation: ConversationId,
        refused: ReserveEndpointError,
    ) -> ChannelStoreError {
        match self.conversations.delete(conversation.as_str()).await {
            Ok(()) => ChannelStoreError::BindEndpoint {
                endpoint: endpoint.kv_key(),
                conversation,
                source: refused,
            },
            Err(source) => ChannelStoreError::OrphanedConversation {
                endpoint: endpoint.kv_key(),
                conversation,
                bind_error: refused,
                source,
            },
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
