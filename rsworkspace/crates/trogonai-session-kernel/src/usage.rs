use buffa::Message as _;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKeyValueUpdate};
use trogonai_session_contracts::{SessionId, SessionUsage};

use crate::config::SessionKernelConfig;
use crate::error::SessionKernelError;
use crate::nats::{session_usage_bucket, session_usage_key};

/// NATS KV store for per-session token usage (`<PREFIX>_SESSION_USAGE`).
///
/// Usage is also materialized into the snapshot; this store persists it to its
/// own KV bucket per the NATS mapping (`NATS KV -> ACP_SESSION_USAGE`) so usage
/// can be read without loading the full snapshot.
#[derive(Clone)]
pub struct UsageStore<S> {
    store: S,
    config: SessionKernelConfig,
}

impl<S> UsageStore<S> {
    pub fn new(store: S, config: SessionKernelConfig) -> Self {
        Self { store, config }
    }

    pub fn bucket_name(&self) -> String {
        session_usage_bucket(&self.config.nats_prefix)
    }
}

impl<S> UsageStore<S>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    pub async fn load_usage(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionUsage>, SessionKernelError> {
        let key = session_usage_key(session_id);
        let Some(bytes) = self
            .store
            .get(key)
            .await
            .map_err(|err| SessionKernelError::UsageLoad(err.to_string()))?
        else {
            return Ok(None);
        };
        let usage = SessionUsage::decode_from_slice(&bytes)
            .map_err(|err| SessionKernelError::Decode(err.to_string()))?;
        Ok(Some(usage))
    }

    pub async fn save_usage(
        &self,
        session_id: &SessionId,
        usage: &SessionUsage,
    ) -> Result<(), SessionKernelError> {
        let key = session_usage_key(session_id);
        let bytes = Bytes::from(usage.encode_to_vec());
        if let Some(entry) = self
            .store
            .entry(key.clone())
            .await
            .map_err(|err| SessionKernelError::UsageStore(err.to_string()))?
        {
            self.store
                .update(&key, bytes, entry.revision)
                .await
                .map_err(|err| SessionKernelError::UsageStore(err.to_string()))?;
            return Ok(());
        }

        self.store
            .create(&key, bytes)
            .await
            .map_err(|err| SessionKernelError::UsageStore(err.to_string()))?;
        Ok(())
    }
}
