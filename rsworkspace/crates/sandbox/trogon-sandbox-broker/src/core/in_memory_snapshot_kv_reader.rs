use super::lock::lock;
use super::*;

/// In-memory [`SnapshotKvReader`] keyed by tenant and digest.
#[derive(Default)]
pub struct InMemorySnapshotKvReader {
    records: std::sync::Mutex<std::collections::HashMap<(TenantId, Digest), SnapshotKvRecord>>,
}

impl InMemorySnapshotKvReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn put(&self, record: SnapshotKvRecord) {
        lock(&self.records).insert((record.tenant.clone(), record.digest.clone()), record);
    }
}

#[async_trait::async_trait]
impl SnapshotKvReader for InMemorySnapshotKvReader {
    async fn get_owned(&self, tenant: &TenantId, digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        Ok(lock(&self.records).get(&(tenant.clone(), digest.clone())).cloned())
    }
    async fn record(&self, record: &SnapshotKvRecord) -> Result<(), SnapshotKvError> {
        self.put(record.clone());
        Ok(())
    }
    async fn get(&self, digest: &Digest) -> Result<Option<SnapshotKvRecord>, SnapshotKvError> {
        let records = lock(&self.records);
        let mut found = records.values().filter(|record| record.digest == *digest);
        let first = found.next().cloned();
        if found.next().is_some() {
            return Err(SnapshotKvError::TenantRequired);
        }
        Ok(first)
    }
}
