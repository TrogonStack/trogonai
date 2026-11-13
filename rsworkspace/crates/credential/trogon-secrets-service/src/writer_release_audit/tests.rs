use std::time::UNIX_EPOCH;

use buffa::Message;
use nkeys::XKey;
use tokio_util::sync::CancellationToken;
use trogon_audit::{
    AuditName, ChainAnchor, MemorySink, SealedRecord, SinkError, StoredAck, WriterConfig, WriterId, WriterIdentity,
    verify_chain_range,
};
use trogon_credential_domain::CredentialVersion;
use trogon_nats::NatsToken;
use trogonai_proto::platform::audit::v1::AuditRecord;

use super::*;
use crate::fixtures::{CORRELATION_ID, OWNER, request, tenant};
use crate::{DenialReason, ReleaseAuditRecordError, ReleaseRequest, RequesterXkey};

mod jetstream_tests;

fn writer_id() -> WriterId {
    WriterId::new("secrets-service:test").unwrap()
}

pub(super) fn audit_over<S: AuditSink>(sink: S) -> (WriterReleaseAudit<S>, AuditWriter<S>) {
    let identity = WriterIdentity::new(
        writer_id(),
        NatsToken::new("secrets_service").unwrap(),
        AuditName::new("platform").unwrap(),
        NatsToken::new("secrets").unwrap(),
    );
    let writer = AuditWriter::new(
        sink,
        identity,
        WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    );
    (WriterReleaseAudit::new(writer.clone()), writer)
}

fn audit() -> (WriterReleaseAudit<MemorySink>, AuditWriter<MemorySink>, MemorySink) {
    let sink = MemorySink::new();
    let (audit, writer) = audit_over(sink.clone());
    (audit, writer, sink)
}

pub(super) fn release_of(request: ReleaseRequest) -> ReleaseRecord {
    ReleaseRecord::new(
        request,
        CredentialVersion::new(1).unwrap(),
        RequesterXkey::parse(&XKey::new().public_key()).unwrap(),
        UNIX_EPOCH,
    )
}

pub(super) fn release() -> ReleaseRecord {
    release_of(request(&tenant()))
}

pub(super) fn denial() -> DenialRecord {
    DenialRecord::new(request(&tenant()), DenialReason::OwnerNotHeld)
}

/// The records stored after the writer's chain reset, once everything stored
/// is checked to be one gap-free chain.
fn records(sink: &MemorySink) -> Vec<SealedRecord> {
    let stored = sink.stored();
    let Some(genesis) = stored.first() else {
        return Vec::new();
    };
    let anchor = ChainAnchor::Genesis {
        writer: writer_id(),
        chain: genesis.chain(),
    };
    let verified = verify_chain_range(anchor, sink.stored_records()).unwrap();
    assert_eq!(verified.records(), stored.len());
    stored.into_iter().skip(1).collect()
}

fn decoded(record: &SealedRecord) -> AuditRecord {
    AuditRecord::decode_from_slice(record.payload()).unwrap()
}

#[tokio::test]
async fn a_release_the_stream_refuses_is_not_acknowledged() {
    let (audit, _writer, sink) = audit();
    sink.fail_all(SinkError::Unreachable);

    assert!(audit.record_release(&release()).await.is_err());
    assert!(records(&sink).is_empty());
}

#[tokio::test]
async fn a_release_whose_ack_is_lost_is_not_acknowledged() {
    let (audit, writer, sink) = audit();
    writer.drain_once().await;
    sink.lose_next_acks(1);

    assert!(audit.record_release(&release()).await.is_err());
}

#[derive(Clone)]
struct PanickingSink;

impl AuditSink for PanickingSink {
    async fn publish(&self, _record: &SealedRecord) -> Result<StoredAck, SinkError> {
        panic!("the sink panicked");
    }
}

#[tokio::test]
#[should_panic(expected = "the sink panicked")]
async fn a_writer_that_panics_is_not_reported_as_an_audit_outage() {
    let (audit, _writer) = audit_over(PanickingSink);

    let _ = audit.record_release(&release()).await;
}

#[test]
fn a_record_that_cannot_be_built_is_refused_naming_the_fact_it_could_not_carry() {
    let error = unrecordable(ReleaseAuditRecordError::Name {
        field: "correlation id",
        source: AuditName::new("use=1").unwrap_err(),
    });

    assert!(error.message().contains("correlation id"), "{error}");
}

#[tokio::test]
async fn a_stopped_writer_acknowledges_neither_a_release_nor_a_denial() {
    let (audit, writer, sink) = audit();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    writer.run(shutdown).await;

    assert!(audit.record_release(&release()).await.is_err());
    assert!(audit.record_denial(&denial()).await.is_err());
    assert!(records(&sink).is_empty());
}

#[tokio::test]
async fn a_stored_release_carries_the_correlation_id() {
    let (audit, _writer, sink) = audit();

    audit.record_release(&release()).await.unwrap();

    let stored = records(&sink);
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0].subject(),
        format!("platform.v1.audit.credential.{OWNER}.release")
    );
    assert_eq!(decoded(&stored[0]).request_id.as_deref(), Some(CORRELATION_ID));
}

#[tokio::test]
async fn a_denial_stands_when_the_stream_is_down_and_is_stored_once_it_returns() {
    let (audit, writer, sink) = audit();
    sink.fail_all(SinkError::Unreachable);

    audit.record_denial(&denial()).await.unwrap();
    assert!(records(&sink).is_empty());

    sink.recover();
    writer.drain_once().await;
    let stored = records(&sink);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].subject(), format!("platform.v1.audit.decision.{OWNER}.deny"));
    assert_eq!(decoded(&stored[0]).request_id.as_deref(), Some(CORRELATION_ID));
}
