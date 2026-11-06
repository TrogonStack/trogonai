//! The writer and the sink against a real JetStream server, on the streams
//! provisioning builds, read back and verified the way an auditor would.

use std::num::NonZeroUsize;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::stream::Stream;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::audit_stream::AuditStream;
use crate::audit_writer::AuditWriter;
use crate::chain_anchor::ChainAnchor;
use crate::constants::STREAM_AUDIT;
use crate::disposition::Disposition;
use crate::ensure_audit_stream::ensure_audit_stream;
use crate::fixtures::{best_effort, change, credential, deny, identity, writer_id};
use crate::stored_audit_record::StoredAuditRecord;
use crate::verify_chain_range::verify_chain_range;
use crate::writer_config::WriterConfig;

async fn provisioned() -> (JetStreamTestServer, jetstream::Context, Stream) {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    ensure_audit_stream(&js, AuditStream::Audit, NonZeroUsize::MIN)
        .await
        .unwrap();
    let stream = js.get_stream(STREAM_AUDIT).await.unwrap();
    (server, js, stream)
}

async fn stored_count(stream: &Stream) -> u64 {
    stream.get_info().await.unwrap().state.messages
}

async fn read_back(stream: &Stream) -> Vec<StoredAuditRecord> {
    let last = stream.get_info().await.unwrap().state.last_sequence;
    let mut records = Vec::new();
    for sequence in 1..=last {
        let message = stream.get_raw_message(sequence).await.unwrap();
        records.push(
            StoredAuditRecord::new(message.sequence, message.payload)
                .with_subject(message.subject.as_str())
                .with_headers(&message.headers),
        );
    }
    records
}

#[tokio::test]
async fn a_written_chain_reads_back_from_the_stream_and_verifies() {
    let (_server, js, stream) = provisioned().await;
    let stamper = stamper();
    let writer = AuditWriter::new(
        JetStreamSink::with_defaults(js),
        identity(),
        WriterConfig::default(),
        &stamper,
    );

    assert_eq!(writer.write(allow(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(writer.write(credential(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(writer.write(change(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Queued));
    writer.drain_once().await;

    let anchor = ChainAnchor::Genesis {
        writer: writer_id(),
        chain: writer.chain(),
    };
    let verified = verify_chain_range(anchor, read_back(&stream).await).unwrap();
    assert_eq!(verified.records(), 6);
    assert_eq!(verified.records_without_metadata(), 0);
    assert!(verified.notices().is_empty());
    let head = writer.head().unwrap();
    assert_eq!(head.sequence(), verified.head().sequence());
    assert_eq!(head.hash(), verified.head().hash());
    assert_eq!(head.stream(), STREAM_AUDIT);
    assert_eq!(head.stream_sequence(), verified.head().stream_sequence());
}

#[tokio::test]
async fn a_sealed_record_is_stored_on_its_subject_under_its_record_id() {
    let (_server, js, stream) = provisioned().await;
    let sink = JetStreamSink::with_defaults(js);
    let record = sealed();

    sink.publish(&record).await.unwrap();

    let stored = stream.get_raw_message(1).await.unwrap();
    assert_eq!(stored.subject.as_str(), record.subject());
    assert_eq!(stored.payload, *record.payload());
    let id = stored.headers.get(NATS_MESSAGE_ID).unwrap().to_string();
    assert_eq!(id, record.id().to_string());
}

#[tokio::test]
async fn a_duplicate_is_acknowledged_at_the_stream_sequence_of_the_first_copy() {
    let (_server, js, stream) = provisioned().await;
    let sink = JetStreamSink::new(js, Duration::from_secs(5), NonZeroU32::MIN);
    let record = sealed();

    let first = sink.publish(&record).await.unwrap();
    let again = sink.publish(&record).await.unwrap();

    assert_eq!(first, StoredAck::new(STREAM_AUDIT, 1, false));
    assert_eq!(again, StoredAck::new(STREAM_AUDIT, 1, true));
    assert_eq!(stored_count(&stream).await, 1);
}

#[tokio::test]
async fn distinct_records_are_each_stored() {
    let (_server, js, stream) = provisioned().await;
    let sink = JetStreamSink::with_defaults(js);
    let stamper = stamper();

    sink.publish(&sealed_at(allow(&stamper), 1)).await.unwrap();
    sink.publish(&sealed_at(deny(&stamper), 2)).await.unwrap();

    assert_eq!(stored_count(&stream).await, 2);
}

#[tokio::test]
async fn a_full_stream_is_reported_as_full_and_not_as_unreachable() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let mut config = AuditStream::Audit.config(NonZeroUsize::MIN);
    config.max_messages = 1;
    let stream = js.create_stream(config).await.unwrap();
    let sink = JetStreamSink::with_defaults(js);
    let stamper = stamper();
    sink.publish(&sealed_at(allow(&stamper), 1)).await.unwrap();

    let refused = sink.publish(&sealed_at(deny(&stamper), 2)).await.unwrap_err();

    assert_eq!(refused, SinkError::StreamFull);
    assert_eq!(refused.reason(), crate::failure_reason::FailureReason::StreamFull);
    assert_eq!(stored_count(&stream).await, 1, "a full stream keeps the record it has");
}

#[tokio::test]
async fn a_stream_that_was_never_provisioned_is_unreachable() {
    let server = JetStreamTestServer::start().await;
    let sink = JetStreamSink::with_defaults(server.jetstream().await);

    assert_eq!(sink.publish(&sealed()).await, Err(SinkError::Unreachable));
}
