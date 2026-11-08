use async_nats::HeaderMap;
use async_nats::header::NATS_MESSAGE_ID;
use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use bytes::Bytes;
use trogon_nats::NatsToken;
use trogonai_proto::platform::audit::v1::audit_actor::Unattributed;
use trogonai_proto::platform::audit::v1::{
    AuditActor, AuditClass, AuditRecord, ChainResetReason, Integrity, WriterLifecycle, WriterLifecycleKind,
};

use crate::audit_subject::AuditSubject;
use crate::chain_anchor::ChainAnchor;
use crate::chain_id::ChainId;
use crate::constants::{GENESIS_SEQUENCE, HEADER_CHAIN, HEADER_SEQUENCE, HEADER_WRITER};
use crate::integrity_terminal::IntegrityTerminal;
use crate::record::Record;
use crate::record_hash::RecordHash;
use crate::record_seal;
use crate::stamp::Stamp;
use crate::stored_audit_record::StoredAuditRecord;
use crate::writer_id::WriterId;

/// Builds valid sealed chains for tests of what reads the `AUDIT` stream.
///
/// It numbers, links and seals records the way the stored chain must look, and
/// stores nothing: it is a source of verifier input, not a writer. Clone it to
/// fork a chain, such as two different records at one sequence.
#[derive(Debug, Clone)]
pub struct ChainBuilder {
    writer: WriterId,
    chain: ChainId,
    component: NatsToken,
    next_sequence: u64,
    previous: Option<RecordHash>,
}

impl ChainBuilder {
    pub fn new(writer: WriterId, chain: ChainId, component: NatsToken) -> Self {
        Self {
            writer,
            chain,
            component,
            next_sequence: GENESIS_SEQUENCE,
            previous: None,
        }
    }

    pub fn genesis(&self) -> ChainAnchor {
        ChainAnchor::Genesis {
            writer: self.writer.clone(),
            chain: self.chain,
        }
    }

    /// The record that opens a chain after a process start.
    pub fn chain_reset(&mut self, stamp: Stamp, stream_sequence: u64) -> StoredAuditRecord {
        let message = AuditRecord {
            record_id: stamp.id.to_string(),
            class: EnumValue::from(AuditClass::Must),
            occurred_at: MessageField::some(Timestamp::from(stamp.occurred_at)),
            recorded_at: MessageField::some(Timestamp::from(stamp.recorded_at)),
            actor: MessageField::some(AuditActor {
                kind: Some(Unattributed::default().into()),
            }),
            detail: Some(
                WriterLifecycle {
                    kind: EnumValue::from(WriterLifecycleKind::ChainReset),
                    reset_reason: Some(EnumValue::from(ChainResetReason::ProcessStartWithoutResume)),
                }
                .into(),
            ),
            ..Default::default()
        };
        let subject = AuditSubject::integrity(self.component.clone(), IntegrityTerminal::ChainReset);
        self.message(subject.render(), message, stream_sequence)
    }

    pub fn record(&mut self, record: Record, stream_sequence: u64) -> StoredAuditRecord {
        let (_, subject, _, message) = record.into_message();
        self.message(subject.render(), message, stream_sequence)
    }

    /// Places `message` at the next sequence, linked to the previous record, and
    /// stores it on `subject` with the headers a writer sets.
    pub fn message(
        &mut self,
        subject: impl Into<String>,
        mut message: AuditRecord,
        stream_sequence: u64,
    ) -> StoredAuditRecord {
        let sequence = self.next_sequence;
        message.integrity = MessageField::some(Integrity {
            writer_id: self.writer.to_string(),
            sequence,
            previous_hash: self.previous.map(|hash| hash.as_bytes().to_vec()),
            record_hash: None,
            chain_id: Some(self.chain.to_string()),
        });
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, message.record_id.as_str());
        headers.insert(HEADER_WRITER, self.writer.as_str());
        headers.insert(HEADER_SEQUENCE, sequence.to_string());
        headers.insert(HEADER_CHAIN, self.chain.to_string());
        let (hash, payload) = record_seal::seal(Bytes::from(message.encode_to_vec()));
        self.next_sequence = sequence.saturating_add(1);
        self.previous = Some(hash);
        StoredAuditRecord::new(stream_sequence, payload)
            .with_subject(subject)
            .with_headers(&headers)
    }
}

#[cfg(test)]
mod tests;
