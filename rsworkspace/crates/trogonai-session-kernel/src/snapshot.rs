use buffa::Message as _;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKeyValueUpdate};
use trogonai_session_contracts::{SessionId, SessionSnapshot, ValidatedSessionSnapshot};

use crate::config::SessionKernelConfig;
use crate::error::SessionKernelError;
use crate::nats::{session_snapshot_key, session_snapshots_bucket};

/// NATS KV store for materialized session snapshots.
#[derive(Clone)]
pub struct SnapshotStore<S> {
    store: S,
    config: SessionKernelConfig,
}

impl<S> SnapshotStore<S> {
    pub fn new(store: S, config: SessionKernelConfig) -> Self {
        Self { store, config }
    }

    pub fn bucket_name(&self) -> String {
        session_snapshots_bucket(&self.config.nats_prefix)
    }
}

impl<S> SnapshotStore<S>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    pub async fn load_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionSnapshot>, SessionKernelError> {
        let key = session_snapshot_key(session_id);
        let Some(bytes) = self
            .store
            .get(key)
            .await
            .map_err(|err| SessionKernelError::SnapshotLoad(err.to_string()))?
        else {
            return Ok(None);
        };
        let snapshot = SessionSnapshot::decode_from_slice(&bytes)
            .map_err(|err| SessionKernelError::Decode(err.to_string()))?;
        ValidatedSessionSnapshot::try_from_snapshot(snapshot.clone())?;
        Ok(Some(snapshot))
    }

    pub async fn save_snapshot(&self, snapshot: &SessionSnapshot) -> Result<(), SessionKernelError> {
        let validated = ValidatedSessionSnapshot::try_from_snapshot(snapshot.clone())?;
        let payload = validated.snapshot.encode_to_vec();
        if payload.len() > self.config.max_snapshot_bytes {
            return Err(SessionKernelError::SnapshotTooLarge {
                actual: payload.len(),
                limit: self.config.max_snapshot_bytes,
            });
        }

        let key = session_snapshot_key(&validated.session_id);
        let bytes = Bytes::from(payload);
        if let Some(entry) = self
            .store
            .entry(key.clone())
            .await
            .map_err(|err| SessionKernelError::SnapshotStore(err.to_string()))?
        {
            self.store
                .update(&key, bytes, entry.revision)
                .await
                .map_err(|err| SessionKernelError::SnapshotStore(err.to_string()))?;
            return Ok(());
        }

        self.store
            .create(&key, bytes)
            .await
            .map_err(|err| SessionKernelError::SnapshotStore(err.to_string()))?;
        Ok(())
    }
}
