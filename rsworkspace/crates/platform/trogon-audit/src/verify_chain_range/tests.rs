use async_nats::HeaderName;
use buffa::{EnumValue, MessageField};
use bytes::Bytes;
use trogonai_proto::platform::audit::v1::{WriterLifecycle, WriterLifecycleKind};

use super::*;
use crate::chain_id::ChainId;
use crate::chain_key::ChainKey;
use crate::chain_notice::ChainNotice;
use crate::constants::{
    HEADER_CHAIN, HEADER_SEQUENCE, HEADER_WRITER, MAX_CHAIN_RANGE_RECORDS, RECORD_HASH_BYTES, SEAL_TRAILER_BYTES,
};
use crate::fixtures::{
    CHAIN, OTHER_CHAIN, body, chain_builder, chain_builder_for, chain_id, deny, resealed, restored, stamper,
    stored_again, stored_headers, valid_chain, with_payload, writer_id,
};
use crate::record_hash::RecordHash;
use crate::seal_error::SealError;
use crate::verified_chain_head::VerifiedChainHead;
use crate::writer_id::WriterId;

fn genesis() -> ChainAnchor {
    chain_builder().genesis()
}

fn verify(records: Vec<StoredAuditRecord>) -> Result<VerifiedChainRange, ChainVerificationError> {
    verify_chain_range(genesis(), records)
}

fn head_of(records: &[StoredAuditRecord]) -> VerifiedChainHead {
    verify(records.to_vec()).unwrap().into_head()
}

fn flip(payload: &Bytes, index: usize) -> Bytes {
    let mut tampered = payload.to_vec();
    tampered[index] ^= 0x01;
    Bytes::from(tampered)
}

fn with_header(record: &StoredAuditRecord, name: &str, value: &str) -> StoredAuditRecord {
    let mut headers = stored_headers(record);
    headers.insert(name, value);
    restored(record, record.stream_sequence(), record.payload().clone(), &headers)
}

#[test]
fn verifies_a_chain_from_its_genesis() {
    let records = valid_chain(&mut chain_builder(), 4);
    let last = records[3].clone();

    let range = verify(records).unwrap();

    let head = range.head();
    assert_eq!(head.key(), &ChainKey::new(writer_id(), chain_id(CHAIN)));
    assert_eq!(head.sequence(), 4);
    assert_eq!(
        head.hash(),
        RecordHash::of(&last.payload()[..last.payload().len() - SEAL_TRAILER_BYTES])
    );
    assert_eq!(head.stream_sequence(), 4);
    assert_eq!(range.records(), 4);
    assert_eq!(range.records_without_metadata(), 0);
    assert!(range.notices().is_empty());
}

#[test]
fn a_chain_may_open_with_a_start_record() {
    let mut builder = chain_builder();
    let mut message = body(&builder.clone().chain_reset(stamper().stamp(), 1));
    message.detail = Some(
        WriterLifecycle {
            kind: EnumValue::from(WriterLifecycleKind::Start),
            reset_reason: None,
        }
        .into(),
    );
    let start = builder.message("platform.v1.audit.integrity.egress_proxy.start", message, 1);
    let next = builder.record(deny(&stamper()), 2);

    assert_eq!(verify(vec![start, next]).unwrap().head().sequence(), 2);
}

#[test]
fn an_edited_body_fails_its_hash() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    records[1] = with_payload(&records[1], flip(records[1].payload(), 10));

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::Unsealed {
            stream_sequence: 2,
            source: SealError::HashMismatch,
        })
    );
}

#[test]
fn an_edited_hash_fails_its_hash() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    let last = records[1].payload().len() - 1;
    records[1] = with_payload(&records[1], flip(records[1].payload(), last));

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::Unsealed {
            stream_sequence: 2,
            source: SealError::HashMismatch,
        })
    );
}

#[test]
fn a_rewritten_and_rehashed_record_breaks_the_next_link() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    let mut message = body(&records[1]);
    message.request_id = Some("rewritten".to_string());
    records[1] = resealed(&records[1], &message);

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::BrokenLink {
            stream_sequence: 3,
            sequence: 3,
        })
    );
}

