//! Per-aggregate lane dispatch.
//!
//! When the durable consumer delivers from a multiplexed event stream, records
//! for the same `schedule_id` must be processed one at a time so a pause,
//! resume, or remove cannot race ahead of the definition it depends on. Records
//! for different schedules map to different `ScheduleKey`s and may process
//! concurrently.
//!
//! Lanes are lazily created per `ScheduleKey`, kept strictly serial, evicted
//! once empty so the lane map cannot grow without bound, and the number of
//! concurrently active lanes is bounded so it stays consistent with the
//! consumer's `max_ack_pending`. A record is settled (acked/termed/retried) by
//! its lane worker only after its durable outcome, never at enqueue time.

use std::collections::{HashMap, VecDeque};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinHandle;
use trogon_decider_runtime::{StreamEvent, StreamPosition};
use trogon_nats::jetstream::{
    JetStreamKeyValueUpdate, JetStreamKvCreate, JetStreamKvEntry, JetStreamKvGet, JetStreamKvKeys, JetStreamPublisher,
    JetStreamSubjectPurger,
};

use crate::commands::domain::ScheduleId;
use crate::processor::execution::checkpoints::ProcessingFailureRecord;
use crate::processor::execution::reconciliation::{DecodedScheduleEvent, ScheduleKey, lane_route_from_stream_event};

use super::processor::{AckAction, PoisonReason, Processed, ProcessedOutcome, RetrySignal, ScheduleProcessor};

/// A delivered NATS message the dispatcher settles after a durable outcome.
pub trait DeliveredMessage: Send + 'static {
    /// True when JetStream is redelivering this record (`delivered` > 1).
    fn is_redelivery(&self) -> bool {
        false
    }

    /// How many times JetStream has delivered this record (first delivery is 1).
    fn delivery_count(&self) -> i64 {
        1
    }

    /// Acknowledge the message.
    fn ack(&self) -> impl std::future::Future<Output = Result<(), String>> + Send;
    /// Terminate the message (only after a durable failure record exists).
    fn term(&self) -> impl std::future::Future<Output = Result<(), String>> + Send;
    /// Negative-acknowledge so NATS redelivers the unprocessed record.
    fn retry(&self) -> impl std::future::Future<Output = Result<(), String>> + Send;
}

/// Wall-clock source, injected so past-due `At` reconciliation is testable.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Lane dispatch tuning. `max_active_lanes` bounds concurrency and must stay
/// consistent with the consumer's `max_ack_pending`.
#[derive(Debug, Clone, Copy)]
pub struct DispatcherConfig {
    /// Maximum number of lanes processing concurrently.
    pub max_active_lanes: usize,
    /// Bound on the submission channel.
    pub channel_capacity: usize,
}

impl Default for DispatcherConfig {
    fn default() -> Self {
        Self {
            max_active_lanes: 64,
            channel_capacity: 256,
        }
    }
}

/// The durable outcome reported for one dispatched record.
#[derive(Debug, Clone)]
pub struct DispatchReport {
    /// Position of the record in its schedule stream.
    pub stream_position: StreamPosition,
    /// Aggregate lane the record was processed on.
    pub lane: ScheduleKey,
    /// Outcome, or a description of the transient failure that forced a retry.
    pub result: Result<ProcessedOutcome, String>,
}

/// Handle used to submit delivered records to the running dispatcher.
#[derive(Clone)]
pub struct DispatcherHandle<M> {
    submit: mpsc::Sender<(StreamEvent, M)>,
    active_lanes: Arc<AtomicUsize>,
}

impl<M> DispatcherHandle<M> {
    /// Submits a delivered record. Back-pressures when the channel is full.
    pub async fn submit(&self, event: StreamEvent, message: M) -> Result<(), ()> {
        self.submit.send((event, message)).await.map_err(|_| ())
    }

    /// Number of lanes currently processing a record.
    pub fn active_lanes(&self) -> usize {
        self.active_lanes.load(Ordering::SeqCst)
    }
}

/// Spawns the dispatcher loop. Returns a submission handle, a join handle for
/// the loop, and a receiver of per-record durable outcome reports.
pub fn spawn_dispatcher<P, U, S, E, M>(
    processor: Arc<ScheduleProcessor<P, U, S, E>>,
    clock: Clock,
    config: DispatcherConfig,
) -> (DispatcherHandle<M>, JoinHandle<()>, mpsc::Receiver<DispatchReport>)
where
    P: JetStreamPublisher,
    U: JetStreamSubjectPurger,
    S: JetStreamKvEntry + JetStreamKvGet + JetStreamKvCreate + JetStreamKeyValueUpdate + JetStreamKvKeys,
    E: trogon_decider_runtime::StreamRead<str>
        + trogon_decider_runtime::StreamAppend<str>
        + ::core::marker::Send
        + ::core::marker::Sync
        + 'static,
    <E as trogon_decider_runtime::StreamRead<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    <E as trogon_decider_runtime::StreamAppend<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    M: DeliveredMessage + Sync,
{
    assert!(
        config.max_active_lanes > 0,
        "DispatcherConfig::max_active_lanes must be at least 1"
    );

    let (submit_tx, submit_rx) = mpsc::channel(config.channel_capacity);
    let (reports_tx, reports_rx) = mpsc::channel(config.channel_capacity);
    let active_lanes = Arc::new(AtomicUsize::new(0));

    let handle = DispatcherHandle {
        submit: submit_tx,
        active_lanes: active_lanes.clone(),
    };

    let join = tokio::spawn(run(processor, clock, config, submit_rx, reports_tx, active_lanes));

    (handle, join, reports_rx)
}

