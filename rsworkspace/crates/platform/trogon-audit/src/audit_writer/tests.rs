use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_std::CapturedEvents;
use trogon_std::log_capture::CapturedLogs;
use trogon_std::log_capture::LevelFilter;
use trogon_std::time::FixedEpochClock;
use trogonai_proto::platform::audit::v1::WriterLifecycleKind;
use trogonai_proto::platform::audit::v1::audit_record::Detail;

use super::*;
use crate::chain_anchor::ChainAnchor;
use crate::chain_notice::ChainNotice;
use crate::constants::{MAX_NAME_BYTES, STREAM_AUDIT};
use crate::decision_outcome::DecisionOutcome;
use crate::enforcement::Enforcement;
use crate::fixtures::{
    SequentialIds, allow, best_effort, body, change, credential, decision_facts, deny, identity, name, scope, stamper,
    writer_id,
};
use crate::memory_sink::MemorySink;
use crate::read_only::ReadOnly;
use crate::record_hash::RecordHash;
use crate::redaction_canary::marker;
use crate::sink_error::SinkError;
use crate::verified_chain_range::VerifiedChainRange;
use crate::verify_chain_range::verify_chain_range;

type TestStamper = Stamper<FixedEpochClock, SequentialIds>;

fn config(best_effort_capacity: usize, retry_capacity: usize) -> WriterConfig {
    WriterConfig {
        best_effort_capacity,
        retry_capacity,
        drain_interval: Duration::from_secs(1),
        ..WriterConfig::default()
    }
}

fn writer_on<S: AuditSink>(sink: S, config: WriterConfig, stamper: &TestStamper) -> AuditWriter<S> {
    AuditWriter::new(sink, identity(), config, stamper)
}

fn setup(best_effort_capacity: usize, retry_capacity: usize) -> (AuditWriter<MemorySink>, MemorySink, TestStamper) {
    let sink = MemorySink::new();
    let stamper = stamper();
    let writer = writer_on(sink.clone(), config(best_effort_capacity, retry_capacity), &stamper);
    (writer, sink, stamper)
}

/// A writer whose chain reset is already stored.
async fn opened(
    best_effort_capacity: usize,
    retry_capacity: usize,
) -> (AuditWriter<MemorySink>, MemorySink, TestStamper) {
    let (writer, sink, stamper) = setup(best_effort_capacity, retry_capacity);
    writer.drain_once().await;
    assert_eq!(sink.stored().len(), 1);
    (writer, sink, stamper)
}

/// What the sink stored, verified as one chain from the writer's chain reset.
fn verified<S>(writer: &AuditWriter<S>, sink: &MemorySink) -> VerifiedChainRange {
    let anchor = ChainAnchor::Genesis {
        writer: writer_id(),
        chain: writer.inner.chain,
    };
    verify_chain_range(anchor, sink.stored_records()).unwrap()
}

fn sequences(sink: &MemorySink) -> Vec<u64> {
    sink.stored().iter().map(SealedRecord::sequence).collect()
}

fn ids(sink: &MemorySink) -> Vec<RecordId> {
    sink.stored().iter().map(SealedRecord::id).collect()
}

/// Records every record handed to it before passing it on, so a test can see
/// what each attempt carried.
#[derive(Debug, Clone, Default)]
struct RecordingSink {
    inner: MemorySink,
    attempts: Arc<StdMutex<Vec<SealedRecord>>>,
}

impl RecordingSink {
    fn attempts(&self) -> Vec<SealedRecord> {
        self.attempts.lock().unwrap().clone()
    }
}

impl AuditSink for RecordingSink {
    async fn publish(&self, record: &SealedRecord) -> Result<StoredAck, SinkError> {
        self.attempts.lock().unwrap().push(record.clone());
        self.inner.publish(record).await
    }
}

#[tokio::test]
async fn opens_its_chain_with_a_chain_reset_before_any_record() {
    let (writer, sink, stamper) = setup(4, 4);
    let record = allow(&stamper);
    let id = record.id();

    assert_eq!(writer.write(record).await, Ok(Disposition::Stored));

    let stored = sink.stored();
    assert_eq!(stored.len(), 2);
    assert_eq!(
        stored[0].subject(),
        "platform.v1.audit.integrity.egress_proxy.chain_reset"
    );
    assert_eq!(stored[0].sequence(), GENESIS_SEQUENCE);
    assert_eq!(stored[0].previous_hash(), None);
    assert_eq!(stored[0].class(), AuditClass::Must);
    assert_eq!(stored[1].id(), id);
    assert_eq!(stored[1].previous_hash(), Some(stored[0].hash()));
    assert_eq!(verified(&writer, &sink).records(), 2);
}

