use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_std::CapturedEvents;
use trogon_std::log_capture::CapturedLogs;
use trogon_std::log_capture::LevelFilter;
use trogon_std::time::FixedEpochClock;

use super::*;
use crate::decision_outcome::DecisionOutcome;
use crate::enforcement::Enforcement;
use crate::fixtures::{
    SequentialIds, allow, best_effort, change, credential, decision_facts, deny, scope, stamper, writer_id,
};
use crate::memory_sink::MemorySink;
use crate::read_only::ReadOnly;
use crate::redaction_canary::marker;
use crate::sink_error::SinkError;
use crate::stamper::Stamper;

type TestStamper = Stamper<FixedEpochClock, SequentialIds>;

fn config(best_effort_capacity: usize, retry_capacity: usize) -> WriterConfig {
    WriterConfig {
        best_effort_capacity,
        retry_capacity,
        drain_interval: Duration::from_secs(1),
    }
}

fn setup(best_effort_capacity: usize, retry_capacity: usize) -> (AuditWriter<MemorySink>, MemorySink, TestStamper) {
    let sink = MemorySink::new();
    let writer = AuditWriter::new(sink.clone(), writer_id(), config(best_effort_capacity, retry_capacity));
    (writer, sink, stamper())
}

#[tokio::test]
async fn a_must_record_is_acknowledged_before_write_returns() {
    let (writer, sink, stamper) = setup(4, 4);
    let record = allow(&stamper);
    let id = record.id();

    assert_eq!(writer.write(record).await, Ok(Disposition::Stored));

    let stored = sink.stored();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id(), id);
    assert_eq!(writer.stats().refused_total(), 0);
}

#[tokio::test]
async fn credential_and_change_records_are_written_before_their_effect() {
    let (writer, sink, stamper) = setup(4, 4);
    assert_eq!(writer.write(credential(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(writer.write(change(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(sink.stored().len(), 2);

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
    let sequences: Vec<u64> = sink.stored().iter().map(SealedRecord::sequence).collect();
    assert_eq!(sequences, vec![1, 2, 3]);
    assert!(sink.stored().iter().all(|record| record.writer() == &writer_id()));
}

#[tokio::test]
async fn an_unacknowledged_must_record_refuses_the_action_with_the_reason() {
    for (error, reason) in [
        (SinkError::Unreachable, FailureReason::Unreachable),
        (SinkError::Timeout, FailureReason::Timeout),
        (SinkError::StreamFull, FailureReason::StreamFull),
    ] {
        let (writer, sink, stamper) = setup(4, 4);
        sink.fail_all(error);

        let refused = writer.write(allow(&stamper)).await.unwrap_err();

        assert_eq!(refused.reason(), reason);
        assert!(sink.stored().is_empty());
        assert_eq!(writer.stats().refused(reason), 1);
        assert_eq!(writer.stats().refused_total(), 1);
        assert_eq!(writer.stats().retry_buffered(), 0);
        assert!(!writer.is_degraded());
    }
}

#[tokio::test]
async fn a_refused_action_is_not_buffered_for_later() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_next(1, SinkError::Unreachable);
    writer.write(allow(&stamper)).await.unwrap_err();
    writer.drain_once().await;
    assert!(sink.stored().is_empty());
}

#[tokio::test]
async fn a_deny_is_written_inline_when_the_stream_is_healthy() {
    let (writer, sink, stamper) = setup(4, 4);
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Stored));
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test]
async fn a_deny_stands_when_the_stream_is_down_and_is_retried_with_the_same_id() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_all(SinkError::Unreachable);
    let record = deny(&stamper);
    let id = record.id();

    assert_eq!(writer.write(record).await, Ok(Disposition::Retrying));
    assert_eq!(writer.stats().retry_buffered(), 1);
    assert!(!writer.is_degraded());

    writer.drain_once().await;
    assert_eq!(writer.stats().retry_buffered(), 1);

    sink.recover();
    writer.drain_once().await;

    assert_eq!(writer.stats().retry_buffered(), 0);
    let stored = sink.stored();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].id(), id);
}