async fn run<P, U, S, E, M>(
    processor: Arc<ScheduleProcessor<P, U, S, E>>,
    clock: Clock,
    config: DispatcherConfig,
    mut submit_rx: mpsc::Receiver<(StreamEvent, M)>,
    reports_tx: mpsc::Sender<DispatchReport>,
    active_lanes: Arc<AtomicUsize>,
) where
    P: JetStreamPublisher,
    U: JetStreamSubjectPurger,
    S: JetStreamKvEntry + JetStreamKvGet + JetStreamKvCreate + JetStreamKeyValueUpdate + JetStreamKvKeys,
    E: trogon_decider_runtime::StreamRead<str>
        + trogon_decider_runtime::StreamAppend<str>
        + ::core::marker::Send
        + ::core::marker::Sync
        + 'static,
    <E as trogon_decider_runtime::StreamRead<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    <E as trogon_decider_runtime::StreamAppend<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    M: DeliveredMessage + Sync,
{
    let mut pending: HashMap<ScheduleKey, VecDeque<(StreamEvent, DecodedScheduleEvent, M)>> = HashMap::new();
    let mut in_flight: std::collections::HashSet<ScheduleKey> = std::collections::HashSet::new();
    let mut ready = VecDeque::new();
    let mut queued_ready = std::collections::HashSet::new();
    let mut workers = FuturesUnordered::new();
    let mut submit_open = true;
    let mut pending_reports = VecDeque::new();

    loop {
        flush_pending_reports(&mut pending_reports, &reports_tx);

        // Dispatch as many ready lanes as the concurrency bound allows. A lane
        // is ready when it has queued records and is not already in flight.
        while in_flight.len() < config.max_active_lanes {
            let Some(key) = ready.pop_front() else { break };
            queued_ready.remove(&key);
            // ReadyOutcome::AlreadyInFlight and ReadyOutcome::EmptyQueue are
            // defensive arms that `queued_ready` + the submission flow make
            // unreachable through the public API; `resolve_ready_key` is
            // unit-tested for each. `if let` drops the wildcard branch
            // entirely so the loop has no untestable defensive arm.
            if let ReadyOutcome::Dispatch(event, decoded, message) = resolve_ready_key(key, &in_flight, &mut pending) {
                in_flight.insert(key);
                active_lanes.store(in_flight.len(), Ordering::SeqCst);

                let processor = processor.clone();
                let clock = clock.clone();
                workers.push(async move {
                    let report = process_one(processor, clock, event, decoded, message, key).await;
                    (key, report)
                });
            }
        }

        let idle = !submit_open && pending.values().all(VecDeque::is_empty) && workers.is_empty();

        if idle && pending_reports.is_empty() {
            break;
        }

        tokio::select! {
            // Guarded so a closed submit channel cannot busy-spin the loop
            // (`recv` on a closed channel resolves immediately with `None`).
            received = submit_rx.recv(), if submit_open => {
                match received {
                    Some((event, message)) => {
                        let (key, decoded) = lane_route_from_stream_event(&event);
                        let queue = pending.entry(key).or_default();
                        let was_empty = queue.is_empty();
                        queue.push_back((event, decoded, message));
                        if was_empty && !in_flight.contains(&key) && queued_ready.insert(key) {
                            ready.push_back(key);
                        }
                    }
                    None => submit_open = false,
                }
            }
            Some((key, report)) = workers.next(), if !workers.is_empty() => {
                in_flight.remove(&key);
                active_lanes.store(in_flight.len(), Ordering::SeqCst);
                // Idle eviction: drop the lane once its queue is empty so the
                // lane map cannot grow without bound.
                if pending.get(&key).is_none_or(VecDeque::is_empty) {
                    pending.remove(&key);
                } else if queued_ready.insert(key) {
                    ready.push_back(key);
                }
                match reports_tx.try_send(report) {
                    Ok(()) | Err(TrySendError::Closed(_)) => {}
                    Err(TrySendError::Full(report)) => pending_reports.push_back(report),
                }
            }
            permit = reports_tx.reserve(), if !pending_reports.is_empty() => {
                drain_one_reserved_report(permit, &mut pending_reports);
            }
        }
    }
}

/// Deliveries after which a missing-checkpoint record stops retrying and is
/// poisoned with a durable failure record. With the consumer's `max_deliver: -1`
/// and the 30s nak-delay cap this bounds the retry window to roughly an hour
/// instead of occupying an ack-pending slot forever.
const MISSING_CHECKPOINT_DELIVERY_CEILING: i64 = 120;

async fn process_one<P, U, S, E, M>(
    processor: Arc<ScheduleProcessor<P, U, S, E>>,
    clock: Clock,
    event: StreamEvent,
    decoded: DecodedScheduleEvent,
    message: M,
    lane: ScheduleKey,
) -> DispatchReport
where
    P: JetStreamPublisher,
    U: JetStreamSubjectPurger,
    S: JetStreamKvEntry + JetStreamKvGet + JetStreamKvCreate + JetStreamKeyValueUpdate + JetStreamKvKeys,
    E: trogon_decider_runtime::StreamRead<str>
        + trogon_decider_runtime::StreamAppend<str>
        + ::core::marker::Send
        + ::core::marker::Sync
        + 'static,
    <E as trogon_decider_runtime::StreamRead<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    <E as trogon_decider_runtime::StreamAppend<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    M: DeliveredMessage + Sync,
{
    let stream_position = event.stream_position;
    let now = (clock)();

    if message.is_redelivery() {
        processor.record_redelivery();
    }

    // Reduce the process result to `Send`-only values before any settlement
    // await: a transient `RetrySignal` can wrap a non-`Send` domain error, so it
    // must not survive across `message.*().await`.
    let step = match AssertUnwindSafe(processor.process_decoded(&event, decoded, now))
        .catch_unwind()
        .await
    {
        Ok(Err(RetrySignal::MissingCheckpoint { schedule_id }))
            if message.delivery_count() >= MISSING_CHECKPOINT_DELIVERY_CEILING =>
        {
            ProcessStep::MissingExhausted { schedule_id }
        }
        Ok(processed) => ProcessStep::Settled(reduce_processed(processed)),
        Err(_) => ProcessStep::Panic(stream_position),
    };

    let (settlement, result) = match step {
        ProcessStep::Settled(reduced) => reduced,
        ProcessStep::MissingExhausted { schedule_id } => {
            let failure = processor.failure_record(
                &event,
                PoisonReason::MissingCheckpointExhausted {
                    schedule_id,
                    deliveries: message.delivery_count(),
                },
            );
            return poison_record(processor, message, lane, stream_position, failure).await;
        }
        ProcessStep::Panic(stream_position) => {
            let failure = processor.failure_record(&event, PoisonReason::ProcessorPanic { stream_position });
            return poison_record(processor, message, lane, stream_position, failure).await;
        }
    };

    let result = finalize_report(message, settlement, stream_position, result).await;

    DispatchReport {
        stream_position,
        lane,
        result,
    }
}