#[tokio::test]
async fn readiness_probes_are_acknowledged_distinct_writer_lifecycle_records() {
    let (writer, sink, stamper) = setup(4, 4);
    for _ in 0..2 {
        assert_eq!(writer.readiness_probe(stamper.stamp()).await, Ok(Disposition::Stored));
    }
    let stored = sink.stored();
    let records = sink.stored_records();
    assert_eq!(stored.len(), 3);
    assert_eq!(
        stored[0].subject(),
        "platform.v1.audit.integrity.egress_proxy.chain_reset"
    );
    assert_eq!(sequences(&sink), vec![1, 2, 3]);
    assert_ne!(stored[1].id(), stored[2].id());
    for (index, record) in records.iter().enumerate().skip(1) {
        assert_eq!(
            stored[index].subject(),
            "platform.v1.audit.integrity.egress_proxy.readiness_probe"
        );
        assert_eq!(stored[index].class(), AuditClass::Must);
        assert_eq!(stored[index].writer(), identity().writer());
        assert_eq!(stored[index].chain(), writer.chain());
        assert_eq!(stored[index].previous_hash(), Some(stored[index - 1].hash()));
        let message = body(record);
        assert!(message.attribution.is_unset());
        assert_eq!(message.tenant.value, identity().tenant().as_str());
        assert_eq!(message.project.value, identity().project().as_str());
        let Some(Detail::WriterLifecycle(lifecycle)) = message.detail.as_ref() else {
            panic!("readiness lifecycle");
        };
        assert_eq!(lifecycle.kind.as_known(), Some(WriterLifecycleKind::ReadinessProbe));
        assert!(lifecycle.reset_reason.is_none());
    }
    assert_eq!(verified(&writer, &sink).records(), 3);
    sink.fail_all(SinkError::Unreachable);
    assert_eq!(
        writer.readiness_probe(stamper.stamp()).await.unwrap_err().reason(),
        FailureReason::Unreachable
    );
    assert_eq!(sink.stored().len(), 3);
    assert!(writer.is_degraded());
}

#[tokio::test]
async fn every_writer_instance_opens_a_new_chain() {
    let stamper = stamper();
    let first = writer_on(MemorySink::new(), config(4, 4), &stamper);
    let restarted = writer_on(MemorySink::new(), config(4, 4), &stamper);

    assert_ne!(first.chain(), restarted.chain());
    assert_eq!(first.inner.identity, restarted.inner.identity);
}

#[tokio::test]
async fn reports_the_last_acknowledged_link_as_its_head() {
    let (writer, sink, stamper) = setup(4, 4);
    assert_eq!(writer.head(), None);

    writer.write(allow(&stamper)).await.unwrap();

    let head = writer.head().unwrap();
    let verified = verified(&writer, &sink);
    assert_eq!(head.sequence(), verified.head().sequence());
    assert_eq!(head.hash(), verified.head().hash());
    assert_eq!(head.stream(), STREAM_AUDIT);
    assert_eq!(head.stream_sequence(), verified.head().stream_sequence());
}

#[tokio::test]
async fn a_must_record_is_acknowledged_before_write_returns() {
    let (writer, sink, stamper) = setup(4, 4);
    let record = allow(&stamper);
    let id = record.id();

    assert_eq!(writer.write(record).await, Ok(Disposition::Stored));

    assert_eq!(ids(&sink)[1], id);
    assert_eq!(writer.stats().refused_total(), 0);
}

