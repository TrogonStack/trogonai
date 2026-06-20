use buffa::Message as _;
use bytes::Bytes;
use trogon_nats::jetstream::{JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKeyValueUpdate};
use trogonai_session_contracts::{ContextTwin, SessionId};

use crate::config::ProjectionConfig;
use crate::error::ProjectionError;
use crate::nats::{context_twin_key, context_twins_bucket};

/// NATS KV store for derived Context Twin snapshots (`ACP_CONTEXT_TWINS`).
#[derive(Clone)]
pub struct ContextTwinStore<S> {
    store: S,
    config: ProjectionConfig,
}

impl<S> ContextTwinStore<S> {
    pub fn new(store: S, config: ProjectionConfig) -> Self {
        Self { store, config }
    }

    pub fn bucket_name(&self) -> String {
        context_twins_bucket(&self.config.nats_prefix)
    }
}

impl<S> ContextTwinStore<S>
where
    S: JetStreamKvGet + JetStreamKvEntry + JetStreamKvCreate + JetStreamKeyValueUpdate + Clone + Send + Sync + 'static,
{
    pub async fn load_context_twin(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ContextTwin>, ProjectionError> {
        let key = context_twin_key(session_id);
        let Some(bytes) = self
            .store
            .get(key)
            .await
            .map_err(|err| ProjectionError::ContextTwinLoad(err.to_string()))?
        else {
            return Ok(None);
        };
        let twin = ContextTwin::decode_from_slice(&bytes)
            .map_err(|err| ProjectionError::Decode(err.to_string()))?;
        Ok(Some(twin))
    }

    pub async fn save_context_twin(&self, twin: &ContextTwin) -> Result<(), ProjectionError> {
        if twin.session_id.is_empty() {
            return Err(ProjectionError::MissingField("session_id"));
        }
        let payload = twin.encode_to_vec();
        if payload.len() > self.config.max_context_twin_bytes {
            return Err(ProjectionError::ContextTwinTooLarge {
                actual: payload.len(),
                limit: self.config.max_context_twin_bytes,
            });
        }

        let session_id = SessionId::new(&twin.session_id)
            .map_err(|err| ProjectionError::ContractValidation(trogonai_session_contracts::ContractValidationError::InvalidSessionId(err)))?;
        let key = context_twin_key(&session_id);
        let bytes = Bytes::from(payload);
        if let Some(entry) = self
            .store
            .entry(key.clone())
            .await
            .map_err(|err| ProjectionError::ContextTwinStore(err.to_string()))?
        {
            self.store
                .update(&key, bytes, entry.revision)
                .await
                .map_err(|err| ProjectionError::ContextTwinStore(err.to_string()))?;
            return Ok(());
        }

        self.store
            .create(&key, bytes)
            .await
            .map_err(|err| ProjectionError::ContextTwinStore(err.to_string()))?;
        Ok(())
    }
}

pub async fn provision_context_twin_store<J, S>(
    js: &J,
    config: &ProjectionConfig,
) -> Result<S, ProjectionError>
where
    J: trogon_nats::jetstream::JetStreamGetKeyValue<Store = S>
        + trogon_nats::jetstream::JetStreamCreateKeyValue<Store = S>,
    S: trogon_nats::jetstream::JetStreamKeyValueStatus,
{
    let bucket = context_twins_bucket(&config.nats_prefix);
    match js.get_key_value(bucket.clone()).await {
        Ok(store) => Ok(store),
        Err(_) => js
            .create_key_value(async_nats::jetstream::kv::Config {
                bucket,
                ..Default::default()
            })
            .await
            .map_err(|err| ProjectionError::Provision(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_name_uses_acp_prefix() {
        let store = ContextTwinStore::new((), ProjectionConfig::default());
        assert_eq!(store.bucket_name(), "ACP_CONTEXT_TWINS");
    }
}
