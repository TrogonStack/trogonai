use std::time::SystemTime;

use trogon_nats::NatsToken;

use crate::checkpoint_id::CheckpointId;
use crate::key_id::KeyId;
use crate::verified_chain_head::VerifiedChainHead;

/// A checkpoint whose signature verified under a trusted key and whose statement
/// names the verified head it was checked against. Only
/// [`verify_checkpoint`](crate::verify_checkpoint) builds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCheckpoint {
    id: CheckpointId,
    issued_at: SystemTime,
    key: KeyId,
    component: NatsToken,
    head: VerifiedChainHead,
}

impl VerifiedCheckpoint {
    pub(crate) fn new(
        id: CheckpointId,
        issued_at: SystemTime,
        key: KeyId,
        component: NatsToken,
        head: VerifiedChainHead,
    ) -> Self {
        Self {
            id,
            issued_at,
            key,
            component,
            head,
        }
    }

    pub fn id(&self) -> CheckpointId {
        self.id
    }

    pub fn issued_at(&self) -> SystemTime {
        self.issued_at
    }

    pub fn key(&self) -> &KeyId {
        &self.key
    }

    pub fn component(&self) -> &NatsToken {
        &self.component
    }

    pub fn head(&self) -> &VerifiedChainHead {
        &self.head
    }
}