#[tokio::test]
async fn credential_and_change_records_are_written_before_their_effect() {
    let (writer, sink, stamper) = setup(4, 4);
    assert_eq!(writer.write(credential(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(writer.write(change(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(sink.stored().len(), 3);

    sink.fail_all(SinkError::Unreachable);
    assert!(writer.write(credential(&stamper)).await.is_err());
    assert!(writer.write(change(&stamper)).await.is_err());
}

#[tokio::test]
async fn sequences_rise_by_one_per_record() {
    let (writer, sink, stamper) = setup(4, 4);
    for _ in 0..3 {
        writer.write(allow(&stamper)).await.unwrap();
    }
    assert_eq!(sequences(&sink), vec![1, 2, 3, 4]);
    assert!(sink.stored().iter().all(|record| record.writer() == &writer_id()));
    assert!(sink.stored().iter().all(|record| record.chain() == writer.chain()));
    assert_eq!(verified(&writer, &sink).records(), 4);
}

#[tokio::test]
async fn a_must_record_waits_for_the_chain_reset_and_is_refused_when_it_cannot_be_stored() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_next(1, SinkError::Unreachable);

    let refused = writer.write(allow(&stamper)).await.unwrap_err();

    assert_eq!(refused.reason(), FailureReason::Unreachable);
    assert!(sink.stored().is_empty());
    assert_eq!(sink.attempts(), 1, "only the chain reset was tried");
    assert!(writer.is_degraded());

    writer.drain_once().await;
    assert!(!writer.is_degraded());
    assert_eq!(writer.write(allow(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(sequences(&sink), vec![1, 2], "the refused record was never numbered");
    assert_eq!(verified(&writer, &sink).records(), 2);
}

#[tokio::test]
async fn an_unacknowledged_must_record_refuses_the_action_with_the_reason() {
    for (error, reason) in [
        (SinkError::Unreachable, FailureReason::Unreachable),
        (SinkError::Timeout, FailureReason::Timeout),
        (SinkError::StreamFull, FailureReason::StreamFull),
    ] {
        let (writer, sink, stamper) = opened(4, 4).await;
        sink.fail_all(error);

        let refused = writer.write(allow(&stamper)).await.unwrap_err();

        assert_eq!(refused.reason(), reason);
        assert_eq!(sink.stored().len(), 1);
        assert_eq!(writer.stats().refused(reason), 1);
        assert_eq!(writer.stats().refused_total(), 1);
        assert_eq!(writer.stats().retry_buffered(), 0);
        assert!(writer.is_degraded(), "the refused record is the chain's head");
    }
}

#[tokio::test]
async fn a_refused_record_stays_the_head_and_is_stored_once_the_stream_recovers() {
    let (writer, sink, stamper) = opened(4, 4).await;
    sink.fail_next(1, SinkError::Unreachable);
    let head = allow(&stamper);
    let head_id = head.id();
    writer.write(head).await.unwrap_err();

    let attempts = sink.attempts();
    let refused = writer.write(allow(&stamper)).await.unwrap_err();
    assert_eq!(refused.reason(), FailureReason::Degraded);
    assert_eq!(sink.attempts(), attempts, "a degraded writer does not try the stream");

    writer.drain_once().await;
    assert!(!writer.is_degraded());
    let next = allow(&stamper);
    let next_id = next.id();
    assert_eq!(writer.write(next).await, Ok(Disposition::Stored));

    assert_eq!(&ids(&sink)[1..], &[head_id, next_id]);
    assert_eq!(verified(&writer, &sink).records(), 3);
}

#[tokio::test]
async fn a_retry_publishes_the_same_bytes_under_the_same_message_id() {
    let sink = RecordingSink::default();
    let stamper = stamper();
    let writer = writer_on(sink.clone(), config(4, 4), &stamper);
    writer.drain_once().await;
    sink.inner.fail_next(1, SinkError::Timeout);

    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert_eq!(writer.stats().retry_buffered(), 0, "the deny is the chain's head");
    writer.drain_once().await;

    let attempts = sink.attempts();
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[1], attempts[2]);
    assert_eq!(attempts[1].payload(), attempts[2].payload());
    assert_eq!(attempts[1].headers(), attempts[2].headers());
    assert_eq!(verified(&writer, &sink.inner).records(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_caller_that_stops_waiting_mid_publish_leaves_no_gap() {
    let (writer, sink, stamper) = opened(4, 4).await;
    sink.hold();
    let abandoned = allow(&stamper);
    let abandoned_id = abandoned.id();

    let waited = tokio::time::timeout(Duration::from_millis(10), writer.write(abandoned)).await;
    assert!(waited.is_err(), "the publish is held");
    assert!(!writer.is_degraded());

    sink.release();
    let next = allow(&stamper);
    let next_id = next.id();
    assert_eq!(writer.write(next).await, Ok(Disposition::Stored));

    assert_eq!(&ids(&sink)[1..], &[abandoned_id, next_id]);
    assert_eq!(verified(&writer, &sink).records(), 3);
}

#[tokio::test]
async fn a_blocked_head_degrades_the_writer_and_refuses_effects_waiting_behind_it() {
    let (writer, sink, stamper) = opened(4, 4).await;
    sink.hold();
    sink.fail_next(1, SinkError::Unreachable);
    let head = allow(&stamper);
    let head_id = head.id();
    let standing = deny(&stamper);
    let standing_id = standing.id();

    let (first, behind, refusal, ()) = tokio::join!(
        writer.write(head),
        writer.write(allow(&stamper)),
        writer.write(standing),
        async {
            tokio::task::yield_now().await;
            sink.release();
        }
    );

    assert_eq!(first.unwrap_err().reason(), FailureReason::Unreachable);
    assert_eq!(behind.unwrap_err().reason(), FailureReason::Degraded);
    assert_eq!(refusal, Ok(Disposition::Retrying));
    assert_eq!(sink.attempts(), 2, "nothing waiting behind the head tried the stream");
    assert!(writer.is_degraded());

    writer.drain_once().await;

    assert!(!writer.is_degraded());
    assert_eq!(&ids(&sink)[1..], &[head_id, standing_id]);
    assert_eq!(verified(&writer, &sink).records(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_mixed_writes_are_stored_as_one_gap_free_chain() {
    let (writer, sink, stamper) = setup(64, 64);
    let tasks: Vec<_> = (0..48)
        .map(|index| {
            let writer = writer.clone();
            let record = match index % 3 {
                0 => allow(&stamper),
                1 => deny(&stamper),
                _ => best_effort(&stamper),
            };
            tokio::spawn(async move { writer.write(record).await })
        })
        .collect();
    for task in tasks {
        assert!(task.await.unwrap().is_ok());
    }

    writer.drain_once().await;

    let verified = verified(&writer, &sink);
    assert_eq!(verified.records(), 49);
    assert!(verified.notices().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writes_against_a_failing_stream_still_leave_a_gap_free_chain() {
    let (writer, sink, stamper) = opened(64, 64).await;
    sink.fail_next(5, SinkError::Timeout);
    let tasks: Vec<_> = (0..30)
        .map(|index| {
            let writer = writer.clone();
            let record = if index % 2 == 0 {
                allow(&stamper)
            } else {
                deny(&stamper)
            };
            let id = record.id();
            tokio::spawn(async move { (id, writer.write(record).await) })
        })
        .collect();
    let mut outcomes = Vec::new();
    for task in tasks {
        outcomes.push(task.await.unwrap());
    }

    for _ in 0..=5 {
        writer.drain_once().await;
    }

    let stored = ids(&sink);
    for (id, outcome) in outcomes {
        if matches!(outcome, Ok(Disposition::Stored | Disposition::Retrying)) {
            assert!(stored.contains(&id), "a stored or standing record is in the chain");
        }
    }
    assert!(!writer.is_degraded());
    assert_eq!(verified(&writer, &sink).records(), stored.len());
}

#[tokio::test]
async fn a_deny_is_written_inline_when_the_stream_is_healthy() {
    let (writer, sink, stamper) = setup(4, 4);
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(sink.stored().len(), 2);
}

#[tokio::test]
async fn a_deny_stands_when_the_stream_is_down_and_is_retried_with_the_same_id() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_all(SinkError::Unreachable);
    let record = deny(&stamper);
    let id = record.id();

    assert_eq!(writer.write(record).await, Ok(Disposition::Retrying));
    assert_eq!(writer.stats().retry_buffered(), 1);
    assert!(writer.is_degraded(), "the chain reset is waiting");

    writer.drain_once().await;
    assert_eq!(writer.stats().retry_buffered(), 1);

    sink.recover();
    writer.drain_once().await;

    assert_eq!(writer.stats().retry_buffered(), 0);
    assert!(!writer.is_degraded());
    assert_eq!(ids(&sink)[1], id);
    assert_eq!(verified(&writer, &sink).records(), 2);
}

#[tokio::test]
async fn a_retry_after_a_lost_ack_does_not_store_the_deny_twice() {
    let (writer, sink, stamper) = opened(4, 4).await;
    sink.lose_next_acks(1);
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert_eq!(sink.stored().len(), 2);

    writer.drain_once().await;

    assert_eq!(sink.stored().len(), 2);
    assert_eq!(sink.attempts(), 3);
    assert_eq!(writer.stats().possible_duplicates(), 0);
    assert_eq!(
        writer.head().unwrap().stream_sequence(),
        2,
        "the stream's original copy"
    );
    assert!(verified(&writer, &sink).notices().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_retry_after_the_duplicate_window_is_counted_and_verifies_as_an_identical_duplicate() {
    let window = Duration::from_secs(10);
    let sink = MemorySink::new().with_duplicate_window(window);
    let stamper = stamper();
    let config = WriterConfig {
        duplicate_window: window,
        ..config(4, 4)
    };
    let writer = writer_on(sink.clone(), config, &stamper);
    writer.drain_once().await;
    sink.lose_next_acks(1);
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));

    tokio::time::advance(window + Duration::from_secs(1)).await;
    writer.drain_once().await;

    assert_eq!(writer.stats().possible_duplicates(), 1);
    assert_eq!(writer.head().unwrap().stream_sequence(), 3);
    let verified = verified(&writer, &sink);
    assert_eq!(
        verified.notices(),
        &[ChainNotice::IdenticalDuplicate {
            sequence: 2,
            stream_sequence: 3,
            previous_stream_sequence: 2,
        }]
    );
}

#[tokio::test]
async fn buffered_denies_keep_their_order() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_next(1, SinkError::Timeout);
    let first = deny(&stamper);
    let second = deny(&stamper);
    let order = vec![first.id(), second.id()];
    writer.write(first).await.unwrap();
    writer.write(second).await.unwrap();
    assert_eq!(writer.stats().retry_buffered(), 2);

    writer.drain_once().await;

    assert_eq!(sequences(&sink), vec![1, 2, 3]);
    assert_eq!(&ids(&sink)[1..], order.as_slice());
}

#[tokio::test]
async fn a_full_retry_buffer_is_audit_degraded_and_refuses_effects() {
    let (writer, sink, stamper) = setup(4, 2);
    sink.fail_all(SinkError::StreamFull);

    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert!(writer.is_degraded());
    assert!(writer.stats().degraded());

    let attempts_before = sink.attempts();
    let refused = writer.write(allow(&stamper)).await.unwrap_err();
    assert_eq!(refused.reason(), FailureReason::Degraded);
    assert_eq!(
        sink.attempts(),
        attempts_before,
        "a degraded writer does not try the stream"
    );
    assert_eq!(writer.stats().refused(FailureReason::Degraded), 1);
}

#[tokio::test]
async fn an_overflowed_retry_buffer_keeps_the_writer_degraded_until_it_empties() {
    let (writer, sink, stamper) = opened(4, 4).await;
    writer.state().overflowed = true;

    assert_eq!(
        writer.write(allow(&stamper)).await.unwrap_err().reason(),
        FailureReason::Degraded
    );
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert_eq!(writer.stats().retry_buffered(), 1);

    writer.drain_once().await;

    assert!(!writer.is_degraded());
    assert_eq!(verified(&writer, &sink).records(), 2);
}

#[tokio::test]
async fn a_deny_that_cannot_be_buffered_is_dropped_and_counted_and_still_stands() {
    let (writer, sink, stamper) = setup(4, 1);
    sink.fail_all(SinkError::Unreachable);

    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Dropped));
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Dropped));

    let stats = writer.stats();
    assert_eq!(stats.dropped(AuditClass::Must, FailureReason::BufferFull), 2);
    assert_eq!(stats.dropped_total(AuditClass::Must), 2);
    assert_eq!(stats.dropped_total(AuditClass::BestEffort), 0);
    assert_eq!(stats.retry_buffered(), 1);

    sink.recover();
    writer.drain_once().await;
    assert_eq!(
        verified(&writer, &sink).records(),
        2,
        "a drop before numbering leaves no gap"
    );
}

#[tokio::test]
async fn draining_the_buffer_ends_audit_degraded() {
    let (writer, sink, stamper) = setup(4, 2);
    sink.fail_all(SinkError::Unreachable);
    writer.write(deny(&stamper)).await.unwrap();
    writer.write(deny(&stamper)).await.unwrap();
    assert!(writer.is_degraded());

    writer.drain_once().await;
    assert!(writer.is_degraded(), "still down, still degraded");

    sink.recover();
    writer.drain_once().await;

    assert!(!writer.is_degraded());
    assert_eq!(sink.stored().len(), 3);
    assert_eq!(writer.write(allow(&stamper)).await, Ok(Disposition::Stored));
}

#[tokio::test]
async fn zero_capacities_are_read_as_one() {
    let (writer, sink, stamper) = setup(0, 0);
    sink.fail_all(SinkError::Unreachable);
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert!(writer.is_degraded());
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Dropped));

    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Queued));
    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Dropped));
}

#[tokio::test]
async fn a_best_effort_record_is_queued_and_never_waits_on_the_stream() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_all(SinkError::Unreachable);

    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Queued));

    assert_eq!(sink.attempts(), 0);
    assert_eq!(writer.stats().best_effort_queued(), 1);
}

