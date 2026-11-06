//! The writer and the sink against a real JetStream server, read back and
//! verified the way an auditor would.

use std::time::Duration;

use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType, Stream};
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::audit_writer::AuditWriter;
use crate::chain_anchor::ChainAnchor;
use crate::constants::{STREAM_AUDIT, STREAM_DUPLICATE_WINDOW};
use crate::disposition::Disposition;
use crate::fixtures::{best_effort, change, credential, deny, identity, writer_id};
use crate::stored_audit_record::StoredAuditRecord;
use crate::verify_chain_range::verify_chain_range;
use crate::writer_config::WriterConfig;

/// The audit stream as provisioning creates it
/// (`https://github.com/TrogonStack/trogonai/pull/831`).
async fn audit_stream(js: &async_nats::jetstream::Context) -> Stream {
    js.create_stream(Config {
        name: STREAM_AUDIT.to_owned(),
        subjects: ["decision", "credential", "change", "integrity"]
            .map(|kind| format!("platform.v1.audit.{kind}.>"))
            .to_vec(),
        storage: StorageType::File,
        retention: RetentionPolicy::Limits,
        discard: DiscardPolicy::New,
        duplicate_window: STREAM_DUPLICATE_WINDOW,
        ..Config::default()
    })
    .await
    .unwrap()
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
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = audit_stream(&js).await;
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
async fn a_duplicate_is_acknowledged_at_the_stream_sequence_of_the_first_copy() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = audit_stream(&js).await;
    let sink = JetStreamSink::new(js, Duration::from_secs(5), NonZeroU32::MIN);
    let record = sealed();

    let first = sink.publish(&record).await.unwrap();
    let again = sink.publish(&record).await.unwrap();

    assert_eq!(first, StoredAck::new(STREAM_AUDIT, 1, false));
    assert_eq!(again, StoredAck::new(STREAM_AUDIT, 1, true));
    assert_eq!(stream.get_info().await.unwrap().state.messages, 1);
}
