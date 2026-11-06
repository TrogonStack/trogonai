use async_nats::header::NATS_MESSAGE_ID;
use buffa::Message;
use trogonai_proto::platform::audit::v1::AuditRecord;

use super::*;
use crate::chain_anchor::ChainAnchor;
use crate::fixtures::{CHAIN, allow, best_effort, chain_id, deny, sealed_at, stamper, writer_id};
use crate::stored_audit_record::StoredAuditRecord;
use crate::verify_chain_range::verify_chain_range;

#[test]
fn stamps_the_chain_position_into_the_payload_and_the_headers() {
    let stamper = stamper();
    let record = deny(&stamper);
    let id = record.id();
    let previous = RecordHash::of(b"previous");
    let sealed = SealedRecord::seal(record, &writer_id(), chain_id(CHAIN), 9, Some(previous));

    assert_eq!(sealed.id(), id);
    assert_eq!(sealed.writer(), &writer_id());
    assert_eq!(sealed.chain(), chain_id(CHAIN));
    assert_eq!(sealed.sequence(), 9);
    assert_eq!(sealed.previous_hash(), Some(previous));
    assert_eq!(sealed.class(), AuditClass::Must);
    assert_eq!(sealed.obligation(), Obligation::Stands);
    assert_eq!(sealed.subject(), "platform.v1.audit.decision.project_1.deny");

    let (body, hash) = record_seal::open(sealed.payload()).unwrap();
    assert_eq!(hash, sealed.hash());
    let hashed = AuditRecord::decode_from_slice(body).unwrap();
    assert_eq!(hashed.record_id, id.to_string());
    assert_eq!(hashed.integrity.writer_id, "gateway-1");
    assert_eq!(hashed.integrity.sequence, 9);
    assert_eq!(hashed.integrity.chain_id.as_deref(), Some(CHAIN));
    assert_eq!(
        hashed.integrity.previous_hash.as_deref(),
        Some(previous.as_bytes().as_slice())
    );
    assert_eq!(hashed.integrity.record_hash, None);

    let stored = AuditRecord::decode_from_slice(sealed.payload()).unwrap();
    assert_eq!(
        stored.integrity.record_hash.as_deref(),
        Some(hash.as_bytes().as_slice())
    );

    let headers = sealed.headers();
    assert_eq!(headers.get(NATS_MESSAGE_ID).unwrap().as_str(), id.to_string());
    assert_eq!(headers.get(HEADER_WRITER).unwrap().as_str(), "gateway-1");
    assert_eq!(headers.get(HEADER_SEQUENCE).unwrap().as_str(), "9");
    assert_eq!(headers.get(HEADER_CHAIN).unwrap().as_str(), CHAIN);
}

#[test]
fn a_record_linked_to_the_previous_hash_verifies_as_a_chain() {
    let stamper = stamper();
    let first = SealedRecord::seal(
        Record::chain_reset(&crate::fixtures::identity(), stamper.stamp()),
        &writer_id(),
        chain_id(CHAIN),
        1,
        None,
    );
    let second = SealedRecord::seal(deny(&stamper), &writer_id(), chain_id(CHAIN), 2, Some(first.hash()));
    let stored = [&first, &second].into_iter().zip(1..).map(|(sealed, stream_sequence)| {
        StoredAuditRecord::new(stream_sequence, sealed.payload().clone())
            .with_subject(sealed.subject())
            .with_headers(&sealed.headers())
    });

    let anchor = ChainAnchor::Genesis {
        writer: writer_id(),
        chain: chain_id(CHAIN),
    };
    let verified = verify_chain_range(anchor, stored).unwrap();
    assert_eq!(verified.head().sequence(), 2);
    assert_eq!(verified.head().hash(), second.hash());
    assert_eq!(verified.records_without_metadata(), 0);
}

#[test]
fn carries_the_class_the_record_kind_fixed() {
    let stamper = stamper();
    let must = sealed_at(allow(&stamper), 1);
    assert_eq!(must.class(), AuditClass::Must);
    assert_eq!(must.obligation(), Obligation::BeforeEffect);
    let best = sealed_at(best_effort(&stamper), 2);
    assert_eq!(best.class(), AuditClass::BestEffort);
    assert_eq!(best.obligation(), Obligation::BestEffort);
}

#[test]
fn the_payload_is_stable_across_clones() {
    let stamper = stamper();
    let sealed = sealed_at(allow(&stamper), 1);
    assert_eq!(sealed.clone().payload(), sealed.payload());
}
