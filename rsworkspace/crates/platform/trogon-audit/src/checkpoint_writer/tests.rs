use std::num::NonZeroU64;
use std::time::Duration;

use async_trait::async_trait;
use ed25519_dalek::Signer;
use tokio_util::sync::CancellationToken;
use trogon_std::CapturedEvents;
use trogon_std::log_capture::{CapturedLogs, LevelFilter};
use trogon_std::time::FixedEpochClock;

use super::*;
use crate::chain_anchor::ChainAnchor;
use crate::checkpoint_sign_error::CheckpointSignError;
use crate::checkpoint_signature::CheckpointSignature;
use crate::constants::GENESIS_SEQUENCE;
use crate::disposition::Disposition;
use crate::fixtures::{SequentialIds, allow, best_effort, identity, now, signing_key, stamper, writer_id};
use crate::key_id::KeyId;
use crate::local_checkpoint_signer::LocalCheckpointSigner;
use crate::memory_sink::MemorySink;
use crate::sink_error::SinkError;
use crate::time_bounds::TimeBounds;
use crate::trusted_keys::TrustedKeys;
use crate::verify_chain_range::verify_chain_range;
use crate::verify_checkpoint::verify_checkpoint;
use crate::writer_config::WriterConfig;

type TestStamper = Stamper<FixedEpochClock, SequentialIds>;
type TestCheckpoints = CheckpointWriter<MemorySink, FixedEpochClock, SequentialIds>;

const INTERVAL: Duration = Duration::from_secs(60);
const TICK: Duration = Duration::from_millis(1);

/// A signer that never signs, as one whose key service is down does.
#[derive(Debug)]
struct RefusingSigner(CheckpointSignError);

#[async_trait]
impl CheckpointSigner for RefusingSigner {
    async fn sign(&self, _draft: &CheckpointDraft) -> Result<CheckpointSignature, CheckpointSignError> {
        Err(self.0)
    }
}

/// A signer that signs with one key and names another.
#[derive(Debug)]
struct MislabelledSigner;

#[async_trait]
impl CheckpointSigner for MislabelledSigner {
    async fn sign(&self, draft: &CheckpointDraft) -> Result<CheckpointSignature, CheckpointSignError> {
        let claimed = KeyId::from_verifying_key(signing_key(1).verifying_key());
        let signature = signing_key(2).sign(&draft.signing_input(&claimed));
        Ok(CheckpointSignature::new(claimed, signature))
    }
}

struct Setup {
    writer: AuditWriter<MemorySink>,
    records: MemorySink,
    checkpoints: TestCheckpoints,
    stored: MemorySink,
    stamper: TestStamper,
}

fn every(records: u64) -> CheckpointConfig {
    CheckpointConfig {
        every_records: NonZeroU64::new(records).unwrap(),
        interval: INTERVAL,
    }
}

fn setup_with(signer: Arc<dyn CheckpointSigner>, config: CheckpointConfig) -> Setup {
    let records = MemorySink::new();
    let stored = MemorySink::new();
    let stamper = stamper();
    let writer = AuditWriter::new(records.clone(), identity(), WriterConfig::default(), &stamper);
    let checkpoints = CheckpointWriter::new(&writer, stored.clone(), signer, config, stamper.clone());
    Setup {
        writer,
        records,
        checkpoints,
        stored,
        stamper,
    }
}

fn setup(config: CheckpointConfig) -> Setup {
    setup_with(Arc::new(LocalCheckpointSigner::new(signing_key(1))), config)
}

fn sequences(sink: &MemorySink) -> Vec<u64> {
    sink.checkpoints().iter().map(SignedCheckpoint::head_sequence).collect()
}

async fn write_allowed(setup: &Setup, count: usize) {
    for _ in 0..count {
        assert_eq!(setup.writer.write(allow(&setup.stamper)).await, Ok(Disposition::Stored));
        tokio::time::sleep(TICK).await;
    }
}

