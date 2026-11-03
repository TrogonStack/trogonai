use async_nats::jetstream::{kv, stream};

use super::PeerStorageBudget;
use crate::PeerTenant;

#[derive(Debug, Clone)]
pub struct PeerStoreConfig {
    tenant: PeerTenant,
    budget: PeerStorageBudget,
}

impl PeerStoreConfig {
    pub fn new(tenant: PeerTenant, budget: PeerStorageBudget) -> Self {
        Self { tenant, budget }
    }

    pub fn tenant(&self) -> &PeerTenant {
        &self.tenant
    }

    pub fn budget(&self) -> PeerStorageBudget {
        self.budget
    }

    pub fn stream_name(&self) -> String {
        format!("PEER_{}", self.tenant.as_str())
    }

    pub fn snapshot_bucket_name(&self) -> String {
        format!("PEER_SNAPSHOTS_{}", self.tenant.as_str())
    }

    pub fn stream_config(&self) -> stream::Config {
        stream::Config::default()
    }

    pub fn snapshot_config(&self) -> kv::Config {
        kv::Config::default()
    }
}
