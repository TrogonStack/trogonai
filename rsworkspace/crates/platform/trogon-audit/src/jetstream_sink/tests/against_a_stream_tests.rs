//! The writer, the checkpoint task and the sink against a real JetStream
//! server, read back and verified the way an auditor would.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::stream::{Config, DiscardPolicy, RetentionPolicy, StorageType, Stream};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;

use super::*;
use crate::audit_writer::AuditWriter;
use crate::chain_anchor::ChainAnchor;
use crate::checkpoint_config::CheckpointConfig;
use crate::checkpoint_writer::CheckpointWriter;
use crate::constants::{STREAM_AUDIT, STREAM_AUDIT_CHECKPOINT, STREAM_DUPLICATE_WINDOW};
use crate::disposition::Disposition;
use crate::fixtures::{best_effort, change, credential, deny, identity, now, signing_key, writer_id};
use crate::key_id::KeyId;
use crate::local_checkpoint_signer::LocalCheckpointSigner;
use crate::stored_audit_record::StoredAuditRecord;
use crate::time_bounds::TimeBounds;
use crate::trusted_keys::TrustedKeys;
use crate::verify_chain_range::verify_chain_range;
use crate::verify_checkpoint::verify_checkpoint;
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

#[tokio::test]
async fn a_checkpoint_stored_beside_the_chain_reads_back_and_verifies_against_its_head() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = audit_stream(&js).await;
    let checkpoint_stream = js
        .create_stream(Config {
            name: STREAM_AUDIT_CHECKPOINT.to_owned(),
            subjects: vec!["platform.v1.audit.checkpoint.>".to_owned()],
            storage: StorageType::File,
            duplicate_window: STREAM_DUPLICATE_WINDOW,
            ..Config::default()
        })
        .await
        .unwrap();
    let stamper = stamper();
    let sink = JetStreamSink::with_defaults(js);
    let writer = AuditWriter::new(sink.clone(), identity(), WriterConfig::default(), &stamper);
    let signer = LocalCheckpointSigner::new(signing_key(1));
    let checkpoints = CheckpointWriter::new(
        &writer,
        sink,
        Arc::new(signer),
        CheckpointConfig::default(),
        stamper.clone(),
    );
    let shutdown = CancellationToken::new();
    let drain = writer.spawn_drain(shutdown.clone());
    let task = checkpoints.spawn();

    assert_eq!(writer.write(allow(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Stored));
    shutdown.cancel();
    drain.await.unwrap();
    task.await.unwrap();

    let anchor = ChainAnchor::Genesis {
        writer: writer_id(),
        chain: writer.chain(),
    };
    let head = verify_chain_range(anchor, read_back(&stream).await)
        .unwrap()
        .into_head();
    let last = checkpoint_stream.get_info().await.unwrap().state.last_sequence;
    let message = checkpoint_stream.get_raw_message(last).await.unwrap();
    let trusted = TrustedKeys::new([KeyId::from_verifying_key(signing_key(1).verifying_key())]);
    let bounds = TimeBounds::new(now(), now());

    let verified = verify_checkpoint(&message.payload, &head, &trusted, &bounds).unwrap();
    assert_eq!(verified.head().sequence(), 3);
    assert_eq!(checkpoints.stats().written(), last);
}
