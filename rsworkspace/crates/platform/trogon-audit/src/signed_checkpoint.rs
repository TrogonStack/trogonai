use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use buffa::Message;
use bytes::Bytes;
use trogonai_proto::platform::audit::v1::AuditCheckpoint;

use crate::audit_subject::AuditSubject;
use crate::chain_key::ChainKey;
use crate::checkpoint_draft::CheckpointDraft;
use crate::checkpoint_id::CheckpointId;
use crate::checkpoint_jws;
use crate::checkpoint_sign_error::CheckpointSignError;
use crate::checkpoint_signature::CheckpointSignature;
use crate::checkpoint_terminal::CheckpointTerminal;
use crate::constants::{HEADER_CHAIN, HEADER_SEQUENCE, HEADER_WRITER};
use crate::key_id::KeyId;

/// A checkpoint signed and encoded once, ready to publish to `AUDIT_CHECKPOINT`.
///
/// The payload is an `AuditCheckpoint` holding the statement bytes that were
/// signed and the detached JWS over them. Checkpoints are not chained, so a
/// failed one is never retried: the next checkpoint covers a later head.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedCheckpoint {
    id: CheckpointId,
    chain: ChainKey,
    head_sequence: u64,
    key: KeyId,
    subject: String,
    payload: Bytes,
}

impl SignedCheckpoint {
    /// Joins `draft` and the `signature` a signer returned for it, after checking
    /// the signature under the key id that came with it.
    pub(crate) fn assemble(
        draft: &CheckpointDraft,
        signature: &CheckpointSignature,
    ) -> Result<Self, CheckpointSignError> {
        let key = signature.key();
        let header = checkpoint_jws::protected_header(key);
        let statement = draft.statement(key);
        key.verifying_key()
            .verify_strict(
                &checkpoint_jws::signing_input(&header, &statement),
                signature.signature(),
            )
            .map_err(|_| CheckpointSignError::InvalidSignature)?;
        let message = AuditCheckpoint {
            statement,
            detached_jws: checkpoint_jws::detached(&header, signature.signature()),
        };
        Ok(Self {
            id: draft.id(),
            chain: draft.chain().clone(),
            head_sequence: draft.head().sequence(),
            key: *key,
            subject: AuditSubject::checkpoint(draft.component().clone(), CheckpointTerminal::Signed).render(),
            payload: Bytes::from(message.encode_to_vec()),
        })
    }

    pub fn id(&self) -> CheckpointId {
        self.id
    }

    pub fn chain(&self) -> &ChainKey {
        &self.chain
    }

    pub fn head_sequence(&self) -> u64 {
        self.head_sequence
    }

    pub fn key(&self) -> &KeyId {
        &self.key
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn payload(&self) -> &Bytes {
        &self.payload
    }

    /// `Nats-Msg-Id` is the checkpoint id. The provenance headers name the chain
    /// and its head, so a reader can select a chain's checkpoints without
    /// decoding them; the verifier trusts only the signed statement.
    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, self.id.to_string());
        headers.insert(HEADER_WRITER, self.chain.writer().as_str());
        headers.insert(HEADER_SEQUENCE, self.head_sequence.to_string());
        headers.insert(HEADER_CHAIN, self.chain.chain().to_string());
        headers
    }
}

#[cfg(test)]
mod tests;