#[tokio::test]
async fn a_read_only_allow_and_a_dry_run_allow_are_best_effort() {
    let (writer, _sink, stamper) = setup(4, 4);
    let read_only = Record::decision(
        scope(),
        decision_facts(DecisionOutcome::Allow(Enforcement::Enforce), ReadOnly::Declared),
        stamper.stamp(),
    );
    let dry_run = Record::decision(
        scope(),
        decision_facts(DecisionOutcome::Allow(Enforcement::DryRun), ReadOnly::Undeclared),
        stamper.stamp(),
    );
    assert_eq!(writer.write(read_only).await, Ok(Disposition::Queued));
    assert_eq!(writer.write(dry_run).await, Ok(Disposition::Queued));
}

#[tokio::test]
async fn a_full_queue_drops_the_newest_and_counts_it_without_refusing() {
    let (writer, sink, stamper) = setup(2, 4);
    let first = best_effort(&stamper);
    let first_id = first.id();
    assert_eq!(writer.write(first).await, Ok(Disposition::Queued));
    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Queued));
    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Dropped));
    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Dropped));

    let stats = writer.stats();
    assert_eq!(stats.dropped(AuditClass::BestEffort, FailureReason::QueueFull), 2);
    assert_eq!(stats.refused_total(), 0);
    assert_eq!(stats.best_effort_queued(), 2);
    assert!(!writer.is_degraded());

    writer.drain_once().await;
    assert_eq!(ids(&sink)[1], first_id);
    assert_eq!(verified(&writer, &sink).records(), 3);
}

