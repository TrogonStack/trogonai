use super::constants::SNAPSHOT_KV_CAS_ATTEMPTS;
use super::*;

/// [`SnapshotKvReader`] over the shared snapshot JetStream KV bucket.
pub struct NatsKvSnapshotKvReader {
    kv: async_nats::jetstream::kv::Store,
}

impl NatsKvSnapshotKvReader {
    pub async fn new(jetstream: async_nats::jetstream::Context) -> Result<Self, SnapshotKvError> {
        let kv = jetstream
            .create_or_update_key_value(async_nats::jetstream::kv::Config {
                bucket: subjects::SNAPSHOT_KV_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(SnapshotKvError::Bucket)?;
        Ok(Self { kv })
    }
}

fn decode(value: &[u8]) -> Result<SnapshotKvRecord, SnapshotKvError> {
    serde_json::from_slice(value).map_err(SnapshotKvError::Decode)
}

#[async_trait::async_trait]
impl SnapshotKvReader for NatsKvSnapshotKvReader {
    async fn get_owned(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        let key = trogon_sandbox_protocol::tenant_scoped_kv_key(tenant, digest.as_str());
        if let Some(entry) = self.kv.entry(&key).await.map_err(SnapshotKvError::Read)? {
            if entry.operation != async_nats::jetstream::kv::Operation::Put {
                return Ok(None);
            }
            let record = decode(&entry.value)?;
            if record.tenant != *tenant || record.digest != *digest {
                return Err(SnapshotKvError::IdentityMismatch);
            }
            return Ok(Some(record));
        }
        Ok(self.get(digest).await?.filter(|record| record.tenant == *tenant))
    }
    async fn record(&self, record: &SnapshotKvRecord) -> Result<(), SnapshotKvError> {
        let key = trogon_sandbox_protocol::tenant_scoped_kv_key(&record.tenant, record.digest.as_str());
        let payload = serde_json::to_vec(record).map_err(SnapshotKvError::Encode)?;
        for _ in 0..SNAPSHOT_KV_CAS_ATTEMPTS {
            match self.kv.entry(&key).await.map_err(SnapshotKvError::Read)? {
                Some(entry) => {
                    if entry.operation == async_nats::jetstream::kv::Operation::Put {
                        let current = decode(&entry.value)?;
                        if current.tenant != record.tenant || current.digest != record.digest {
                            return Err(SnapshotKvError::IdentityMismatch);
                        }
                        if current.created_at > record.created_at || current == *record {
                            return Ok(());
                        }
                        if current.created_at == record.created_at {
                            return Err(SnapshotKvError::ConflictingCapture);
                        }
                    }
                    match self.kv.update(&key, payload.clone().into(), entry.revision).await {
                        Ok(_) => return Ok(()),
                        Err(error) if error.kind() == async_nats::jetstream::kv::UpdateErrorKind::WrongLastRevision => {
                            continue;
                        }
                        Err(error) => return Err(SnapshotKvError::Update(error)),
                    }
                }
                None => match self.kv.create(&key, payload.clone().into()).await {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == async_nats::jetstream::kv::CreateErrorKind::AlreadyExists => {
                        continue;
                    }
                    Err(error) => return Err(SnapshotKvError::Create(error)),
                },
            }
        }
        Err(SnapshotKvError::CasRetriesExhausted)
    }

    async fn get(&self, digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        let key = subjects::snapshot_kv_key(digest);
        match self.kv.entry(&key).await.map_err(SnapshotKvError::Read)? {
            None => Ok(None),
            Some(entry) => Ok(Some(decode(&entry.value)?)),
        }
    }
}