#[tokio::test]
async fn a_retry_after_a_lost_ack_does_not_store_the_deny_twice() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.lose_next_acks(1);
    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert_eq!(sink.stored().len(), 1);

    writer.drain_once().await;

    assert_eq!(sink.stored().len(), 1);
    assert_eq!(sink.attempts(), 2);
    assert_eq!(writer.stats().retry_buffered(), 0);
}

#[tokio::test]
async fn buffered_denies_keep_their_order() {
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_next(1, SinkError::Timeout);
    writer.write(deny(&stamper)).await.unwrap();
    writer.write(deny(&stamper)).await.unwrap();
    assert_eq!(writer.stats().retry_buffered(), 2);

    writer.drain_once().await;

    let sequences: Vec<u64> = sink.stored().iter().map(SealedRecord::sequence).collect();
    assert_eq!(sequences, vec![1, 2]);
}

/// Holds the first publish open until released, then fails it, so a second
/// write can run while the first is between its publish and the retry buffer.
#[derive(Debug, Clone)]
struct HeldFirstPublish {
    inner: MemorySink,
    release: Arc<tokio::sync::Notify>,
    held: Arc<std::sync::atomic::AtomicBool>,
}

impl AuditSink for HeldFirstPublish {
    async fn publish(&self, record: &SealedRecord) -> Result<(), SinkError> {
        if !self.held.swap(true, Ordering::SeqCst) {
            self.release.notified().await;
            return Err(SinkError::Unreachable);
        }
        self.inner.publish(record).await
    }
}

#[tokio::test]
async fn concurrent_denies_are_stored_in_the_order_they_were_sealed() {
    let sink = MemorySink::new();
    let release = Arc::new(tokio::sync::Notify::new());
    let held = HeldFirstPublish {
        inner: sink.clone(),
        release: Arc::clone(&release),
        held: Arc::default(),
    };
    let writer = AuditWriter::new(held, writer_id(), config(4, 4));
    let stamper = stamper();

    let (first, second, ()) = tokio::join!(writer.write(deny(&stamper)), writer.write(deny(&stamper)), async {
        tokio::task::yield_now().await;
        release.notify_one();
    });
    assert_eq!(first, Ok(Disposition::Retrying));
    assert_eq!(second, Ok(Disposition::Retrying));

    writer.drain_once().await;

    let sequences: Vec<u64> = sink.stored().iter().map(SealedRecord::sequence).collect();
    assert_eq!(sequences, vec![1, 2]);
}

#[tokio::test]
async fn a_full_retry_buffer_is_audit_degraded_and_refuses_effects() {
    let (writer, sink, stamper) = setup(4, 2);
    sink.fail_all(SinkError::StreamFull);

    assert_eq!(writer.write(deny(&stamper)).await, Ok(Disposition::Retrying));
    assert!(!writer.is_degraded());
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
    assert_eq!(sink.stored().len(), 2);
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
    let stored = sink.stored();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].id(), first_id);
}

#[tokio::test]
async fn a_best_effort_record_the_stream_will_not_take_is_dropped_and_counted_by_reason() {
    let (writer, sink, stamper) = setup(4, 4);
    writer.write(best_effort(&stamper)).await.unwrap();
    writer.write(best_effort(&stamper)).await.unwrap();
    sink.fail_next(1, SinkError::StreamFull);

    writer.drain_once().await;

    assert_eq!(sink.stored().len(), 0);
    let stats = writer.stats();
    assert_eq!(stats.dropped(AuditClass::BestEffort, FailureReason::StreamFull), 1);
    assert_eq!(stats.best_effort_queued(), 1);
    assert!(!writer.is_degraded());

    writer.drain_once().await;

    assert_eq!(sink.stored().len(), 1);
    assert_eq!(writer.stats().best_effort_queued(), 0);
}