#[tokio::test]
async fn a_best_effort_record_the_stream_will_not_take_stays_the_head_until_it_is_stored() {
    let (writer, sink, stamper) = opened(4, 4).await;
    writer.write(best_effort(&stamper)).await.unwrap();
    writer.write(best_effort(&stamper)).await.unwrap();
    sink.fail_next(1, SinkError::StreamFull);

    writer.drain_once().await;

    assert_eq!(sink.stored().len(), 1);
    let stats = writer.stats();
    assert_eq!(
        stats.dropped_total(AuditClass::BestEffort),
        0,
        "a numbered record is never dropped"
    );
    assert_eq!(stats.best_effort_queued(), 1);
    assert!(writer.is_degraded());

    writer.drain_once().await;

    assert_eq!(writer.stats().best_effort_queued(), 0);
    assert!(!writer.is_degraded());
    assert_eq!(verified(&writer, &sink).records(), 3);
}

#[tokio::test]
async fn a_failing_stream_costs_one_publish_per_drain() {
    let (writer, sink, stamper) = setup(4, 4);
    for _ in 0..3 {
        writer.write(best_effort(&stamper)).await.unwrap();
    }
    sink.fail_all(SinkError::Timeout);

    writer.drain_once().await;
    assert_eq!(sink.attempts(), 1);
    writer.drain_once().await;
    assert_eq!(sink.attempts(), 2);

    assert_eq!(writer.stats().best_effort_queued(), 3);
}

