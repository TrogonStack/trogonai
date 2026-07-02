use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet};

use crate::projection::kill_status_wire::{KillStatusWireError, decode_kill_status, encode_kill_status};
use crate::{AgentId, KillSwitchState};

pub type BoxedKvError = Box<dyn std::error::Error + Send + Sync>;

/// Errors raised projecting [`KillSwitchState`] into the `KILL_SWITCH_STATE`
/// KV bucket.
///
/// Shaped like `a2a-nats`'s `CatalogStoreError`: a boxed-but-source-preserving
/// `Kv` variant for the underlying JetStream/async-nats failure, plus typed
/// variants for everything this crate itself can reject.
#[derive(Debug, thiserror::Error)]
pub enum KillSwitchProjectionError {
    #[error("failed to encode or decode projected kill status: {0}")]
    Wire(#[source] KillStatusWireError),
    /// Wraps the underlying KV failure as a boxed source so callers keep the
    /// source-chain and can downcast to the concrete JetStream / async-nats
    /// error type instead of pattern-matching on a stringified message.
    #[error("KV store error: {0}")]
    Kv(#[source] BoxedKvError),
    #[error("kill switch projection write lost a revision race after {attempts} attempts")]
    ConflictRetriesExhausted { attempts: u32 },
}

enum PutAttemptError {
    Conflict,
    Fatal(KillSwitchProjectionError),
}

/// Projects current [`KillSwitchState`] into a NATS JetStream Key/Value
/// bucket, keyed by [`AgentId`].
///
/// This is a read-model projection, not the system of record: the
/// `KILL_SWITCH_EVENTS` JetStream stream (one subject per agent, appended to
/// by [`crate::Kill`]/[`crate::Revive`] through `trogon-decider-nats`) is
/// authoritative. `KillSwitchProjectionStore` exists so a caller that only
/// needs "is this agent killed right now" (a gateway policy check, an
/// auth-callout hook) can do a single KV `get` instead of replaying a stream.
/// See `docs/proposals/kill-switch-enforcement.md`.
#[derive(Clone)]
pub struct KillSwitchProjectionStore<K> {
    store: K,
}

impl<K> KillSwitchProjectionStore<K> {
    pub fn new(store: K) -> Self {
        Self { store }
    }
}

impl<K> KillSwitchProjectionStore<K>
where
    K: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Send + Sync + Clone + 'static,
{
    /// Projects `state` as the current status for `agent_id`.
    ///
    /// Bounded retry loop so a concurrent projector re-applying an
    /// overlapping event (e.g. two consumer replicas racing after a
    /// redelivery) can't make the write fail permanently: three attempts
    /// absorbs a revision race without spinning on a persistent backend
    /// failure, matching `a2a-nats`'s `KvCatalogStore::put_card`.
    pub async fn put(&self, agent_id: &AgentId, state: &KillSwitchState) -> Result<(), KillSwitchProjectionError> {
        let value: Bytes = encode_kill_status(state)
            .map_err(KillSwitchProjectionError::Wire)
            .map(Bytes::from)?;
        let key = agent_id.as_str().to_owned();

        const MAX_PUT_ATTEMPTS: u32 = 3;
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let entry = self
                .store
                .entry(key.clone())
                .await
                .map_err(|e| KillSwitchProjectionError::Kv(Box::new(e)))?;

            // JetStream KV returns the latest revision even for a delete or
            // purge tombstone; `update` against that revision fails with
            // "wrong last sequence", so re-projecting after a delete must go
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
                        _ => PutAttemptError::Fatal(KillSwitchProjectionError::Kv(Box::new(e))),
                    }),
                None => self
                    .store
                    .create(&key, value.clone())
                    .await
                    .map(|_| ())
                    .map_err(|e| match e.kind() {
                        async_nats::jetstream::kv::CreateErrorKind::AlreadyExists => PutAttemptError::Conflict,
                        _ => PutAttemptError::Fatal(KillSwitchProjectionError::Kv(Box::new(e))),
                    }),
            };

            match outcome {
                Ok(()) => return Ok(()),
                Err(PutAttemptError::Fatal(e)) => return Err(e),
                Err(PutAttemptError::Conflict) if attempt < MAX_PUT_ATTEMPTS => continue,
                Err(PutAttemptError::Conflict) => {
                    return Err(KillSwitchProjectionError::ConflictRetriesExhausted { attempts: attempt });
                }
            }
        }
    }

    /// Reads the currently projected status for `agent_id`, or `None` when
    /// the agent has no projected status (never killed, or the key was
    /// deleted/purged).
    pub async fn get(&self, agent_id: &AgentId) -> Result<Option<KillSwitchState>, KillSwitchProjectionError> {
        let key = agent_id.as_str().to_owned();
        match self
            .store
            .get(key)
            .await
            .map_err(|e| KillSwitchProjectionError::Kv(Box::new(e)))?
        {
            None => Ok(None),
            Some(bytes) => decode_kill_status(&bytes)
                .map(Some)
                .map_err(KillSwitchProjectionError::Wire),
        }
    }
}

#[cfg(test)]
mod tests;