#[test]
fn reordered_records_are_out_of_sequence() {
    let records = valid_chain(&mut chain_builder(), 4);
    let reordered = vec![
        records[0].clone(),
        stored_again(&records[2], 2),
        stored_again(&records[1], 3),
        records[3].clone(),
    ];

    assert_eq!(
        verify(reordered),
        Err(ChainVerificationError::SequenceGap {
            stream_sequence: 2,
            expected: 2,
            observed: 3,
        })
    );
}

#[test]
fn a_record_stored_again_later_goes_back_in_sequence() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    records.push(stored_again(&records[1], 4));

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::SequenceRegression {
            stream_sequence: 4,
            expected: 4,
            observed: 2,
        })
    );
}

#[test]
fn a_deleted_middle_record_leaves_a_gap() {
    let mut records = valid_chain(&mut chain_builder(), 4);
    records.remove(1);

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::SequenceGap {
            stream_sequence: 3,
            expected: 2,
            observed: 3,
        })
    );
}

#[test]
fn a_deleted_genesis_is_missing() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    records.remove(0);

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::MissingGenesis {
            stream_sequence: 2,
            observed: 2,
        })
    );
}

#[test]
fn an_identical_duplicate_is_a_notice() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    let copy = stored_again(&records[1], 3);
    let third = stored_again(&records[2], 4);
    records.truncate(2);
    records.extend([copy, third]);

    let range = verify(records).unwrap();

    assert_eq!(range.head().sequence(), 3);
    assert_eq!(range.head().stream_sequence(), 4);
    assert_eq!(range.records(), 4);
    assert_eq!(
        range.notices(),
        [ChainNotice::IdenticalDuplicate {
            sequence: 2,
            stream_sequence: 3,
            previous_stream_sequence: 2,
        }]
    );
}

#[test]
fn a_duplicate_head_moves_the_head_to_the_last_copy() {
    let mut records = valid_chain(&mut chain_builder(), 2);
    records.push(stored_again(&records[1], 5));
    let hash = head_of(&records[..2]).hash();

    let range = verify(records).unwrap();

    assert_eq!(range.head().sequence(), 2);
    assert_eq!(range.head().hash(), hash);
    assert_eq!(range.head().stream_sequence(), 5);
}

#[test]
fn a_conflicting_duplicate_is_a_fault() {
    let mut builder = chain_builder();
    let mut records = valid_chain(&mut builder, 2);
    let mut fork = builder.clone();
    let stamper = stamper();
    records.push(builder.record(deny(&stamper), 3));
    records.push(fork.record(deny(&stamper), 4));

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::ConflictingDuplicate {
            stream_sequence: 4,
            sequence: 3,
            previous_stream_sequence: 3,
        })
    );
}

#[test]
fn a_rewritten_header_disagrees_with_the_payload() {
    let other_record = "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9fff";
    let cases = [
        ("Nats-Msg-Id", other_record),
        (HEADER_WRITER, "gateway-2"),
        (HEADER_SEQUENCE, "3"),
        (HEADER_CHAIN, OTHER_CHAIN),
    ];
    for (header, value) in cases {
        let mut records = valid_chain(&mut chain_builder(), 3);
        records[1] = with_header(&records[1], header, value);

        assert_eq!(
            verify(records),
            Err(ChainVerificationError::HeaderMismatch {
                stream_sequence: 2,
                header: HeaderName::from_static(header),
            }),
            "{header}"
        );
    }
}

#[test]
fn a_rewritten_subject_disagrees_with_the_payload() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    let headers = stored_headers(&records[1]);
    records[1] = StoredAuditRecord::new(2, records[1].payload().clone())
        .with_subject("platform.v1.audit.decision.project_1.allow")
        .with_headers(&headers);

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::SubjectMismatch { stream_sequence: 2 })
    );
}

#[test]
fn records_without_metadata_verify_on_their_payload_and_are_counted() {
    let mut records: Vec<_> = valid_chain(&mut chain_builder(), 3)
        .iter()
        .map(|record| StoredAuditRecord::new(record.stream_sequence(), record.payload().clone()))
        .collect();
    records.push(stored_again(&records[2], 4));

    let range = verify(records).unwrap();

    assert_eq!(range.records(), 4);
    assert_eq!(range.records_without_metadata(), 4);
    assert_eq!(range.notices().len(), 1);
}

#[test]
fn a_record_of_another_writer_is_refused() {
    let records = valid_chain(&mut chain_builder_for("gateway-2", CHAIN), 2);

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::WrongWriter {
            stream_sequence: 1,
            expected: writer_id(),
        })
    );
}

