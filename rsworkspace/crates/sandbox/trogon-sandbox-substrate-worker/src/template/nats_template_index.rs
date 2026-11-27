use async_nats::jetstream::kv;
use bytes::Bytes;

use super::constants::TEMPLATE_KV_BUCKET;
use super::{TemplateIndex, TemplateIndexError, TemplateRecord};

pub struct NatsTemplateIndex {
    kv: kv::Store,
}

impl NatsTemplateIndex {
    pub async fn new(jetstream: async_nats::jetstream::Context) -> Result<Self, TemplateIndexError> {
        let kv = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: TEMPLATE_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(TemplateIndexError::storage)?;
        Ok(Self { kv })
    }
}

#[async_trait::async_trait]
impl TemplateIndex for NatsTemplateIndex {
    async fn get(&self, key: &str) -> Result<Option<TemplateRecord>, TemplateIndexError> {
        match self.kv.entry(key).await.map_err(TemplateIndexError::storage)? {
            None => Ok(None),
            Some(entry) => Ok(serde_json::from_slice(&entry.value).ok()),
        }
    }

    async fn create(&self, key: &str, record: &TemplateRecord) -> Result<bool, TemplateIndexError> {
        let payload = Bytes::from(serde_json::to_vec(record).map_err(TemplateIndexError::Encode)?);
        match self.kv.create(key, payload).await {
            Ok(_) => Ok(true),
            Err(e) if is_key_exists(&e) => Ok(false),
            Err(e) => Err(TemplateIndexError::storage(e)),
        }
    }

    async fn put(&self, key: &str, record: &TemplateRecord) -> Result<(), TemplateIndexError> {
        let payload = Bytes::from(serde_json::to_vec(record).map_err(TemplateIndexError::Encode)?);
        self.kv.put(key, payload).await.map_err(TemplateIndexError::storage)?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<(), TemplateIndexError> {
        self.kv.delete(key).await.map_err(TemplateIndexError::storage)?;
        Ok(())
    }
}

fn is_key_exists(error: &kv::CreateError) -> bool {
    matches!(error.kind(), kv::CreateErrorKind::AlreadyExists)
}
