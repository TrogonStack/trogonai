use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::sync::RwLock;
use trogon_sandbox_contract::{Digest, LeaseId};
use trogon_sandbox_protocol::subjects::lease_snapshot_in;

use super::SnapshotPinStore;
use crate::bus::BrokerBus;

pub struct BusSnapshotPinStore {
    bus: Arc<dyn BrokerBus>,
    cache: RwLock<HashMap<LeaseId, Digest>>,
}

impl BusSnapshotPinStore {
    pub fn new(bus: Arc<dyn BrokerBus>) -> Arc<Self> {
        Arc::new(Self {
            bus,
            cache: RwLock::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl SnapshotPinStore for BusSnapshotPinStore {
    async fn pin(&self, lease_id: &LeaseId, digest: &Digest) -> anyhow::Result<()> {
        let subject = lease_snapshot_in(lease_id);
        let payload = Bytes::from(serde_json::to_vec(digest)?);
        self.bus.publish(&subject, payload).await?;
        self.cache.write().await.insert(lease_id.clone(), digest.clone());
        Ok(())
    }

    async fn expected_digest(&self, lease_id: &LeaseId) -> anyhow::Result<Option<Digest>> {
        if let Some(d) = self.cache.read().await.get(lease_id) {
            return Ok(Some(d.clone()));
        }
        let subject = lease_snapshot_in(lease_id);
        let bytes = self.bus.request(&subject, Bytes::new()).await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let digest: Digest = serde_json::from_slice(&bytes)?;
        self.cache.write().await.insert(lease_id.clone(), digest.clone());
        Ok(Some(digest))
    }

    async fn unpin(&self, lease_id: &LeaseId) -> anyhow::Result<()> {
        self.cache.write().await.remove(lease_id);
        Ok(())
    }
}
