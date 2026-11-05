use async_nats::header::NATS_MESSAGE_ID;
use buffa::Message;
use trogonai_proto::platform::audit::v1::AuditRecord;

use super::*;
use crate::fixtures::{allow, best_effort, deny, stamper, writer_id};

#[test]
fn stamps_the_writer_and_sequence_into_the_payload_and_the_headers() {
    let stamper = stamper();
    let record = deny(&stamper);
    let id = record.id();
    let sealed = SealedRecord::seal(record, &writer_id(), 9);

    assert_eq!(sealed.id(), id);
    assert_eq!(sealed.writer(), &writer_id());
    assert_eq!(sealed.sequence(), 9);
    assert_eq!(sealed.class(), AuditClass::Must);
    assert_eq!(sealed.obligation(), Obligation::Stands);
    assert_eq!(sealed.subject(), "platform.v1.audit.decision.project_1.deny");

    let decoded = AuditRecord::decode_from_slice(sealed.payload()).unwrap();
    assert_eq!(decoded.record_id, id.to_string());
    assert_eq!(decoded.integrity.writer_id, "gateway-1");
    assert_eq!(decoded.integrity.sequence, 9);

    let headers = sealed.headers();
    assert_eq!(headers.get(NATS_MESSAGE_ID).unwrap().as_str(), id.to_string());
    assert_eq!(headers.get(HEADER_WRITER).unwrap().as_str(), "gateway-1");
    assert_eq!(headers.get(HEADER_SEQUENCE).unwrap().as_str(), "9");
}

#[test]
fn carries_the_class_the_record_kind_fixed() {
    let stamper = stamper();
    let must = SealedRecord::seal(allow(&stamper), &writer_id(), 1);
    assert_eq!(must.class(), AuditClass::Must);
    assert_eq!(must.obligation(), Obligation::BeforeEffect);
    let best = SealedRecord::seal(best_effort(&stamper), &writer_id(), 2);
    assert_eq!(best.class(), AuditClass::BestEffort);
    assert_eq!(best.obligation(), Obligation::BestEffort);
}

#[test]
fn the_payload_is_stable_across_clones() {
    let stamper = stamper();
    let sealed = SealedRecord::seal(allow(&stamper), &writer_id(), 1);
    assert_eq!(sealed.clone().payload(), sealed.payload());
}