#[test]
fn a_record_of_another_chain_is_refused() {
    let records = valid_chain(&mut chain_builder_for("gateway-1", OTHER_CHAIN), 2);

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::WrongChain {
            stream_sequence: 1,
            expected: chain_id(CHAIN),
            observed: chain_id(OTHER_CHAIN),
        })
    );
}

#[test]
fn a_record_without_a_valid_chain_id_is_refused() {
    for chain_id in [None, Some("not-a-chain".to_string())] {
        let records = valid_chain(&mut chain_builder(), 1);
        let mut message = body(&records[0]);
        message.integrity.get_or_insert_default().chain_id = chain_id;
        let stored = StoredAuditRecord::new(1, resealed(&records[0], &message).payload().clone());

        assert_eq!(
            verify(vec![stored]),
            Err(ChainVerificationError::InvalidChainId { stream_sequence: 1 })
        );
    }
}

#[test]
fn stream_sequences_must_increase() {
    let mut records = valid_chain(&mut chain_builder(), 3);
    records[2] = stored_again(&records[2], 2);

    assert_eq!(
        verify(records),
        Err(ChainVerificationError::StreamSequenceRegression {
            stream_sequence: 2,
            previous_stream_sequence: 2,
        })
    );
}

#[test]
fn continues_from_a_trusted_head() {
    let records = valid_chain(&mut chain_builder(), 6);
    let head = head_of(&records[..3]);

    let range = verify_chain_range(ChainAnchor::TrustedHead(head), records[3..].to_vec()).unwrap();

    assert_eq!(range.head(), &head_of(&records));
    assert_eq!(range.records(), 3);
}

#[test]
fn a_continuation_must_start_right_after_the_trusted_head() {
    let records = valid_chain(&mut chain_builder(), 6);
    let head = head_of(&records[..3]);

    assert_eq!(
        verify_chain_range(ChainAnchor::TrustedHead(head.clone()), records[4..].to_vec()),
        Err(ChainVerificationError::SequenceGap {
            stream_sequence: 5,
            expected: 4,
            observed: 5,
        })
    );
    assert_eq!(
        verify_chain_range(ChainAnchor::TrustedHead(head), records[2..].to_vec()),
        Err(ChainVerificationError::StreamSequenceRegression {
            stream_sequence: 3,
            previous_stream_sequence: 3,
        })
    );
}

#[test]
fn a_continuation_may_open_with_a_copy_of_the_trusted_head() {
    let records = valid_chain(&mut chain_builder(), 4);
    let head = head_of(&records[..2]);
    let continuation = vec![stored_again(&records[1], 3), stored_again(&records[2], 4)];

    let range = verify_chain_range(ChainAnchor::TrustedHead(head), continuation).unwrap();

    assert_eq!(range.head().sequence(), 3);
    assert_eq!(range.notices().len(), 1);
}

#[test]
fn an_empty_range_is_refused() {
    assert_eq!(verify(Vec::new()), Err(ChainVerificationError::EmptyRange));

    let head = head_of(&valid_chain(&mut chain_builder(), 2));
    assert_eq!(
        verify_chain_range(ChainAnchor::TrustedHead(head), Vec::new()),
        Err(ChainVerificationError::EmptyRange)
    );
}

#[test]
fn a_range_longer_than_the_bound_is_refused() {
    let length = u64::try_from(MAX_CHAIN_RANGE_RECORDS).unwrap() + 1;
    let records = valid_chain(&mut chain_builder(), length);

    assert_eq!(verify(records.clone()), Err(ChainVerificationError::RangeTooLong));
    assert!(verify(records[..MAX_CHAIN_RANGE_RECORDS].to_vec()).is_ok());
}

#[test]
fn a_record_without_its_trailer_is_unsealed() {
    let mut records = valid_chain(&mut chain_builder(), 2);
    let payload = records[1].payload().clone();
    records[1] = with_payload(&records[1], payload.slice(..payload.len() - SEAL_TRAILER_BYTES));

    assert_eq!(
        verify(records.clone()),
        Err(ChainVerificationError::Unsealed {
            stream_sequence: 2,
            source: SealError::MissingTrailer,
        })
    );

    records[1] = with_payload(&records[1], Bytes::from_static(&[1, 2, 3]));
    assert_eq!(
        verify(records),
        Err(ChainVerificationError::Unsealed {
            stream_sequence: 2,
            source: SealError::TooShort(3),
        })
    );
}