enum ProcessStep {
    Settled((Settle, Result<ProcessedOutcome, String>)),
    MissingExhausted { schedule_id: ScheduleId },
    Panic(StreamPosition),
}

fn reduce_processed(processed: Result<Processed, RetrySignal>) -> (Settle, Result<ProcessedOutcome, String>) {
    match processed {
        Ok(processed) => (Settle::from(processed.ack), Ok(processed.outcome)),
        Err(retry) => (Settle::Retry, Err(retry.to_string())),
    }
}

async fn poison_record<P, U, S, E, M>(
    processor: Arc<ScheduleProcessor<P, U, S, E>>,
    message: M,
    lane: ScheduleKey,
    stream_position: StreamPosition,
    failure: ProcessingFailureRecord,
) -> DispatchReport
where
    P: JetStreamPublisher,
    U: JetStreamSubjectPurger,
    S: JetStreamKvEntry + JetStreamKvGet + JetStreamKvCreate + JetStreamKeyValueUpdate + JetStreamKvKeys,
    E: trogon_decider_runtime::StreamRead<str>
        + trogon_decider_runtime::StreamAppend<str>
        + ::core::marker::Send
        + ::core::marker::Sync
        + 'static,
    <E as trogon_decider_runtime::StreamRead<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    <E as trogon_decider_runtime::StreamAppend<str>>::Error: ::std::error::Error + Send + Sync + 'static,
    M: DeliveredMessage + Sync,
{
    let (settlement, result) = match AssertUnwindSafe(processor.poison_failure(failure)).catch_unwind().await {
        Ok(processed) => reduce_processed(processed),
        Err(_) => (Settle::Retry, Err(PoisonReason::FailureRecordPanic.to_string())),
    };

    let result = finalize_report(message, settlement, stream_position, result).await;

    DispatchReport {
        stream_position,
        lane,
        result,
    }
}

async fn finalize_report<M: DeliveredMessage>(
    message: M,
    settlement: Settle,
    stream_position: StreamPosition,
    result: Result<ProcessedOutcome, String>,
) -> Result<ProcessedOutcome, String> {
    if let Err(settlement_error) = settle_message(message, settlement, stream_position).await {
        return match result {
            Ok(_) => Err(settlement_error),
            Err(process_error) => Err(format!("{process_error}; {settlement_error}")),
        };
    }
    result
}

async fn settle_message<M: DeliveredMessage>(
    message: M,
    settlement: Settle,
    stream_position: StreamPosition,
) -> Result<(), String> {
    let settle = async {
        match settlement {
            Settle::Ack => message.ack().await,
            Settle::Term => message.term().await,
            Settle::Retry => message.retry().await,
        }
    };
    match AssertUnwindSafe(settle).catch_unwind().await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(settlement_error)) => {
            tracing::warn!(
                stream_position = %stream_position,
                error = %settlement_error,
                "scheduler message settlement failed"
            );
            Err(settlement_error)
        }
        Err(_) => {
            tracing::warn!(
                stream_position = %stream_position,
                "scheduler message settlement panicked"
            );
            Err("message settlement panicked".to_string())
        }
    }
}

/// Pops one report and sends it via the reserved permit, or clears the queue if
/// the channel has closed. Extracted so the success + closed-channel branches
/// can be exercised deterministically by unit tests instead of relying on the
/// tokio scheduler to drive the live `select!` arm at coverage time.
fn drain_one_reserved_report(
    permit: Result<mpsc::Permit<'_, DispatchReport>, mpsc::error::SendError<()>>,
    pending_reports: &mut VecDeque<DispatchReport>,
) {
    match permit {
        Ok(permit) => {
            if let Some(report) = pending_reports.pop_front() {
                permit.send(report);
            }
        }
        Err(_) => pending_reports.clear(),
    }
}

/// Drains `pending_reports` into `reports_tx` using non-blocking sends.
///
/// Stops when the queue empties, the channel is full (caller will retry via
/// `select!`), or the channel receiver has been dropped.
fn flush_pending_reports(pending_reports: &mut VecDeque<DispatchReport>, reports_tx: &mpsc::Sender<DispatchReport>) {
    while let Some(report) = pending_reports.front().cloned() {
        match reports_tx.try_send(report) {
            Ok(()) => {
                pending_reports.pop_front();
            }
            Err(TrySendError::Full(_)) => break,
            Err(TrySendError::Closed(_)) => {
                pending_reports.clear();
                break;
            }
        }
    }
}

/// The outcome of evaluating one key popped from the ready queue.
enum ReadyOutcome<M> {
    /// The key is already being processed on an active worker; skip it.
    AlreadyInFlight,
    /// The pending queue for the key is empty or absent; evict the entry.
    EmptyQueue,
    /// The key has work ready to dispatch.
    Dispatch(StreamEvent, DecodedScheduleEvent, M),
}

/// Looks up `key` in `in_flight` and `pending`, returning the dispatch decision.
///
/// Removes the pending entry if the queue turns out to be empty, keeping the
/// map from accumulating tombstone entries.
fn resolve_ready_key<M>(
    key: ScheduleKey,
    in_flight: &std::collections::HashSet<ScheduleKey>,
    pending: &mut HashMap<ScheduleKey, VecDeque<(StreamEvent, DecodedScheduleEvent, M)>>,
) -> ReadyOutcome<M> {
    if in_flight.contains(&key) {
        return ReadyOutcome::AlreadyInFlight;
    }
    match pending.get_mut(&key).and_then(VecDeque::pop_front) {
        None => {
            pending.remove(&key);
            ReadyOutcome::EmptyQueue
        }
        Some((event, decoded, message)) => ReadyOutcome::Dispatch(event, decoded, message),
    }
}

enum Settle {
    Ack,
    Term,
    Retry,
}

