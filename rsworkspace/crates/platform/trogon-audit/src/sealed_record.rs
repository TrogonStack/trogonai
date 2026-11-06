use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use buffa::Message;
use bytes::Bytes;
use trogonai_proto::platform::audit::v1::{AuditClass, Integrity};

use crate::chain_id::ChainId;
use crate::constants::{HEADER_CHAIN, HEADER_SEQUENCE, HEADER_WRITER};
use crate::obligation::Obligation;
use crate::record::Record;
use crate::record_hash::RecordHash;
use crate::record_id::RecordId;
use crate::record_seal;
use crate::writer_id::WriterId;

/// A record placed in its writer's chain, encoded and sealed once and ready to
/// publish (ADR#0086 Decision 6). A retry publishes these same bytes under the
/// same message id, so a copy the stream stores twice is identical.
///
/// The record's integrity names its writer, chain, sequence and the previous
/// record's hash, with no record hash; the stored bytes are that encoding
/// followed by the seal trailer that carries its hash.
#[derive(Debug, Clone, PartialEq)]
pub struct SealedRecord {
    id: RecordId,
    subject: String,
    obligation: Obligation,
    class: AuditClass,
    writer: WriterId,
    chain: ChainId,
    sequence: u64,
    previous_hash: Option<RecordHash>,
    hash: RecordHash,
    payload: Bytes,
}

impl SealedRecord {
    pub(crate) fn seal(
        record: Record,
        writer: &WriterId,
        chain: ChainId,
        sequence: u64,
        previous_hash: Option<RecordHash>,
    ) -> Self {
        let obligation = record.obligation();
        let class = record.kind().audit_class();
        let (id, subject, _kind, mut message) = record.into_message();
        message.integrity = buffa::MessageField::some(Integrity {
            writer_id: writer.to_string(),
            sequence,
            previous_hash: previous_hash.map(|hash| hash.as_bytes().to_vec()),
            record_hash: None,
            chain_id: Some(chain.to_string()),
        });
        let (hash, payload) = record_seal::seal(Bytes::from(message.encode_to_vec()));
        Self {
            id,
            subject: subject.render(),
            obligation,
            class,
            writer: writer.clone(),
            chain,
            sequence,
            previous_hash,
            hash,
            payload,
        }
    }

    pub fn id(&self) -> RecordId {
        self.id
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn class(&self) -> AuditClass {
        self.class
    }

    pub fn obligation(&self) -> Obligation {
        self.obligation
    }

    pub fn writer(&self) -> &WriterId {
        &self.writer
    }

    pub fn chain(&self) -> ChainId {
        self.chain
    }

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The hash of the record before this one in the chain, absent only at the
    /// chain's first record.
    pub fn previous_hash(&self) -> Option<RecordHash> {
        self.previous_hash
    }

    /// The hash the next record in the chain names.
    pub fn hash(&self) -> RecordHash {
        self.hash
    }

    pub fn payload(&self) -> &Bytes {
        &self.payload
    }

    /// `Nats-Msg-Id` is the record id, so a retry after a lost ack inside the
    /// stream's duplicate window cannot store the record twice. Provenance rides
    /// in headers and never in the payload (ADR#0039).
    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, self.id.to_string());
        headers.insert(HEADER_WRITER, self.writer.as_str());
        headers.insert(HEADER_SEQUENCE, self.sequence.to_string());
        headers.insert(HEADER_CHAIN, self.chain.to_string());
        headers
    }
}

#[cfg(test)]
mod tests;
