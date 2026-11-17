//! One chain's records checked in stream order, keeping only the previous
//! record's position and hash.

use async_nats::HeaderName;
use async_nats::header::NATS_MESSAGE_ID;
use buffa::Message;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{AuditRecord, Integrity, WriterLifecycleKind};

use crate::chain_anchor::ChainAnchor;
use crate::chain_id::ChainId;
use crate::chain_key::ChainKey;
use crate::chain_notice::ChainNotice;
use crate::chain_verification_error::ChainVerificationError;
use crate::constants::{GENESIS_SEQUENCE, HEADER_CHAIN, HEADER_SEQUENCE, HEADER_WRITER, MAX_CHAIN_RANGE_RECORDS};
use crate::record_hash::RecordHash;
use crate::record_seal;
use crate::stored_audit_record::StoredAuditRecord;
use crate::subject_agreement::subject_agrees;
use crate::verified_chain_head::VerifiedChainHead;
use crate::verified_chain_range::VerifiedChainRange;

pub(crate) struct ChainWalk {
    key: ChainKey,
    head: Option<VerifiedChainHead>,
    records: usize,
    records_without_metadata: usize,
    notices: Vec<ChainNotice>,
}

impl ChainWalk {
    pub(crate) fn new(anchor: ChainAnchor) -> Self {
        let (key, head) = match anchor {
            ChainAnchor::Genesis { writer, chain } => (ChainKey::new(writer, chain), None),
            ChainAnchor::TrustedHead(head) => (head.key().clone(), Some(head)),
        };
        Self {
            key,
            head,
            records: 0,
            records_without_metadata: 0,
            notices: Vec::new(),
        }
    }

