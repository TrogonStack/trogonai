use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use buffa::Message;
use bytes::Bytes;
use trogonai_proto::platform::audit::v1::{AuditClass, Integrity};

use crate::constants::{HEADER_SEQUENCE, HEADER_WRITER};
use crate::obligation::Obligation;
use crate::record::Record;
use crate::record_id::RecordId;
use crate::writer_id::WriterId;

/// A record with its writer id and sequence stamped in, encoded once and ready to
/// publish. A retry publishes these same bytes under the same message id.
///
/// The sequence is a process-local monotonic counter and a placeholder for the
/// per-writer hash chain of ADR#0086 Decision 6, which replaces it. It restarts
/// at the writer's next start.
#[derive(Debug, Clone, PartialEq)]
pub struct SealedRecord {
    id: RecordId,
    subject: String,
    obligation: Obligation,
    class: AuditClass,
    writer: WriterId,
    sequence: u64,
    payload: Bytes,
}

impl SealedRecord {
    pub(crate) fn seal(record: Record, writer: &WriterId, sequence: u64) -> Self {
        let obligation = record.obligation();
        let class = record.kind().audit_class();
        let (id, subject, _kind, mut message) = record.into_message();
        message.integrity = buffa::MessageField::some(Integrity {
            writer_id: writer.to_string(),
            sequence,
            ..Default::default()
        });
        Self {
            id,
            subject: subject.render(),
            obligation,
            class,
            writer: writer.clone(),
            sequence,
            payload: Bytes::from(message.encode_to_vec()),
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

    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    pub fn payload(&self) -> &Bytes {
        &self.payload
    }

    /// `Nats-Msg-Id` is the record id, so a retry after a lost ack cannot store
    /// the record twice. Provenance rides in headers and never in the payload
    /// (ADR#0039).
    pub fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, self.id.to_string());
        headers.insert(HEADER_WRITER, self.writer.as_str());
        headers.insert(HEADER_SEQUENCE, self.sequence.to_string());
        headers
    }
}

#[cfg(test)]
mod tests;