/// A record whose rule ids alone are larger than the stream takes.
fn oversized(stamper: &TestStamper, outcome: DecisionOutcome) -> Record {
    let mut facts = decision_facts(outcome, ReadOnly::Undeclared);
    let rule = name(&"r".repeat(MAX_NAME_BYTES));
    facts.rule_ids = vec![rule; MAX_RECORD_BYTES / MAX_NAME_BYTES + 1];
    Record::decision(scope(), facts, stamper.stamp())
}

#[tokio::test]
async fn a_record_larger_than_the_stream_takes_is_turned_away_before_it_is_numbered() {
    let (writer, sink, stamper) = opened(4, 4).await;

    let refused = writer
        .write(oversized(&stamper, DecisionOutcome::Allow(Enforcement::Enforce)))
        .await;
    let denied = writer.write(oversized(&stamper, DecisionOutcome::Deny)).await;
    let observed = writer.write(oversized(&stamper, DecisionOutcome::WouldDeny)).await;

    assert_eq!(refused.unwrap_err().reason(), FailureReason::TooLarge);
    assert_eq!(denied, Ok(Disposition::Dropped));
    assert_eq!(observed, Ok(Disposition::Dropped));
    let stats = writer.stats();
    assert_eq!(stats.refused(FailureReason::TooLarge), 1);
    assert_eq!(stats.dropped(AuditClass::Must, FailureReason::TooLarge), 1);
    assert_eq!(stats.dropped(AuditClass::BestEffort, FailureReason::TooLarge), 1);
    assert_eq!(sink.attempts(), 1, "only the chain reset reached the stream");
    assert!(!writer.is_degraded());

    assert_eq!(writer.write(allow(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(sequences(&sink), vec![1, 2]);
}

/// A writer whose chain reset is stored and whose chain is at its last sequence.
async fn exhausted() -> (AuditWriter<MemorySink>, MemorySink, TestStamper) {
    let (writer, sink, stamper) = opened(4, 4).await;
    *writer.inner.tip.lock().await = ChainTip::at(u64::MAX, RecordHash::of(b"last"));
    (writer, sink, stamper)
}

#[tokio::test]
async fn a_chain_out_of_sequences_refuses_effects_instead_of_wrapping() {
    let (writer, sink, stamper) = exhausted().await;

    let refused = writer.write(allow(&stamper)).await.unwrap_err();

    assert_eq!(refused.reason(), FailureReason::Degraded);
    assert!(writer.is_degraded());
    assert_eq!(sink.attempts(), 1);
}

#[tokio::test]
async fn a_chain_out_of_sequences_keeps_every_record_it_could_not_number() {
    let (writer, sink, stamper) = exhausted().await;

    assert_eq!(writer.write(best_effort(&stamper)).await, Ok(Disposition::Queued));
    writer.drain_once().await;
    assert_eq!(writer.stats().best_effort_queued(), 1);
    assert!(writer.is_degraded());

    writer.state().blocked = false;
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert_eq!(writer.stats().retry_buffered(), 1);
    writer.drain_once().await;
    assert_eq!(writer.stats().retry_buffered(), 1);
    assert_eq!(writer.stats().best_effort_queued(), 1);
    assert_eq!(sink.attempts(), 1);
}

#[tokio::test]
async fn the_writer_is_shared_between_clones() {
    let (writer, sink, stamper) = setup(4, 4);
    let other = writer.clone();
    other.write(allow(&stamper)).await.unwrap();
    writer.write(allow(&stamper)).await.unwrap();
    assert_eq!(sequences(&sink), vec![1, 2, 3]);
}

#[tokio::test]
async fn a_poisoned_lock_does_not_take_audit_down() {
    let (writer, sink, stamper) = setup(4, 4);
    let poisoner = writer.clone();
    let _ = std::thread::spawn(move || {
        let _held = poisoner.inner.state.lock().unwrap();
        panic!("poison the lock");
    })
    .join();
    assert_eq!(writer.write(allow(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(sink.stored().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_drain_task_stores_the_chain_reset_as_soon_as_it_runs() {
    let (writer, sink, _stamper) = setup(4, 4);
    let shutdown = CancellationToken::new();
    let task = writer.spawn_drain(shutdown.clone());

    tokio::time::sleep(Duration::from_millis(1)).await;

    assert_eq!(sequences(&sink), vec![GENESIS_SEQUENCE]);
    assert_eq!(writer.head().unwrap().sequence(), GENESIS_SEQUENCE);
    shutdown.cancel();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_drain_task_publishes_a_queued_record_as_soon_as_it_is_written() {
    let (writer, sink, stamper) = opened(4, 4).await;
    let shutdown = CancellationToken::new();
    let task = writer.spawn_drain(shutdown.clone());
    tokio::time::sleep(Duration::from_millis(1)).await;

    writer.write(best_effort(&stamper)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1)).await;

    assert_eq!(sink.stored().len(), 2);
    shutdown.cancel();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_drain_task_retries_the_head_every_interval() {
    let (writer, sink, stamper) = setup(4, 4);
    let shutdown = CancellationToken::new();
    let task = writer.spawn_drain(shutdown.clone());
    sink.fail_all(SinkError::Unreachable);
    writer.write(deny(&stamper)).await.unwrap();

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(sink.stored().is_empty());
    assert_eq!(writer.stats().retry_buffered(), 1);

    sink.recover();
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(sink.stored().len(), 2);
    assert_eq!(writer.stats().retry_buffered(), 0);
    shutdown.cancel();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_clean_stop_drains_the_head_and_the_buffers() {
    let (writer, sink, stamper) = setup(4, 4);
    writer.write(best_effort(&stamper)).await.unwrap();
    sink.fail_all(SinkError::Unreachable);
    writer.write(deny(&stamper)).await.unwrap();
    sink.recover();

    let shutdown = CancellationToken::new();
    shutdown.cancel();
    writer.run(shutdown).await;

    assert_eq!(writer.stats().retry_buffered(), 0);
    assert_eq!(writer.stats().best_effort_queued(), 0);
    assert_eq!(verified(&writer, &sink).records(), 3);
}

fn field_of(captured: &CapturedEvents, message: &str, field: &str) -> Option<String> {
    captured
        .events()
        .iter()
        .find(|event| event.message() == Some(message))
        .and_then(|event| event.field(field).map(str::to_owned))
}

#[tokio::test]
async fn a_refusal_is_logged_with_the_reason_and_the_record_id() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_all(SinkError::StreamFull);
    let record = allow(&stamper);
    let id = record.id().to_string();

    writer.write(record).await.unwrap_err();

    let message = "audit unavailable, refusing the action";
    assert_eq!(field_of(&captured, message, "reason").as_deref(), Some("stream_full"));
    assert_eq!(field_of(&captured, message, "record_id"), Some(id));
}

#[tokio::test]
async fn a_dropped_deny_is_logged_as_an_error() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, sink, stamper) = setup(4, 1);
    sink.fail_all(SinkError::Unreachable);
    writer.write(deny(&stamper)).await.unwrap();
    writer.write(deny(&stamper)).await.unwrap();

    let message = "audit retry buffer is full, a must-audit record was dropped";
    assert_eq!(field_of(&captured, message, "reason").as_deref(), Some("buffer_full"));
}

#[tokio::test]
async fn a_head_that_cannot_be_stored_is_logged_with_its_sequence_and_the_reason() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, sink, _stamper) = setup(4, 4);
    sink.fail_all(SinkError::Timeout);

    writer.drain_once().await;

    let message = "the audit chain head could not be stored";
    assert_eq!(field_of(&captured, message, "reason").as_deref(), Some("timeout"));
    assert_eq!(field_of(&captured, message, "sequence").as_deref(), Some("1"));
}

#[tokio::test]
async fn a_dropped_best_effort_record_is_logged_with_the_reason() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, _sink, stamper) = setup(1, 4);
    writer.write(best_effort(&stamper)).await.unwrap();
    writer.write(best_effort(&stamper)).await.unwrap();

    let message = "a best-effort audit record was dropped";
    assert_eq!(field_of(&captured, message, "reason").as_deref(), Some("queue_full"));
}

#[tokio::test]
async fn an_oversized_record_that_is_dropped_is_logged_with_the_reason() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, _sink, stamper) = setup(4, 4);
    writer.write(oversized(&stamper, DecisionOutcome::Deny)).await.unwrap();

    let message = "an audit record is larger than the stream takes and was dropped";
    assert_eq!(field_of(&captured, message, "reason").as_deref(), Some("too_large"));
}

#[tokio::test(start_paused = true)]
async fn a_possible_duplicate_is_logged_with_its_sequence() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let window = Duration::from_secs(10);
    let sink = MemorySink::new().with_duplicate_window(window);
    let stamper = stamper();
    let config = WriterConfig {
        duplicate_window: window,
        ..config(4, 4)
    };
    let writer = writer_on(sink.clone(), config, &stamper);
    sink.lose_next_acks(1);
    writer.drain_once().await;
    tokio::time::advance(window + Duration::from_secs(1)).await;
    writer.drain_once().await;

    let message = "an audit record was stored by a retry after the duplicate window, the stream may hold it twice";
    assert_eq!(field_of(&captured, message, "sequence").as_deref(), Some("1"));
}