    pub(crate) fn check(&mut self, stored: &StoredAuditRecord) -> Result<(), ChainVerificationError> {
        if self.records == MAX_CHAIN_RANGE_RECORDS {
            return Err(ChainVerificationError::RangeTooLong);
        }
        let stream_sequence = stored.stream_sequence();
        if let Some(head) = &self.head
            && stream_sequence <= head.stream_sequence()
        {
            return Err(ChainVerificationError::StreamSequenceRegression {
                stream_sequence,
                previous_stream_sequence: head.stream_sequence(),
            });
        }
        let (body, hash) = record_seal::open(stored.payload()).map_err(|source| ChainVerificationError::Unsealed {
            stream_sequence,
            source,
        })?;
        let record = AuditRecord::decode_from_slice(body).map_err(|source| ChainVerificationError::Malformed {
            stream_sequence,
            source,
        })?;
        let integrity = record
            .integrity
            .as_option()
            .ok_or(ChainVerificationError::MissingIntegrity { stream_sequence })?;
        if integrity.record_hash.is_some() {
            return Err(ChainVerificationError::HashInBody { stream_sequence });
        }
        self.check_identity(stream_sequence, integrity)?;
        check_metadata(stored, &record, integrity)?;

        let next = self.link(stream_sequence, &record, integrity, hash)?;
        self.records += 1;
        if !stored.has_metadata() {
            self.records_without_metadata += 1;
        }
        self.head = Some(next);
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<VerifiedChainRange, ChainVerificationError> {
        let records = self.records;
        let head = self
            .head
            .filter(|_| records > 0)
            .ok_or(ChainVerificationError::EmptyRange)?;
        Ok(VerifiedChainRange::new(
            head,
            self.records,
            self.records_without_metadata,
            self.notices,
        ))
    }

    fn check_identity(&self, stream_sequence: u64, integrity: &Integrity) -> Result<(), ChainVerificationError> {
        if integrity.writer_id != self.key.writer().as_str() {
            return Err(ChainVerificationError::WrongWriter {
                stream_sequence,
                expected: self.key.writer().clone(),
            });
        }
        let observed = chain_id(integrity).ok_or(ChainVerificationError::InvalidChainId { stream_sequence })?;
        if observed != self.key.chain() {
            return Err(ChainVerificationError::WrongChain {
                stream_sequence,
                expected: self.key.chain(),
                observed,
            });
        }
        Ok(())
    }

    fn link(
        &mut self,
        stream_sequence: u64,
        record: &AuditRecord,
        integrity: &Integrity,
        hash: RecordHash,
    ) -> Result<VerifiedChainHead, ChainVerificationError> {
        let sequence = integrity.sequence;
        let Some(previous) = self.head.take() else {
            check_genesis(stream_sequence, record, integrity)?;
            return Ok(VerifiedChainHead::new(
                self.key.clone(),
                sequence,
                hash,
                stream_sequence,
            ));
        };
        if sequence == previous.sequence() {
            if hash != previous.hash() {
                return Err(ChainVerificationError::ConflictingDuplicate {
                    stream_sequence,
                    sequence,
                    previous_stream_sequence: previous.stream_sequence(),
                });
            }
            self.notices.push(ChainNotice::IdenticalDuplicate {
                sequence,
                stream_sequence,
                previous_stream_sequence: previous.stream_sequence(),
            });
            return Ok(previous.restored_at(stream_sequence));
        }
        let expected = previous
            .sequence()
            .checked_add(1)
            .ok_or(ChainVerificationError::SequenceOverflow {
                stream_sequence,
                previous: previous.sequence(),
            })?;
        if sequence > expected {
            return Err(ChainVerificationError::SequenceGap {
                stream_sequence,
                expected,
                observed: sequence,
            });
        }
        if sequence < expected {
            return Err(ChainVerificationError::SequenceRegression {
                stream_sequence,
                expected,
                observed: sequence,
            });
        }
        let previous_hash = integrity
            .previous_hash
            .as_deref()
            .ok_or(ChainVerificationError::MissingPreviousHash {
                stream_sequence,
                sequence,
            })?;
        if RecordHash::from_slice(previous_hash) != Ok(previous.hash()) {
            return Err(ChainVerificationError::BrokenLink {
                stream_sequence,
                sequence,
            });
        }
        Ok(VerifiedChainHead::new(
            self.key.clone(),
            sequence,
            hash,
            stream_sequence,
        ))
    }
}

fn chain_id(integrity: &Integrity) -> Option<ChainId> {
    ChainId::parse(integrity.chain_id.as_deref()?).ok()
}

fn check_genesis(
    stream_sequence: u64,
    record: &AuditRecord,
    integrity: &Integrity,
) -> Result<(), ChainVerificationError> {
    if integrity.sequence != GENESIS_SEQUENCE {
        return Err(ChainVerificationError::MissingGenesis {
            stream_sequence,
            observed: integrity.sequence,
        });
    }
    if integrity.previous_hash.is_some() {
        return Err(ChainVerificationError::GenesisPreviousHash { stream_sequence });
    }
    let opens_chain = matches!(
        &record.detail,
        Some(Detail::WriterLifecycle(lifecycle)) if matches!(
            lifecycle.kind.as_known(),
            Some(WriterLifecycleKind::WRITER_LIFECYCLE_KIND_CHAIN_RESET | WriterLifecycleKind::WRITER_LIFECYCLE_KIND_START)
        )
    );
    if !opens_chain {
        return Err(ChainVerificationError::GenesisNotLifecycle { stream_sequence });
    }
    Ok(())
}

/// The subject and headers ride outside the hash, so a privileged operator can
/// rewrite them; each one the reader supplied must say what the payload says.
fn check_metadata(
    stored: &StoredAuditRecord,
    record: &AuditRecord,
    integrity: &Integrity,
) -> Result<(), ChainVerificationError> {
    let stream_sequence = stored.stream_sequence();
    let sequence = integrity.sequence.to_string();
    let headers = [
        (NATS_MESSAGE_ID, stored.message_id(), Some(record.record_id.as_str())),
        (
            HeaderName::from_static(HEADER_WRITER),
            stored.writer(),
            Some(integrity.writer_id.as_str()),
        ),
        (
            HeaderName::from_static(HEADER_SEQUENCE),
            stored.sequence(),
            Some(sequence.as_str()),
        ),
        (
            HeaderName::from_static(HEADER_CHAIN),
            stored.chain(),
            integrity.chain_id.as_deref(),
        ),
    ];
    for (header, value, payload) in headers {
        if value.is_some_and(|value| Some(value) != payload) {
            return Err(ChainVerificationError::HeaderMismatch {
                stream_sequence,
                header,
            });
        }
    }
    if stored.subject().is_some_and(|subject| !subject_agrees(subject, record)) {
        return Err(ChainVerificationError::SubjectMismatch { stream_sequence });
    }
    Ok(())
}
