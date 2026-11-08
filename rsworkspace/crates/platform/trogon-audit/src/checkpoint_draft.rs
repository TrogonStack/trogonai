use std::time::SystemTime;

use buffa::{Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_nats::NatsToken;
use trogonai_proto::platform::audit::v1::CheckpointStatement;

use crate::chain_head::ChainHead;
use crate::chain_key::ChainKey;
use crate::checkpoint_id::CheckpointId;
use crate::checkpoint_jws;
use crate::key_id::KeyId;

/// A checkpoint before a key is chosen for it: the writer's chain, the head the
/// stream acknowledged and where it stored it, when, and the checkpoint's id.
///
/// The statement names the key that signs it, so a signer completes a draft
/// with its own key id and signs the result in one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointDraft {
    id: CheckpointId,
    chain: ChainKey,
    component: NatsToken,
    head: ChainHead,
    issued_at: SystemTime,
}

impl CheckpointDraft {
    pub fn new(
        id: CheckpointId,
        chain: ChainKey,
        component: NatsToken,
        head: ChainHead,
        issued_at: SystemTime,
    ) -> Self {
        Self {
            id,
            chain,
            component,
            head,
            issued_at,
        }
    }

    pub fn id(&self) -> CheckpointId {
        self.id
    }

    pub fn chain(&self) -> &ChainKey {
        &self.chain
    }

    pub fn component(&self) -> &NatsToken {
        &self.component
    }

    pub fn head(&self) -> &ChainHead {
        &self.head
    }

    pub fn issued_at(&self) -> SystemTime {
        self.issued_at
    }

    /// The bytes a signature by `key` covers: the JWS signing input of the
    /// statement this draft makes under `key`.
    pub fn signing_input(&self, key: &KeyId) -> Vec<u8> {
        checkpoint_jws::signing_input(&checkpoint_jws::protected_header(key), &self.statement(key))
    }

    /// The encoded statement under `key`, the bytes stored and signed.
    pub(crate) fn statement(&self, key: &KeyId) -> Vec<u8> {
        CheckpointStatement {
            writer_id: self.chain.writer().to_string(),
            chain_id: self.chain.chain().to_string(),
            head_sequence: self.head.sequence(),
            head_hash: self.head.hash().as_bytes().to_vec(),
            audit_stream: self.head.stream().to_owned(),
            audit_stream_sequence: self.head.stream_sequence(),
            issued_at: MessageField::some(Timestamp::from(self.issued_at)),
            key_id: key.to_string(),
            checkpoint_id: self.id.to_string(),
            component: self.component.to_string(),
        }
        .encode_to_vec()
    }
}

#[cfg(test)]
mod tests;
