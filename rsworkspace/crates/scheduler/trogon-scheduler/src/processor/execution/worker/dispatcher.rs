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

use super::processor::{
    AckAction, PoisonReasonError, Processed, ProcessedOutcome, RetrySignalError, ScheduleProcessor,
};

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
    // await: a transient `RetrySignalError` can wrap a non-`Send` domain error, so it
    // must not survive across `message.*().await`.
    let step = match AssertUnwindSafe(processor.process_decoded(&event, decoded, now))
        .catch_unwind()
        .await
    {
        Ok(Err(RetrySignalError::MissingCheckpoint { schedule_id }))
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
                PoisonReasonError::MissingCheckpointExhausted {
                    schedule_id,
                    deliveries: message.delivery_count(),
                },
            );
            return poison_record(processor, message, lane, stream_position, failure).await;
        }
        ProcessStep::Panic(stream_position) => {
            let failure = processor.failure_record(&event, PoisonReasonError::ProcessorPanic { stream_position });
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

fn reduce_processed(processed: Result<Processed, RetrySignalError>) -> (Settle, Result<ProcessedOutcome, String>) {
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
        Err(_) => (Settle::Retry, Err(PoisonReasonError::FailureRecordPanic.to_string())),
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
mod tests;