#[tokio::test(start_paused = true)]
async fn checkpoints_the_chain_reset_as_soon_as_it_is_stored_and_the_checkpoint_verifies() {
    let setup = setup(CheckpointConfig::default());
    let task = setup.checkpoints.spawn();

    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    assert_eq!(sequences(&setup.stored), vec![GENESIS_SEQUENCE]);
    assert_eq!(setup.checkpoints.stats().written(), 1);
    let anchor = ChainAnchor::Genesis {
        writer: writer_id(),
        chain: setup.writer.chain(),
    };
    let head = verify_chain_range(anchor, setup.records.stored_records())
        .unwrap()
        .into_head();
    let trusted = TrustedKeys::new([KeyId::from_verifying_key(signing_key(1).verifying_key())]);
    let checkpoint = &setup.stored.checkpoints()[0];
    let verified = verify_checkpoint(checkpoint.payload(), &head, &trusted, &TimeBounds::new(now(), now())).unwrap();
    assert_eq!(verified.id(), checkpoint.id());
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn checkpoints_a_head_already_stored_when_it_starts() {
    let setup = setup(CheckpointConfig::default());
    setup.writer.drain_once().await;

    let task = setup.checkpoints.spawn();
    tokio::time::sleep(TICK).await;

    assert_eq!(sequences(&setup.stored), vec![GENESIS_SEQUENCE]);
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn checkpoints_again_after_every_records_more() {
    let setup = setup(every(3));
    let task = setup.checkpoints.spawn();
    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    write_allowed(&setup, 7).await;

    assert_eq!(sequences(&setup.stored), vec![1, 4, 7]);
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn an_idle_head_is_signed_again_every_interval() {
    let setup = setup(CheckpointConfig::default());
    let task = setup.checkpoints.spawn();
    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    tokio::time::sleep(INTERVAL).await;
    assert_eq!(sequences(&setup.stored), vec![1, 1]);
    tokio::time::sleep(INTERVAL).await;
    assert_eq!(sequences(&setup.stored), vec![1, 1, 1]);

    let checkpoints = setup.stored.checkpoints();
    assert_ne!(
        checkpoints[0].id(),
        checkpoints[1].id(),
        "a heartbeat is a new checkpoint"
    );
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn the_interval_covers_records_short_of_the_count_and_restarts_from_each_checkpoint() {
    let setup = setup(every(10));
    let task = setup.checkpoints.spawn();
    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    write_allowed(&setup, 2).await;
    assert_eq!(sequences(&setup.stored), vec![1]);
    tokio::time::sleep(INTERVAL).await;
    assert_eq!(sequences(&setup.stored), vec![1, 3]);

    tokio::time::sleep(INTERVAL / 2).await;
    write_allowed(&setup, 10).await;
    assert_eq!(sequences(&setup.stored), vec![1, 3, 13]);
    tokio::time::sleep(INTERVAL / 2).await;
    assert_eq!(
        sequences(&setup.stored),
        vec![1, 3, 13],
        "the count checkpoint restarted the interval"
    );
    tokio::time::sleep(INTERVAL / 2).await;
    assert_eq!(sequences(&setup.stored), vec![1, 3, 13, 13]);
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn no_head_means_no_checkpoint() {
    let setup = setup(CheckpointConfig::default());
    let task = setup.checkpoints.spawn();

    tokio::time::sleep(INTERVAL * 2).await;
    drop(setup.writer);
    task.await.unwrap();

    assert!(setup.stored.checkpoints().is_empty());
    assert_eq!(setup.stored.attempts(), 0);
}

#[tokio::test(start_paused = true)]
async fn after_the_writers_final_drain_it_checkpoints_the_final_head_and_stops() {
    let setup = setup(CheckpointConfig::default());
    let shutdown = CancellationToken::new();
    let drain = setup.writer.spawn_drain(shutdown.clone());
    let task = setup.checkpoints.spawn();
    tokio::time::sleep(TICK).await;
    write_allowed(&setup, 2).await;

    shutdown.cancel();
    assert_eq!(
        setup.writer.write(best_effort(&setup.stamper)).await,
        Ok(Disposition::Queued)
    );
    drain.await.unwrap();
    task.await.unwrap();

    assert_eq!(setup.records.stored().len(), 4);
    assert_eq!(sequences(&setup.stored).last(), Some(&4));
}

#[tokio::test(start_paused = true)]
async fn a_dropped_writer_ends_the_task_with_a_checkpoint_of_its_last_head() {
    let setup = setup(every(100));
    let task = setup.checkpoints.spawn();
    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;
    write_allowed(&setup, 2).await;

    drop(setup.writer);
    task.await.unwrap();

    assert_eq!(sequences(&setup.stored), vec![1, 3]);
}

#[tokio::test(start_paused = true)]
async fn a_signer_that_fails_is_an_alert_that_never_holds_up_the_writer() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let setup = setup_with(
        Arc::new(RefusingSigner(CheckpointSignError::Unavailable)),
        CheckpointConfig::default(),
    );
    let task = setup.checkpoints.spawn();
    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    assert_eq!(setup.writer.write(allow(&setup.stamper)).await, Ok(Disposition::Stored));
    assert!(!setup.writer.is_degraded());
    tokio::time::sleep(INTERVAL).await;

    let stats = setup.checkpoints.stats();
    assert_eq!(stats.sign_failures(), 2, "one attempt per trigger, never a retry loop");
    assert_eq!(stats.written(), 0);
    assert_eq!(stats.publish_failures(), 0);
    assert_eq!(setup.stored.attempts(), 0);
    let events = captured.events();
    let alert = events
        .iter()
        .find(|event| event.message() == Some("an audit checkpoint could not be signed"))
        .expect("the failure is logged");
    assert_eq!(alert.field("reason"), Some("unavailable"));
    assert_eq!(alert.field("sequence"), Some("1"));
    assert_eq!(
        alert.field("chain"),
        Some(format!("gateway-1/{}", setup.writer.chain()).as_str())
    );
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_signature_that_does_not_verify_is_never_published() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let setup = setup_with(Arc::new(MislabelledSigner), CheckpointConfig::default());
    let task = setup.checkpoints.spawn();
    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    assert_eq!(setup.checkpoints.stats().sign_failures(), 1);
    assert_eq!(setup.stored.attempts(), 0);
    let events = captured.events();
    assert!(
        events
            .iter()
            .any(|event| event.field("reason") == Some("invalid_signature"))
    );
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn a_checkpoint_the_stream_refuses_is_counted_and_the_next_trigger_tries_again() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let setup = setup(CheckpointConfig::default());
    setup.stored.fail_all(SinkError::Unreachable);
    let task = setup.checkpoints.spawn();
    setup.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    assert_eq!(setup.checkpoints.stats().publish_failures(), 1);
    assert!(!setup.writer.is_degraded());
    let events = captured.events();
    let alert = events
        .iter()
        .find(|event| event.message() == Some("an audit checkpoint could not be stored"))
        .expect("the failure is logged");
    assert_eq!(alert.field("reason"), Some("unreachable"));
    assert!(alert.field("checkpoint_id").is_some());

    setup.stored.recover();
    tokio::time::sleep(INTERVAL).await;

    let stats = setup.checkpoints.stats();
    assert_eq!(stats.written(), 1);
    assert_eq!(stats.publish_failures(), 1);
    assert_eq!(sequences(&setup.stored), vec![1]);
    task.abort();
}

#[tokio::test(start_paused = true)]
async fn checkpoint_alerts_reach_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let unsigned = setup_with(
        Arc::new(RefusingSigner(CheckpointSignError::Unavailable)),
        CheckpointConfig::default(),
    );
    let refused = setup(CheckpointConfig::default());
    refused.stored.fail_all(SinkError::Unreachable);
    let unsigned_task = unsigned.checkpoints.spawn();
    let refused_task = refused.checkpoints.spawn();
    unsigned.writer.drain_once().await;
    refused.writer.drain_once().await;
    tokio::time::sleep(TICK).await;

    let records = logs.records();
    let line = |needle: &str| {
        records
            .iter()
            .find(|record| record.message.contains(needle))
            .unwrap_or_else(|| panic!("no log line containing {needle:?}"))
            .message
            .clone()
    };
    assert!(line("could not be signed").contains("unavailable"));
    assert!(line("could not be stored").contains("unreachable"));
    unsigned_task.abort();
    refused_task.abort();
}