#[tokio::test]
async fn an_exhausted_chain_is_logged_as_an_error() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, _sink, stamper) = exhausted().await;
    let record = allow(&stamper);
    let id = record.id().to_string();
    writer.write(record).await.unwrap_err();

    let message = "the audit chain has no sequence left, the writer is audit-degraded";
    assert_eq!(field_of(&captured, message, "record_id"), Some(id));
}

#[tokio::test]
async fn diagnostics_reach_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let (writer, sink, stamper) = setup(1, 1);
    sink.fail_all(SinkError::Unreachable);
    let refused = allow(&stamper);
    let refused_id = refused.id().to_string();
    writer.write(refused).await.unwrap_err();
    writer.write(deny(&stamper)).await.unwrap();
    writer.write(deny(&stamper)).await.unwrap();
    writer.write(best_effort(&stamper)).await.unwrap();
    writer.write(best_effort(&stamper)).await.unwrap();
    writer.drain_once().await;

    let records = logs.records();
    let line = |needle: &str| {
        records
            .iter()
            .find(|record| record.message.contains(needle))
            .unwrap_or_else(|| panic!("no log line containing {needle:?}"))
            .message
            .clone()
    };
    assert!(line("refusing the action").contains(&refused_id));
    assert!(line("a must-audit record was dropped").contains("buffer_full"));
    assert!(line("could not be stored").contains("unreachable"));
    assert!(line("best-effort audit record was dropped").contains("queue_full"));
    assert!(records.iter().all(|record| !record.message.contains(marker())));
}