impl From<AckAction> for Settle {
    fn from(action: AckAction) -> Self {
        match action {
            AckAction::Ack => Self::Ack,
            AckAction::Term => Self::Term,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use buffa::MessageField;
    use trogonai_proto::scheduler::schedules::v1;

    use super::super::testkit::{
        InMemoryExecution, InMemoryKv, MemoryEventStore, malformed_stream_event, recorded_at, stream_event,
    };
    use super::*;
    use crate::commands::domain::MessageContent;
    use crate::commands::domain::{
        Delivery, MessageEnvelope, Schedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleEventStatus,
        ScheduleHeaders, ScheduleMessage,
    };
    use crate::processor::execution::checkpoints::ScheduleCheckpointStore;
    use crate::processor::execution::execution_schedules::ExecutionScheduleWriter;
    use crate::processor::execution::reconciliation::ScheduleSubject;
    use crate::processor::execution::reconciliation::{ScheduleKey, StreamRoutingId};
    use crate::processor::execution::worker::ProcessorMetrics;

    #[derive(Clone)]
    enum Settlement {
        Ack,
        Term,
        Retry,
    }

    #[derive(Clone)]
    struct MockMessage {
        log: Arc<Mutex<Vec<Settlement>>>,
        redelivery: bool,
        deliveries: i64,
    }

    fn mock_message(log: Arc<Mutex<Vec<Settlement>>>) -> MockMessage {
        MockMessage {
            log,
            redelivery: false,
            deliveries: 1,
        }
    }

    impl DeliveredMessage for MockMessage {
        fn is_redelivery(&self) -> bool {
            self.redelivery
        }

        fn delivery_count(&self) -> i64 {
            self.deliveries
        }

        async fn ack(&self) -> Result<(), String> {
            self.log.lock().unwrap().push(Settlement::Ack);
            Ok(())
        }
        async fn term(&self) -> Result<(), String> {
            self.log.lock().unwrap().push(Settlement::Term);
            Ok(())
        }
        async fn retry(&self) -> Result<(), String> {
            self.log.lock().unwrap().push(Settlement::Retry);
            Ok(())
        }
    }

    fn created(id: &str, schedule: Schedule) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: id.to_string(),
                    status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
                    schedule: MessageField::some(
                        v1::Schedule::try_from(&ScheduleEventSchedule::from(&schedule)).unwrap(),
                    ),
                    delivery: MessageField::some(
                        v1::Delivery::try_from(&ScheduleEventDelivery::from(
                            &Delivery::nats_event("agent.run").unwrap(),
                        ))
                        .unwrap(),
                    ),
                    message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&ScheduleMessage {
                        content: MessageContent::json("{}"),
                        headers: ScheduleHeaders::default(),
                    }))),
                }
                .into(),
            ),
        }
    }

    fn paused(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn resumed(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleResumed {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn removed(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    type Processor = ScheduleProcessor<InMemoryExecution, InMemoryExecution, InMemoryKv, MemoryEventStore>;

    fn build() -> (Arc<Processor>, InMemoryKv, InMemoryExecution) {
        let kv = InMemoryKv::new();
        let execution = InMemoryExecution::new();
        let writer = ExecutionScheduleWriter::new(execution.clone(), execution.clone());
        let processor = ScheduleProcessor::new(
            writer,
            ScheduleCheckpointStore::new(kv.clone()),
            MemoryEventStore::default(),
            "SCHEDULER_SCHEDULE_EVENTS",
            Arc::new(ProcessorMetrics::new()),
        );
        (Arc::new(processor), kv, execution)
    }

    fn clock() -> Clock {
        Arc::new(recorded_at)
    }

    fn key_for_stream(id: &str) -> ScheduleKey {
        ScheduleKey::for_stream(&StreamRoutingId::from(id))
    }

    async fn drain(mut reports: mpsc::Receiver<DispatchReport>, expected: usize) -> Vec<DispatchReport> {
        let mut collected = Vec::new();
        while collected.len() < expected {
            match reports.recv().await {
                Some(report) => collected.push(report),
                None => break,
            }
        }
        collected
    }

    #[tokio::test]
    async fn records_for_one_schedule_are_processed_in_submission_order() {
        let (processor, kv, execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        let key = key_for_stream(id);
        let subject = ScheduleSubject::execution(&key);
        let log = Arc::new(Mutex::new(Vec::new()));
        let message = mock_message(log.clone());

        let events = [
            created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap()),
            paused(id),
            resumed(id),
            removed(id),
        ];
        for (offset, event) in events.iter().enumerate() {
            handle
                .submit(stream_event(event, id, offset as u64 + 1), message.clone())
                .await
                .unwrap();
        }
        drop(handle);

        let reports = drain(reports, 4).await;
        join.await.unwrap();

        // Per-aggregate ordering: positions reported in submission order.
        let positions: Vec<u64> = reports.iter().map(|r| r.stream_position.as_u64()).collect();
        assert_eq!(positions, vec![1, 2, 3, 4]);
        let outcomes: Vec<ProcessedOutcome> = reports.iter().map(|r| r.result.clone().unwrap()).collect();
        assert_eq!(
            outcomes,
            vec![
                ProcessedOutcome::Published,
                ProcessedOutcome::Purged,
                ProcessedOutcome::Published,
                ProcessedOutcome::Purged,
            ]
        );

        // Final checkpoint is Removed and no execution schedule remains.
        let stored = kv.contains(&format!("v1.{}", key.simple()));
        assert!(stored, "checkpoint record persists");
        assert_eq!(execution.scheduled_count(subject.as_str()), 0);
        assert_eq!(log.lock().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn distinct_schedules_all_make_progress_and_lanes_are_evicted() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
            processor,
            clock(),
            DispatcherConfig {
                max_active_lanes: 2,
                channel_capacity: 64,
            },
        );

        let log = Arc::new(Mutex::new(Vec::new()));
        let message = mock_message(log);
        let ids = ["a", "b", "c", "d", "e"];
        for (offset, id) in ids.iter().enumerate() {
            let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
            handle
                .submit(stream_event(&event, id, offset as u64 + 1), message.clone())
                .await
                .unwrap();
        }
        drop(handle);

        let reports = drain(reports, ids.len()).await;
        join.await.unwrap();

        assert_eq!(reports.len(), ids.len());
        assert!(
            reports
                .iter()
                .all(|r| matches!(r.result, Ok(ProcessedOutcome::Published)))
        );
    }

    #[tokio::test]
    async fn transient_failure_is_retried_not_acked() {
        let kv = InMemoryKv::new();
        let execution = InMemoryExecution::new();
        // Fail the publish so the record reaches no durable outcome.
        execution.fail_next_publish();
        let writer = ExecutionScheduleWriter::new(execution.clone(), execution.clone());
        let processor = Arc::new(ScheduleProcessor::new(
            writer,
            ScheduleCheckpointStore::new(kv.clone()),
            MemoryEventStore::default(),
            "SCHEDULER_SCHEDULE_EVENTS",
            Arc::new(ProcessorMetrics::new()),
        ));
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        let log = Arc::new(Mutex::new(Vec::new()));
        let message = mock_message(log.clone());
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle.submit(stream_event(&event, id, 1), message).await.unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert!(reports[0].result.is_err());
        assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Retry]));
        // No checkpoint advanced because the execution schedule write failed before persistence.
        assert!(!kv.contains(&format!("v1.{}", key_for_stream(id).simple())));
    }

    #[tokio::test]
    async fn paused_create_does_not_publish() {
        let (processor, kv, execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        let key = key_for_stream(id);
        let subject = ScheduleSubject::execution(&key);
        let log = Arc::new(Mutex::new(Vec::new()));

        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: id.to_string(),
                    status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Paused)),
                    schedule: MessageField::some(
                        v1::Schedule::try_from(&ScheduleEventSchedule::from(
                            &Schedule::every(std::time::Duration::from_secs(30)).unwrap(),
                        ))
                        .unwrap(),
                    ),
                    delivery: MessageField::some(
                        v1::Delivery::try_from(&ScheduleEventDelivery::from(
                            &Delivery::nats_event("agent.run").unwrap(),
                        ))
                        .unwrap(),
                    ),
                    message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&ScheduleMessage {
                        content: MessageContent::json("{}"),
                        headers: ScheduleHeaders::default(),
                    }))),
                }
                .into(),
            ),
        };
        handle
            .submit(stream_event(&event, id, 1), mock_message(log))
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert_eq!(reports[0].result.clone().unwrap(), ProcessedOutcome::StoredPaused);
        assert_eq!(execution.scheduled_count(subject.as_str()), 0);
        assert!(kv.contains(&format!("v1.{}", key.simple())));
    }

    #[tokio::test(start_paused = true)]
    async fn full_report_channel_does_not_block_dispatcher_completion() {
        let (processor, _kv, _execution) = build();
        let (handle, join, _reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
            processor,
            clock(),
            DispatcherConfig {
                max_active_lanes: 2,
                channel_capacity: 1,
            },
        );

        let log = Arc::new(Mutex::new(Vec::new()));
        let message = mock_message(log);
        for (position, id) in ["a", "b"].into_iter().enumerate() {
            let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
            handle
                .submit(stream_event(&event, id, position as u64 + 1), message.clone())
                .await
                .unwrap();
        }
        drop(handle);
        drop(_reports);

        tokio::time::timeout(std::time::Duration::from_secs(1), join)
            .await
            .expect("dispatcher must not wait for report receiver capacity")
            .unwrap();
    }

    #[tokio::test]
    async fn active_lanes_is_zero_when_idle() {
        let (processor, _kv, _execution) = build();
        let (handle, join, _reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        assert_eq!(handle.active_lanes(), 0);
        drop(handle);
        join.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_event_is_terminated() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let log = Arc::new(Mutex::new(Vec::new()));
        handle
            .submit(malformed_stream_event(1), mock_message(log.clone()))
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::DurableFailure);
        assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Term]));
    }

    #[tokio::test]
    async fn redelivered_record_is_processed() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let log = Arc::new(Mutex::new(Vec::new()));
        let id = "orders/created";
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(
                stream_event(&event, id, 1),
                MockMessage {
                    log: log.clone(),
                    redelivery: true,
                    deliveries: 2,
                },
            )
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::Published);
        assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Ack]));
    }

    #[tokio::test]
    async fn missing_checkpoint_is_retried_below_the_delivery_ceiling() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        let log = Arc::new(Mutex::new(Vec::new()));
        handle
            .submit(stream_event(&paused(id), id, 2), mock_message(log.clone()))
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert!(reports[0].result.is_err());
        assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Retry]));
    }

    #[tokio::test]
    async fn missing_checkpoint_is_poisoned_at_the_delivery_ceiling() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        let log = Arc::new(Mutex::new(Vec::new()));
        handle
            .submit(
                stream_event(&paused(id), id, 2),
                MockMessage {
                    log: log.clone(),
                    redelivery: true,
                    deliveries: MISSING_CHECKPOINT_DELIVERY_CEILING,
                },
            )
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::DurableFailure);
        assert!(matches!(log.lock().unwrap().as_slice(), [Settlement::Term]));
    }

    #[derive(Clone)]
    struct PanickingMessage;

    impl DeliveredMessage for PanickingMessage {
        async fn ack(&self) -> Result<(), String> {
            panic!("settlement panic");
        }

        async fn term(&self) -> Result<(), String> {
            panic!("settlement panic");
        }

        async fn retry(&self) -> Result<(), String> {
            panic!("settlement panic");
        }
    }

    #[derive(Clone, Copy)]
    struct FailingMessage;

    impl DeliveredMessage for FailingMessage {
        async fn ack(&self) -> Result<(), String> {
            Err("ack failed".to_string())
        }

        async fn term(&self) -> Result<(), String> {
            Err("term failed".to_string())
        }

        async fn retry(&self) -> Result<(), String> {
            Err("retry failed".to_string())
        }
    }

    #[tokio::test]
    async fn settlement_panic_does_not_abort_dispatcher() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, PanickingMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(stream_event(&event, id, 1), PanickingMessage)
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.expect("dispatcher must survive settlement panics");

        assert!(reports[0].result.is_err());
        assert!(
            reports[0]
                .result
                .as_ref()
                .unwrap_err()
                .contains("message settlement panicked")
        );
    }

    #[derive(Clone, Copy)]
    enum PanicSettlement {
        Term,
        Retry,
    }

    #[derive(Clone, Copy)]
    struct SelectivePanickingMessage {
        panic_on: PanicSettlement,
    }

    impl DeliveredMessage for SelectivePanickingMessage {
        async fn ack(&self) -> Result<(), String> {
            let _ = self.panic_on;
            Ok(())
        }

        async fn term(&self) -> Result<(), String> {
            if matches!(self.panic_on, PanicSettlement::Term) {
                panic!("settlement panic");
            }
            Ok(())
        }

        async fn retry(&self) -> Result<(), String> {
            if matches!(self.panic_on, PanicSettlement::Retry) {
                panic!("settlement panic");
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn selective_panicking_message_inert_paths_are_exercised() {
        let term = SelectivePanickingMessage {
            panic_on: PanicSettlement::Retry,
        };
        DeliveredMessage::term(&term).await.unwrap();
        let retry = SelectivePanickingMessage {
            panic_on: PanicSettlement::Term,
        };
        DeliveredMessage::retry(&retry).await.unwrap();
    }

    #[tokio::test]
    async fn settlement_panic_on_term_does_not_abort_dispatcher() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, SelectivePanickingMessage>(processor, clock(), DispatcherConfig::default());

        handle
            .submit(
                malformed_stream_event(1),
                SelectivePanickingMessage {
                    panic_on: PanicSettlement::Term,
                },
            )
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();
        assert!(reports[0].result.is_err());
    }

    #[tokio::test]
    async fn settlement_panic_on_retry_does_not_abort_dispatcher() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, SelectivePanickingMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        handle
            .submit(
                stream_event(&paused(id), id, 1),
                SelectivePanickingMessage {
                    panic_on: PanicSettlement::Retry,
                },
            )
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();
        assert!(reports[0].result.is_err());
    }

    #[tokio::test]
    async fn processor_panic_is_recovered_and_settled() {
        let (processor, kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        kv.panic_on_next_entry();
        let log = Arc::new(Mutex::new(Vec::new()));
        handle
            .submit(
                stream_event(
                    &created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap()),
                    id,
                    1,
                ),
                mock_message(log),
            )
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert_eq!(reports[0].result.as_ref().unwrap(), &ProcessedOutcome::DurableFailure);
    }

    #[tokio::test]
    async fn poison_failure_panic_falls_back_to_retry() {
        let (processor, kv, _execution) = build();
        kv.panic_on_next_failure_record();

        let failure = processor.failure_record(
            &malformed_stream_event(9),
            PoisonReason::ProcessorPanic {
                stream_position: StreamPosition::try_new(9).unwrap(),
            },
        );
        let report = poison_record(
            processor,
            mock_message(Arc::new(Mutex::new(Vec::new()))),
            key_for_stream("malformed"),
            StreamPosition::try_new(9).unwrap(),
            failure,
        )
        .await;

        assert!(report.result.as_ref().unwrap_err().contains("recording a failure"));
    }

    #[tokio::test]
    async fn finalize_report_merges_process_and_settlement_errors() {
        let error = finalize_report(
            PanickingMessage,
            Settle::Ack,
            StreamPosition::try_new(1).unwrap(),
            Err("process failed".to_string()),
        )
        .await
        .unwrap_err();

        assert!(error.contains("process failed"));
        assert!(error.contains("message settlement panicked"));
    }

    #[tokio::test]
    async fn panicking_message_term_and_retry_also_panic() {
        use std::panic::AssertUnwindSafe;

        let message = PanickingMessage;
        assert!(AssertUnwindSafe(message.term()).catch_unwind().await.is_err());
        assert!(AssertUnwindSafe(message.retry()).catch_unwind().await.is_err());
    }

    #[tokio::test]
    async fn selective_panicking_message_ack_is_inert() {
        let message = SelectivePanickingMessage {
            panic_on: PanicSettlement::Term,
        };
        DeliveredMessage::ack(&message).await.unwrap();
    }

    #[tokio::test]
    async fn settlement_error_is_reported() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, FailingMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        handle
            .submit(
                stream_event(
                    &created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap()),
                    id,
                    1,
                ),
                FailingMessage,
            )
            .await
            .unwrap();
        drop(handle);

        let reports = drain(reports, 1).await;
        join.await.unwrap();

        assert!(reports[0].result.as_ref().unwrap_err().contains("ack failed"));
    }

    #[tokio::test]
    async fn term_settlement_error_is_reported() {
        let error = finalize_report(
            FailingMessage,
            Settle::Term,
            StreamPosition::try_new(1).unwrap(),
            Ok(ProcessedOutcome::DurableFailure),
        )
        .await
        .unwrap_err();

        assert!(error.contains("term failed"));
    }

    #[tokio::test]
    async fn retry_settlement_error_is_reported() {
        let error = finalize_report(
            FailingMessage,
            Settle::Retry,
            StreamPosition::try_new(1).unwrap(),
            Ok(ProcessedOutcome::DurableFailure),
        )
        .await
        .unwrap_err();

        assert!(error.contains("retry failed"));
    }

    #[test]
    #[should_panic(expected = "max_active_lanes must be at least 1")]
    fn zero_max_active_lanes_is_rejected() {
        let (processor, _kv, _execution) = build();
        let _ = spawn_dispatcher::<_, _, _, _, MockMessage>(
            processor,
            clock(),
            DispatcherConfig {
                max_active_lanes: 0,
                channel_capacity: 1,
            },
        );
    }

    #[tokio::test]
    async fn dispatcher_finishes_when_report_receiver_is_dropped() {
        let (processor, _kv, _execution) = build();
        let (handle, join, _reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
            processor,
            clock(),
            DispatcherConfig {
                max_active_lanes: 3,
                channel_capacity: 1,
            },
        );

        let log = Arc::new(Mutex::new(Vec::new()));
        let message = mock_message(log);
        for (index, id) in ["a", "b", "c"].into_iter().enumerate() {
            let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
            handle
                .submit(stream_event(&event, id, index as u64 + 1), message.clone())
                .await
                .unwrap();
        }
        drop(handle);
        drop(_reports);

        tokio::time::timeout(std::time::Duration::from_secs(1), join)
            .await
            .expect("dispatcher must finish when the report receiver is dropped")
            .unwrap();
    }

    #[tokio::test]
    async fn slow_report_consumer_still_receives_all_reports() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
            processor,
            clock(),
            DispatcherConfig {
                max_active_lanes: 3,
                channel_capacity: 1,
            },
        );

        let log = Arc::new(Mutex::new(Vec::new()));
        let message = mock_message(log);
        for (index, id) in ["a", "b", "c"].into_iter().enumerate() {
            let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
            handle
                .submit(stream_event(&event, id, index as u64 + 1), message.clone())
                .await
                .unwrap();
        }
        drop(handle);

        let collected = drain(reports, 3).await;
        join.await.unwrap();

        assert_eq!(collected.len(), 3);
        let positions: Vec<u64> = collected.iter().map(|report| report.stream_position.as_u64()).collect();
        assert_eq!(positions, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn mismatched_stream_ids_for_one_schedule_stay_on_one_lane() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
            processor,
            clock(),
            DispatcherConfig {
                max_active_lanes: 2,
                channel_capacity: 8,
            },
        );

        let schedule = Schedule::every(std::time::Duration::from_secs(30)).unwrap();
        let id = "orders/created";
        let message = mock_message(Arc::new(Mutex::new(Vec::new())));

        let mut first = stream_event(&created(id, schedule.clone()), id, 1);
        first.stream_id = "wrong/create".to_string();
        handle.submit(first, message.clone()).await.unwrap();

        let second = stream_event(&created("other/schedule", schedule), "other/schedule", 2);
        handle.submit(second, message.clone()).await.unwrap();

        let mut third = stream_event(&paused(id), id, 3);
        third.stream_id = "wrong/pause".to_string();
        handle.submit(third, message).await.unwrap();
        drop(handle);

        let mut reports = drain(reports, 3).await;
        join.await.unwrap();
        reports.sort_by_key(|report| report.stream_position.as_u64());

        assert_eq!(
            reports
                .iter()
                .map(|report| report.stream_position.as_u64())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(matches!(reports[0].result, Ok(ProcessedOutcome::DurableFailure)));
        assert!(matches!(reports[1].result, Ok(ProcessedOutcome::Published)));
        assert!(matches!(reports[2].result, Ok(ProcessedOutcome::DurableFailure)));
    }

    #[tokio::test]
    async fn same_lane_resumes_after_the_first_record_finishes() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) = spawn_dispatcher::<_, _, _, _, MockMessage>(
            processor,
            clock(),
            DispatcherConfig {
                max_active_lanes: 1,
                channel_capacity: 8,
            },
        );

        let id = "orders/created";
        let log = Arc::new(Mutex::new(Vec::new()));
        let message = mock_message(log);
        for position in 1..=2 {
            let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
            handle
                .submit(stream_event(&event, id, position), message.clone())
                .await
                .unwrap();
        }
        drop(handle);

        let reports = drain(reports, 2).await;
        join.await.unwrap();
        assert_eq!(
            reports
                .iter()
                .map(|report| report.stream_position.as_u64())
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn dropped_report_receiver_stops_drain_early() {
        let (processor, _kv, _execution) = build();
        let (handle, join, reports) =
            spawn_dispatcher::<_, _, _, _, MockMessage>(processor, clock(), DispatcherConfig::default());

        let id = "orders/created";
        let event = created(id, Schedule::every(std::time::Duration::from_secs(30)).unwrap());
        handle
            .submit(
                stream_event(&event, id, 1),
                mock_message(Arc::new(Mutex::new(Vec::new()))),
            )
            .await
            .unwrap();
        drop(handle);
        drop(reports);

        join.await.unwrap();
    }

    // -------------------------------------------------------------------------
    // Deterministic coverage for four concurrency-edge-case branches
    // -------------------------------------------------------------------------

    /// Branch 1 — `flush_pending_reports`: `TrySendError::Closed` arm (lines
    /// 184-186 in the original, now inside `flush_pending_reports`).
    ///
    /// Directly exercises the extracted helper with a pre-populated queue and a
    /// closed channel, confirming the queue is cleared and the function returns.
    #[test]
    fn flush_pending_reports_clears_queue_when_channel_is_closed() {
        use trogon_decider_runtime::StreamPosition;

        let (tx, rx) = mpsc::channel::<DispatchReport>(4);
        drop(rx);

        let report = DispatchReport {
            stream_position: StreamPosition::try_new(1).unwrap(),
            lane: key_for_stream("flush-closed"),
            result: Ok(ProcessedOutcome::Published),
        };

        let mut pending_reports = std::collections::VecDeque::new();
        pending_reports.push_back(report);

        super::flush_pending_reports(&mut pending_reports, &tx);

        assert!(
            pending_reports.is_empty(),
            "flush_pending_reports must clear the queue when the channel receiver is dropped"
        );
    }

    /// `flush_pending_reports`: `Ok(())` success arm drains every report into
    /// the channel and leaves the queue empty.
    #[test]
    fn flush_pending_reports_drains_every_report_when_channel_has_capacity() {
        use trogon_decider_runtime::StreamPosition;

        let (tx, mut rx) = mpsc::channel::<DispatchReport>(8);
        let lane = key_for_stream("flush-ok");
        let mut pending_reports = std::collections::VecDeque::new();
        for stream_position in 1..=4 {
            pending_reports.push_back(DispatchReport {
                stream_position: StreamPosition::try_new(stream_position).unwrap(),
                lane,
                result: Ok(ProcessedOutcome::Published),
            });
        }

        super::flush_pending_reports(&mut pending_reports, &tx);

        assert!(pending_reports.is_empty(), "every report must be drained on success");
        let mut drained = 0;
        while rx.try_recv().is_ok() {
            drained += 1;
        }
        assert_eq!(drained, 4);
    }

    /// `drain_one_reserved_report`: success arm pops one report and sends it.
    #[tokio::test]
    async fn drain_one_reserved_report_sends_front_report_through_permit() {
        use trogon_decider_runtime::StreamPosition;

        let (tx, mut rx) = mpsc::channel::<DispatchReport>(4);
        let permit = tx.reserve().await.expect("permit must reserve");

        let lane = key_for_stream("drain-ok");
        let mut pending_reports = std::collections::VecDeque::new();
        pending_reports.push_back(DispatchReport {
            stream_position: StreamPosition::try_new(1).unwrap(),
            lane,
            result: Ok(ProcessedOutcome::Published),
        });

        super::drain_one_reserved_report(Ok(permit), &mut pending_reports);

        assert!(pending_reports.is_empty(), "front report must be consumed");
        assert!(rx.try_recv().is_ok(), "channel must have received the report");
    }

    /// `drain_one_reserved_report`: closed-channel arm clears the queue so the
    /// loop doesn't keep replaying reports against a receiver that's gone.
    #[test]
    fn drain_one_reserved_report_clears_queue_when_channel_closed() {
        use trogon_decider_runtime::StreamPosition;

        let lane = key_for_stream("drain-closed");
        let mut pending_reports = std::collections::VecDeque::new();
        pending_reports.push_back(DispatchReport {
            stream_position: StreamPosition::try_new(1).unwrap(),
            lane,
            result: Ok(ProcessedOutcome::Published),
        });

        super::drain_one_reserved_report(Err(mpsc::error::SendError(())), &mut pending_reports);

        assert!(pending_reports.is_empty(), "queue must be cleared on channel close");
    }

    /// `flush_pending_reports`: `TrySendError::Full` arm leaves the report
    /// queued for the next select tick instead of dropping or clearing it.
    #[test]
    fn flush_pending_reports_stops_when_channel_is_full_without_clearing_queue() {
        use trogon_decider_runtime::StreamPosition;

        let (tx, _rx) = mpsc::channel::<DispatchReport>(1);
        let lane = key_for_stream("flush-full");
        let blocker = DispatchReport {
            stream_position: StreamPosition::try_new(1).unwrap(),
            lane,
            result: Ok(ProcessedOutcome::Published),
        };
        // Saturate the channel before the helper runs so the next try_send
        // returns Full immediately.
        tx.try_send(blocker).expect("seeded send must fit");

        let mut pending_reports = std::collections::VecDeque::new();
        pending_reports.push_back(DispatchReport {
            stream_position: StreamPosition::try_new(2).unwrap(),
            lane,
            result: Ok(ProcessedOutcome::Published),
        });

        super::flush_pending_reports(&mut pending_reports, &tx);

        assert_eq!(
            pending_reports.len(),
            1,
            "Full must stop the drain without consuming the still-pending report"
        );
    }

    /// Branch 2 — `resolve_ready_key`: `AlreadyInFlight` arm (lines 196-198).
    ///
    /// Constructs the dispatcher's internal state directly (key present in both
    /// `ready` and `in_flight`) and calls the extracted helper, which would be
    /// unreachable via the public submission API because `queued_ready` prevents
    /// duplicates.
    #[test]
    fn resolve_ready_key_returns_already_in_flight_when_key_is_in_flight() {
        use std::collections::{HashMap, HashSet, VecDeque};
        use trogon_decider_runtime::StreamEvent;

        use super::{ReadyOutcome, resolve_ready_key};
        use crate::processor::execution::reconciliation::DecodedScheduleEvent;

        let key = key_for_stream("in-flight-key");

        let mut in_flight = HashSet::new();
        in_flight.insert(key);

        let event = super::super::testkit::malformed_stream_event(1);
        let mut pending: HashMap<ScheduleKey, VecDeque<(StreamEvent, DecodedScheduleEvent, MockMessage)>> =
            HashMap::new();
        pending.entry(key).or_default().push_back((
            event,
            DecodedScheduleEvent::Undecoded,
            mock_message(Arc::new(Mutex::new(Vec::new()))),
        ));

        let outcome = resolve_ready_key(key, &in_flight, &mut pending);

        assert!(
            matches!(outcome, ReadyOutcome::AlreadyInFlight),
            "a key already in flight must be skipped without touching pending"
        );
        assert!(
            pending.contains_key(&key),
            "pending entry must be untouched when key is in flight"
        );
    }

    /// Branch 3 — `resolve_ready_key`: `EmptyQueue` arm (lines 200-203).
    ///
    /// Constructs state where `ready` contains a key whose `pending` queue is
    /// absent, exercising the defensive eviction branch that is an invariant
    /// guard against internal state corruption.
    #[test]
    fn resolve_ready_key_returns_empty_queue_when_pending_entry_is_absent() {
        use std::collections::{HashMap, HashSet, VecDeque};
        use trogon_decider_runtime::StreamEvent;

        use super::{ReadyOutcome, resolve_ready_key};
        use crate::processor::execution::reconciliation::DecodedScheduleEvent;

        let key = key_for_stream("no-pending-key");

        let in_flight: HashSet<ScheduleKey> = HashSet::new();
        let mut pending: HashMap<ScheduleKey, VecDeque<(StreamEvent, DecodedScheduleEvent, MockMessage)>> =
            HashMap::new();

        let outcome = resolve_ready_key(key, &in_flight, &mut pending);

        assert!(
            matches!(outcome, ReadyOutcome::EmptyQueue),
            "a key with no pending entries must be evicted"
        );
        assert!(
            !pending.contains_key(&key),
            "evicted key must not be present in pending after eviction"
        );
    }

    /// Branch 4 — `drain`: `None` arm (line 607).
    ///
    /// Calls the test-local `drain` helper with a channel whose sender has
    /// already been dropped, so `recv()` immediately returns `None` and the
    /// helper stops before collecting `expected` items.
    #[tokio::test]
    async fn drain_stops_early_when_channel_closes_before_expected_count() {
        let (tx, rx) = mpsc::channel::<DispatchReport>(4);
        drop(tx);

        let collected = drain(rx, usize::MAX).await;

        assert!(
            collected.is_empty(),
            "drain must return immediately when the sender is dropped before any items are sent"
        );
    }
}
