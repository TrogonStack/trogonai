use std::error::Error;

use async_nats::jetstream::kv;
use futures::StreamExt;

use super::constants::ACTOR_KV_BUCKET;
use super::entry_codec::{decode_entry, encode_entry};
use super::exchange_key::exchange_key;
use super::{ActorEntry, ActorIndex, ActorIndexError, ActorIndexResult, entry_key};

pub struct NatsActorIndex {
    kv: kv::Store,
}

impl NatsActorIndex {
    pub async fn new(jetstream: async_nats::jetstream::Context) -> ActorIndexResult<Self> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: ACTOR_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(ActorIndexError::storage)?;
        Ok(Self { kv })
    }

    /// The stored value under `key`, ignoring a tombstone left by a delete or purge.
    async fn live(&self, key: &str) -> ActorIndexResult<Option<kv::Entry>> {
        Ok(self
            .kv
            .entry(key)
            .await
            .map_err(ActorIndexError::storage)?
            .filter(is_live))
    }
}

fn is_live(entry: &kv::Entry) -> bool {
    !matches!(entry.operation, kv::Operation::Delete | kv::Operation::Purge)
}

pub(super) fn lost_update(error: &kv::UpdateError) -> bool {
    error.kind() == kv::UpdateErrorKind::WrongLastRevision
}

pub(super) fn lost_delete(error: &kv::DeleteError) -> bool {
    error.kind() == kv::DeleteErrorKind::WrongLastRevision
}

/// Reads a compare-and-set outcome: success is `true`, losing the race is `false`, and
/// anything else is a storage failure.
pub(super) fn settle<T, E>(result: Result<T, E>, lost_race: impl FnOnce(&E) -> bool) -> ActorIndexResult<bool>
where
    E: Error + Send + Sync + 'static,
{
    match result {
        Ok(_) => Ok(true),
        Err(error) if lost_race(&error) => Ok(false),
        Err(error) => Err(ActorIndexError::storage(error)),
    }
}

#[async_trait::async_trait]
impl ActorIndex for NatsActorIndex {
    async fn put(&self, entry: &ActorEntry) -> ActorIndexResult<()> {
        let payload = encode_entry(entry)?;
        self.kv
            .put(entry.key(), payload)
            .await
            .map_err(ActorIndexError::storage)?;
        Ok(())
    }

    async fn get(&self, atespace: &str, actor: &str) -> ActorIndexResult<Option<ActorEntry>> {
        let key = entry_key(atespace, actor);
        self.live(&key)
            .await?
            .map(|entry| decode_entry(&key, &entry.value))
            .transpose()
    }

    async fn delete(&self, atespace: &str, actor: &str) -> ActorIndexResult<()> {
        self.kv
            .delete(entry_key(atespace, actor))
            .await
            .map_err(ActorIndexError::storage)?;
        Ok(())
    }

    async fn all(&self) -> ActorIndexResult<Vec<ActorEntry>> {
        let mut keys = self.kv.keys().await.map_err(ActorIndexError::storage)?;
        let mut entries = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(ActorIndexError::storage)?;
            let live = self.live(&key).await?;
            entries.extend(live.map(|entry| decode_entry(&key, &entry.value)).transpose()?);
        }
        Ok(entries)
    }

    async fn compare_exchange(
        &self,
        expected: Option<&ActorEntry>,
        replacement: Option<&ActorEntry>,
    ) -> ActorIndexResult<bool> {
        let key = exchange_key(expected, replacement)?;
        let stored = self.live(&key).await?;
        let current = stored
            .as_ref()
            .map(|entry| decode_entry(&key, &entry.value))
            .transpose()?;
        if current.as_ref() != expected {
            return Ok(false);
        }
        match (stored, replacement) {
            (None, Some(entry)) => settle(self.kv.create(&key, encode_entry(entry)?).await, |error| {
                error.kind() == kv::CreateErrorKind::AlreadyExists
            }),
            (Some(stored), Some(entry)) => settle(
                self.kv.update(&key, encode_entry(entry)?, stored.revision).await,
                lost_update,
            ),
            // With no replacement `exchange_key` has already required an expected entry, so
            // this is always a delete of a stored entry; a missing revision would never match.
            (stored, None) => settle(
                self.kv
                    .delete_expect_revision(&key, stored.map(|entry| entry.revision))
                    .await,
                lost_delete,
            ),
        }
    }
}