#[test]
fn a_sealed_body_that_is_not_a_record_is_malformed() {
    let (_, payload) = crate::record_seal::seal(Bytes::from_static(&[0xff]));

    assert!(matches!(
        verify(vec![StoredAuditRecord::new(1, payload)]),
        Err(ChainVerificationError::Malformed { stream_sequence: 1, .. })
    ));
}

#[test]
fn a_record_without_integrity_is_refused() {
    let records = valid_chain(&mut chain_builder(), 1);
    let mut message = body(&records[0]);
    message.integrity = MessageField::none();

    assert_eq!(
        verify(vec![resealed(&records[0], &message)]),
        Err(ChainVerificationError::MissingIntegrity { stream_sequence: 1 })
    );
}

#[test]
fn a_record_hash_inside_the_hashed_bytes_is_refused() {
    let records = valid_chain(&mut chain_builder(), 1);
    let mut message = body(&records[0]);
    message.integrity.get_or_insert_default().record_hash = Some(vec![0; RECORD_HASH_BYTES]);

    assert_eq!(
        verify(vec![resealed(&records[0], &message)]),
        Err(ChainVerificationError::HashInBody { stream_sequence: 1 })
    );
}

#[test]
fn a_genesis_with_a_previous_hash_is_refused() {
    let records = valid_chain(&mut chain_builder(), 1);
    let mut message = body(&records[0]);
    message.integrity.get_or_insert_default().previous_hash = Some(vec![0; RECORD_HASH_BYTES]);

    assert_eq!(
        verify(vec![resealed(&records[0], &message)]),
        Err(ChainVerificationError::GenesisPreviousHash { stream_sequence: 1 })
    );
}

#[test]
fn a_genesis_must_be_a_lifecycle_record_that_opens_a_chain() {
    let first_decision = chain_builder().record(deny(&stamper()), 1);
    assert_eq!(
        verify(vec![first_decision]),
        Err(ChainVerificationError::GenesisNotLifecycle { stream_sequence: 1 })
    );

    let records = valid_chain(&mut chain_builder(), 1);
    let mut message = body(&records[0]);
    message.detail = Some(WriterLifecycle::default().into());
    let unspecified = StoredAuditRecord::new(1, resealed(&records[0], &message).payload().clone());
    assert_eq!(
        verify(vec![unspecified]),
        Err(ChainVerificationError::GenesisNotLifecycle { stream_sequence: 1 })
    );
}

#[test]
fn a_later_record_must_name_the_previous_hash() {
    let records = valid_chain(&mut chain_builder(), 2);
    let mut message = body(&records[1]);
    message.integrity.get_or_insert_default().previous_hash = None;

    assert_eq!(
        verify(vec![records[0].clone(), resealed(&records[1], &message)]),
        Err(ChainVerificationError::MissingPreviousHash {
            stream_sequence: 2,
            sequence: 2,
        })
    );
}

#[test]
fn a_previous_hash_of_the_wrong_length_breaks_the_link() {
    let records = valid_chain(&mut chain_builder(), 2);
    let mut message = body(&records[1]);
    message.integrity.get_or_insert_default().previous_hash = Some(vec![0; RECORD_HASH_BYTES - 1]);

    assert_eq!(
        verify(vec![records[0].clone(), resealed(&records[1], &message)]),
        Err(ChainVerificationError::BrokenLink {
            stream_sequence: 2,
            sequence: 2,
        })
    );
}

#[test]
fn a_chain_cannot_pass_the_last_sequence() {
    let records = valid_chain(&mut chain_builder(), 2);
    let key = ChainKey::new(writer_id(), chain_id(CHAIN));
    let head = VerifiedChainHead::new(key, u64::MAX, RecordHash::of(b"head"), 1);

    assert_eq!(
        verify_chain_range(ChainAnchor::TrustedHead(head), vec![records[1].clone()]),
        Err(ChainVerificationError::SequenceOverflow {
            stream_sequence: 2,
            previous: u64::MAX,
        })
    );
}

#[test]
fn a_genesis_anchor_names_its_writer_and_chain() {
    let anchor = ChainAnchor::Genesis {
        writer: WriterId::new("gateway-1").unwrap(),
        chain: ChainId::parse(CHAIN).unwrap(),
    };

    assert_eq!(anchor, genesis());
}