#[tokio::test(start_paused = true)]
async fn chain_diagnostics_reach_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let window = Duration::from_secs(10);
    let sink = MemorySink::new().with_duplicate_window(window);
    let stamper = stamper();
    let config = WriterConfig {
        duplicate_window: window,
        ..config(4, 4)
    };
    let writer = writer_on(sink.clone(), config, &stamper);
    let too_large = oversized(&stamper, DecisionOutcome::Deny);
    let too_large_id = too_large.id().to_string();
    writer.write(too_large).await.unwrap();
    sink.lose_next_acks(1);
    writer.drain_once().await;
    tokio::time::advance(window + Duration::from_secs(1)).await;
    writer.drain_once().await;
    let (exhausted, _sink, stamper) = exhausted().await;
    let unnumbered = deny(&stamper);
    let unnumbered_id = unnumbered.id().to_string();
    exhausted.write(unnumbered).await.unwrap();

    let records = logs.records();
    let line = |needle: &str| {
        records
            .iter()
            .find(|record| record.message.contains(needle))
            .unwrap_or_else(|| panic!("no log line containing {needle:?}"))
            .message
            .clone()
    };
    assert!(line("larger than the stream takes").contains(&too_large_id));
    assert!(line("may hold it twice").contains(&sink.stored()[0].id().to_string()));
    assert!(line("no sequence left").contains(&unnumbered_id));
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn a_write_can_run_on_a_multi_threaded_runtime() {
    let (writer, _sink, stamper) = setup(4, 4);
    let refusal = writer.write(deny(&stamper));
    let effect = writer.write(allow(&stamper));
    let drain = writer.drain_once();
    assert_send(&refusal);
    assert_send(&effect);
    assert_send(&drain);
}