#[tokio::test]
async fn a_failing_stream_costs_one_best_effort_publish_per_drain() {
    let (writer, sink, stamper) = setup(4, 4);
    for _ in 0..3 {
        writer.write(best_effort(&stamper)).await.unwrap();
    }
    sink.fail_all(SinkError::Timeout);

    writer.drain_once().await;

    assert_eq!(sink.attempts(), 1);
    let stats = writer.stats();
    assert_eq!(stats.dropped(AuditClass::BestEffort, FailureReason::Timeout), 1);
    assert_eq!(stats.best_effort_queued(), 2);
}

#[tokio::test]
async fn the_writer_is_shared_between_clones() {
    let (writer, sink, stamper) = setup(4, 4);
    let other = writer.clone();
    other.write(allow(&stamper)).await.unwrap();
    writer.write(allow(&stamper)).await.unwrap();
    let sequences: Vec<u64> = sink.stored().iter().map(SealedRecord::sequence).collect();
    assert_eq!(sequences, vec![1, 2]);
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
    assert_eq!(sink.stored().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn the_drain_task_publishes_a_queued_record_as_soon_as_it_is_written() {
    let (writer, sink, stamper) = setup(4, 4);
    let shutdown = CancellationToken::new();
    let task = writer.spawn_drain(shutdown.clone());

    writer.write(best_effort(&stamper)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1)).await;

    assert_eq!(sink.stored().len(), 1);
    shutdown.cancel();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_drain_task_retries_the_buffer_every_interval() {
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

    assert_eq!(sink.stored().len(), 1);
    assert_eq!(writer.stats().retry_buffered(), 0);
    shutdown.cancel();
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_clean_stop_flushes_what_it_can() {
    let (writer, sink, stamper) = setup(4, 4);
    writer.write(best_effort(&stamper)).await.unwrap();
    sink.fail_all(SinkError::Unreachable);
    writer.write(deny(&stamper)).await.unwrap();
    sink.recover();

    let shutdown = CancellationToken::new();
    shutdown.cancel();
    writer.run(shutdown).await;

    assert_eq!(sink.stored().len(), 2);
    assert_eq!(writer.stats().retry_buffered(), 0);
    assert_eq!(writer.stats().best_effort_queued(), 0);
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
async fn a_stalled_drain_is_logged_with_the_reason() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, sink, stamper) = setup(4, 4);
    sink.fail_all(SinkError::Timeout);
    writer.write(deny(&stamper)).await.unwrap();

    writer.drain_once().await;

    let message = "audit retry buffer could not be drained";
    assert_eq!(field_of(&captured, message, "reason").as_deref(), Some("timeout"));
}

#[tokio::test]
async fn a_dropped_best_effort_record_is_logged_with_the_reason() {
    let captured = CapturedEvents::new();
    let _guard = captured.install(LevelFilter::TRACE);
    let (writer, sink, stamper) = setup(4, 4);
    writer.write(best_effort(&stamper)).await.unwrap();
    sink.fail_next(1, SinkError::Unreachable);

    writer.drain_once().await;

    let message = "a best-effort audit record was dropped";
    assert_eq!(field_of(&captured, message, "reason").as_deref(), Some("unreachable"));
}

#[tokio::test]
async fn diagnostics_reach_the_log_facade_when_no_subscriber_is_installed() {
    let Some(logs) = CapturedLogs::isolated() else {
        return;
    };
    let (writer, sink, stamper) = setup(4, 1);
    sink.fail_all(SinkError::Unreachable);
    let refused = allow(&stamper);
    let refused_id = refused.id().to_string();
    writer.write(refused).await.unwrap_err();
    writer.write(deny(&stamper)).await.unwrap();
    writer.write(deny(&stamper)).await.unwrap();
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
    assert!(line("could not be drained").contains("unreachable"));
    assert!(line("best-effort audit record was dropped").contains("unreachable"));
    assert!(records.iter().all(|record| !record.message.contains(marker())));
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn a_write_can_run_on_a_multi_threaded_runtime() {
    let (writer, _sink, stamper) = setup(4, 4);
    let refusal = writer.write(deny(&stamper));
    let effect = writer.write(allow(&stamper));
    assert_send(&refusal);
    assert_send(&effect);
}
