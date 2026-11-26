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
        stream::Config {
            name: self.stream_name(),
            subjects: vec![format!("{}.peer.v1.peer.*.events", self.tenant.as_str())],
            retention: stream::RetentionPolicy::Limits,
            storage: stream::StorageType::File,
            discard: stream::DiscardPolicy::New,
            max_bytes: self.budget.as_bytes() as i64,
            max_age: std::time::Duration::ZERO,
            max_messages: -1,
            max_messages_per_subject: -1,
            allow_atomic_publish: true,
            ..Default::default()
        }
    }

    pub fn snapshot_config(&self) -> kv::Config {
        kv::Config {
            bucket: self.snapshot_bucket_name(),
            history: 1,
            max_bytes: self.budget.as_bytes() as i64,
            max_value_size: self.budget.as_bytes().min(i32::MAX as u64) as i32,
            max_age: std::time::Duration::ZERO,
            storage: stream::StorageType::File,
            ..Default::default()
        }
    }
}
