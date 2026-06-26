//! The per-record reconciliation step.
//!
//! Processing one record runs four distinct phases for the same reconciliation
//! unit, then acks: projection (read checkpoint + decide), scheduling (publish
//! or purge the execution schedule), persistence (write the final checkpoint
//! with `last_applied_stream_position`), and ack. `last_applied_stream_position`
//! advances only after both the NATS operation and the final checkpoint write
//! succeed, which is what makes at-least-once redelivery safe.

use std::sync::Arc;
use std::time::Duration;

use async_nats::HeaderMap;
use chrono::{DateTime, Utc};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use trogon_decider_runtime::{CommandError, CommandExecution, StreamAppend, StreamEvent, StreamRead};

use crate::commands::domain::{Delivery, MessageContent, Schedule, ScheduleHeaders, ScheduleId, ScheduleMessage};
use crate::commands::{ScheduleNextOccurrence, ScheduleNextOccurrenceError};
use crate::processor::execution::checkpoints::{
    CheckpointStoreError, LoadedCheckpoint, ProcessingFailureRecord, ReconcileOutcome, ScheduleCheckpointRecord,
    ScheduleCheckpointStore, ScheduleStatus,
};
use crate::processor::execution::execution_schedules::{ExecutionScheduleWriteError, ExecutionScheduleWriter};
use crate::processor::execution::reconciliation::{
    CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE, DecodedScheduleEvent, ReconcileAction, ReconcileError, Reconciliation,
    ScheduleChange, ScheduleEventDecodeError, ScheduleKey, ScheduleRequestError, ScheduleSubject, reconcile,
    schedule_change_from_stream_event, stream_routing_matches_payload,
};
use crate::telemetry::metrics::ProcessorMetrics;
use crate::telemetry::trace::{execution_trace_headers, extract_context};

/// The reconciliation outcome of a single processed record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessedOutcome {
    /// An execution schedule was published (purge-then-publish upsert).
    Published,
    /// The execution subject was purged for a pause/remove.
    Purged,
    /// A paused create stored checkpoint without publishing.
    StoredPaused,
    /// A schedule kind was recorded as unsupported by this processor version.
    Unsupported,
    /// A past-due one-shot `At` reconciled to a terminal expired outcome.
    Expired,
    /// A resume reconciled to an already-expired one-shot `At`: the resume is
    /// a no-op and nothing will ever fire.
    ResumedExpired,
    /// The record was already applied and was a no-op.
    DuplicateStale,
    /// The envelope belonged to another decider's event set.
    SkippedForeign,
    /// The record could not be mapped to a schedule and a durable failure
    /// record was written before terminating it.
    DurableFailure,
}

impl ProcessedOutcome {
    fn metric_label(self) -> &'static str {
        match self {
            Self::Published => "published",
            Self::Purged => "purged",
            Self::StoredPaused => "stored_paused",
            Self::Unsupported => "unsupported",
            Self::Expired => "expired",
            Self::ResumedExpired => "resumed_expired",
            Self::DuplicateStale => "duplicate_stale",
            Self::SkippedForeign => "skipped_foreign",
            Self::DurableFailure => "durable_failure",
        }
    }
}

/// How the NATS message should be acknowledged after a durable outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckAction {
    /// Acknowledge the message normally.
    Ack,
    /// Terminate the message; only used after a durable failure record exists.
    Term,
}

/// A durable processing result for one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Processed {
    /// The reconciliation outcome.
    pub outcome: ProcessedOutcome,
    /// How the message should be acknowledged.
    pub ack: AckAction,
}

enum CheckpointLoadStep {
    Loaded(Option<LoadedCheckpoint>),
    Recovery {
        reconciliation: Reconciliation,
        revision: Option<u64>,
    },
    Poison(ProcessingFailureRecord),
}

enum DecodeStep {
    Change(ScheduleChange),
    SkippedForeign,
    Poison(ProcessingFailureRecord),
}

enum ReconcileStep {
    Reconciled(Box<Reconciliation>),
    RetryMissing { schedule_id: ScheduleId },
    Poison(ProcessingFailureRecord),
}

enum SaveStep {
    Saved,
    Conflict,
    Poison(ProcessingFailureRecord),
}

#[derive(Debug, thiserror::Error)]
pub(super) enum PoisonReason {
    #[error("event could not be decoded: {source}")]
    EventDecode {
        #[source]
        source: ScheduleEventDecodeError,
    },
    #[error("stored checkpoint could not be read: {source}")]
    CheckpointRead {
        #[source]
        source: CheckpointStoreError,
    },
    #[error("schedule could not be converted: {source}")]
    ScheduleRequest {
        #[source]
        source: ScheduleRequestError,
    },
    #[error("checkpoint could not be written: {source}")]
    CheckpointWrite {
        #[source]
        source: CheckpointStoreError,
    },
    #[error("scheduler checkpoint for schedule '{schedule_id}' cannot be resumed")]
    UnrecoverableCheckpoint { schedule_id: ScheduleId },
    #[error("no checkpoint for schedule '{schedule_id}' after {deliveries} deliveries")]
    MissingCheckpointExhausted { schedule_id: ScheduleId, deliveries: i64 },
    #[error("scheduler processor panicked while processing stream position {stream_position}")]
    ProcessorPanic {
        stream_position: trogon_decider_runtime::StreamPosition,
    },
    #[error("scheduler processor panicked while recording a failure")]
    FailureRecordPanic,
}

/// A transient failure: the record reached no durable outcome and must be
/// retried (negative-acknowledged or left to `ack_wait`), never acknowledged.
#[derive(Debug, thiserror::Error)]
pub enum RetrySignal {
    /// A KV checkpoint operation failed transiently.
    #[error("transient checkpoint failure: {source}")]
    Checkpoint {
        #[source]
        source: CheckpointStoreError,
    },
    /// An execution schedule operation failed transiently.
    #[error("transient execution schedule failure: {source}")]
    ExecutionSchedule {
        #[source]
        source: ExecutionScheduleWriteError,
    },
    /// A schedule change arrived before its schedule's checkpoint existed. Per
    /// aggregate ordering plus full-history retention make this a transient
    /// recovery condition (e.g. KV loss awaiting a durable reset replay), not a
    /// poison record.
    #[error("no checkpoint yet for schedule '{schedule_id}', retrying")]
    MissingCheckpoint { schedule_id: ScheduleId },
    /// Arming the next occurrence (the `ScheduleNextOccurrence` command) failed.
    #[error("arming the next occurrence failed: {source}")]
    ArmSchedule {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Reconciles persisted schedule events into execution schedules against
/// concrete NATS KV checkpoints and execution schedule writes.
#[derive(Debug, Clone)]
pub struct ScheduleProcessor<P, U, S, E> {
    execution_schedules: ExecutionScheduleWriter<P, U>,
    checkpoints: ScheduleCheckpointStore<S>,
    event_store: E,
    event_stream_name: String,
    metrics: Arc<ProcessorMetrics>,
}

impl<P, U, S, E> ScheduleProcessor<P, U, S, E>
where
    P: trogon_nats::jetstream::JetStreamPublisher,
    U: trogon_nats::jetstream::JetStreamSubjectPurger,
    S: trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + trogon_nats::jetstream::JetStreamKvKeys,
    E: StreamRead<str> + StreamAppend<str>,
    <E as StreamRead<str>>::Error: std::error::Error + Send + Sync + 'static,
    <E as StreamAppend<str>>::Error: std::error::Error + Send + Sync + 'static,
{
    /// Assembles a processor over its execution schedule writer, checkpoint store,
    /// and the schedule event store used to arm recurrence occurrences.
    pub fn new(
        execution_schedules: ExecutionScheduleWriter<P, U>,
        checkpoints: ScheduleCheckpointStore<S>,
        event_store: E,
        event_stream_name: impl Into<String>,
        metrics: Arc<ProcessorMetrics>,
    ) -> Self {
        Self {
            execution_schedules,
            checkpoints,
            event_store,
            event_stream_name: event_stream_name.into(),
            metrics,
        }
    }

    /// Processes one persisted record, reaching a durable outcome before the
    /// caller acks. `now` is injected so past-due `At` reconciliation is
    /// deterministic in tests.
    pub async fn process(&self, stream_event: &StreamEvent, now: DateTime<Utc>) -> Result<Processed, RetrySignal> {
        self.process_decoded(stream_event, DecodedScheduleEvent::Undecoded, now)
            .await
    }

    /// Like [`Self::process`], but reuses the payload already decoded during
    /// lane routing so each record is protobuf-decoded once.
    pub(crate) async fn process_decoded(
        &self,
        stream_event: &StreamEvent,
        decoded: DecodedScheduleEvent,
        now: DateTime<Utc>,
    ) -> Result<Processed, RetrySignal> {
        let span = tracing::info_span!(
            trogon_semconv::span::SCHEDULER_PROCESS_SCHEDULE_EVENT,
            stream_id = %stream_event.stream_id,
            stream_position = %stream_event.stream_position,
            schedule_key = tracing::field::Empty,
            event_type = tracing::field::Empty,
            outcome = tracing::field::Empty,
        );
        // Link this span to the trace recorded when the command produced the
        // event, instead of starting an orphaned trace.
        let _ = span.set_parent(extract_context(&stream_event.event.headers));

        let result = self
            .process_inner(stream_event, decoded, now)
            .instrument(span.clone())
            .await;
        if let Ok(processed) = &result {
            span.record(trogon_semconv::attribute::OUTCOME, processed.outcome.metric_label());
            self.metrics.record_outcome(processed.outcome.metric_label());
        }
        result
    }

    async fn process_inner(
        &self,
        stream_event: &StreamEvent,
        decoded: DecodedScheduleEvent,
        now: DateTime<Utc>,
    ) -> Result<Processed, RetrySignal> {
        // Phase 1 is synchronous and converts any non-`Send` domain error into a
        // `Send` poison reason; phase 2 awaits the poison write. Non-`Send`
        // errors must not be held across an await so the dispatcher future stays
        // `Send`. `Undecoded` records decode here (the only decode for callers
        // of `process`; a second decode only on the rare poison/panic path) so
        // the failure record captures the decode error.
        let decode_step = match decoded {
            DecodedScheduleEvent::Change(change) => DecodeStep::Change(change),
            DecodedScheduleEvent::Foreign => DecodeStep::SkippedForeign,
            DecodedScheduleEvent::Undecoded => match schedule_change_from_stream_event(stream_event) {
                Ok(Some(change)) => DecodeStep::Change(change),
                Ok(None) => DecodeStep::SkippedForeign,
                Err(source) => {
                    let failure = self.failure_record(stream_event, PoisonReason::EventDecode { source });
                    DecodeStep::Poison(failure)
                }
            },
        };
        let change = match decode_step {
            DecodeStep::Change(change) => change,
            DecodeStep::SkippedForeign => return Ok(self.ack(ProcessedOutcome::SkippedForeign)),
            DecodeStep::Poison(failure) => return self.poison_failure(failure).await,
        };

        if !stream_routing_matches_payload(stream_event, &change) {
            return self
                .poison_failure(self.failure_record(
                    stream_event,
                    PoisonReason::EventDecode {
                        source: ScheduleEventDecodeError::StreamRoutingMismatch {
                            stream_id: stream_event.stream_id.clone(),
                            schedule_id: change.schedule_id().as_str().to_string(),
                        },
                    },
                ))
                .await;
        }

        let schedule_id = change.schedule_id().clone();
        let key = ScheduleKey::derive(&schedule_id);
        tracing::Span::current().record(trogon_semconv::attribute::SCHEDULE_KEY, key.simple());
        let event_id = stream_event.event.id.to_string();
        let position = stream_event.stream_position;

        let load_step = match self.checkpoints.load(&key).await {
            Ok(loaded) => CheckpointLoadStep::Loaded(loaded),
            Err(error) if error.is_transient() => return Err(RetrySignal::Checkpoint { source: error }),
            Err(error) => {
                // A matching event id proves this exact event was already
                // applied, but the stored record is still undecodable: only a
                // Resumed duplicate acks without repair (recovery cannot
                // rebuild a definition from it). Other duplicates fall through
                // to recovery so the corrupt blob is rewritten; side effects
                // are idempotent (purge-then-publish upsert / purge).
                if matches!(change, ScheduleChange::Resumed { .. })
                    && error
                        .corrupt_last_applied_event_id()
                        .is_some_and(|applied| applied == event_id)
                {
                    return Ok(self.ack(ProcessedOutcome::DuplicateStale));
                }
                let corrupt_revision = error.corrupt_revision();
                let recovery = recover_corrupt_checkpoint(&change, position, Some(&event_id), corrupt_revision, now);
                if corrupt_revision.is_some() && !matches!(recovery, Ok(Some(_))) {
                    // The corrupt KV entry stays in the bucket, so every
                    // future event for this schedule will poison the same
                    // way until an operator deletes the entry.
                    tracing::error!(
                        schedule_key = key.simple(),
                        revision = corrupt_revision,
                        "poisoning corrupt checkpoint without repairing it; delete the KV entry to revive the schedule"
                    );
                }
                match recovery {
                    Ok(Some(reconciliation)) => CheckpointLoadStep::Recovery {
                        reconciliation,
                        revision: corrupt_revision,
                    },
                    Ok(None)
                    | Err(ReconcileError::MissingCheckpoint { .. })
                    | Err(ReconcileError::UnrecoverableCheckpoint { .. }) => CheckpointLoadStep::Poison(
                        self.failure_record(stream_event, PoisonReason::CheckpointRead { source: error }),
                    ),
                    Err(ReconcileError::ScheduleRequest { source }) => CheckpointLoadStep::Poison(
                        self.failure_record(stream_event, PoisonReason::ScheduleRequest { source }),
                    ),
                }
            }
        };

        let loaded = match load_step {
            CheckpointLoadStep::Loaded(loaded) => loaded,
            CheckpointLoadStep::Recovery {
                reconciliation,
                revision,
            } => {
                return self
                    .apply_reconciliation(stream_event, &event_id, reconciliation, revision)
                    .await;
            }
            CheckpointLoadStep::Poison(failure) => return self.poison_failure(failure).await,
        };

        let reconcile_step = match reconcile(
            loaded.as_ref().map(|loaded| &loaded.record),
            &change,
            position,
            Some(&event_id),
            now,
        ) {
            Ok(reconciliation) => ReconcileStep::Reconciled(Box::new(reconciliation)),
            Err(ReconcileError::MissingCheckpoint { schedule_id }) => ReconcileStep::RetryMissing { schedule_id },
            Err(ReconcileError::UnrecoverableCheckpoint { schedule_id }) => ReconcileStep::Poison(
                self.failure_record(stream_event, PoisonReason::UnrecoverableCheckpoint { schedule_id }),
            ),
            Err(ReconcileError::ScheduleRequest { source }) => {
                ReconcileStep::Poison(self.failure_record(stream_event, PoisonReason::ScheduleRequest { source }))
            }
        };
        let reconciliation = match reconcile_step {
            ReconcileStep::Reconciled(reconciliation) => *reconciliation,
            ReconcileStep::RetryMissing { schedule_id } => return Err(RetrySignal::MissingCheckpoint { schedule_id }),
            ReconcileStep::Poison(failure) => return self.poison_failure(failure).await,
        };

        let revision = loaded.as_ref().map(|loaded| loaded.revision);
        let resumed = matches!(change, ScheduleChange::Resumed { .. });
        let mut processed = self
            .apply_reconciliation(stream_event, &event_id, reconciliation, revision)
            .await?;
        // A stale Resumed (position <= watermark) on an expired schedule
        // short-circuits to DuplicateStale before this reclassification:
        // it counts as a duplicate, not as ResumedExpired.
        if resumed && processed.outcome == ProcessedOutcome::Expired {
            tracing::warn!(
                schedule_id = %schedule_id,
                "resume reconciled to an already-expired schedule; nothing will fire"
            );
            processed.outcome = ProcessedOutcome::ResumedExpired;
        }
        Ok(processed)
    }

    async fn apply_reconciliation(
        &self,
        stream_event: &StreamEvent,
        event_id: &str,
        reconciliation: Reconciliation,
        revision: Option<u64>,
    ) -> Result<Processed, RetrySignal> {
        // A duplicate or stale record is already applied: ack without touching
        // the execution schedule or the checkpoint record.
        if reconciliation.next_checkpoint.last_outcome == ReconcileOutcome::DuplicateStale {
            return Ok(self.ack(ProcessedOutcome::DuplicateStale));
        }

        let trace_headers = execution_trace_headers(&stream_event.event.headers);
        self.apply_action(&reconciliation.action, event_id, &trace_headers)
            .await?;

        // The save error is not `Send`, so it is converted into a `Send` step
        // before any await (see the `process_inner` phase note).
        let save_step = match self.checkpoints.save(&reconciliation.next_checkpoint, revision).await {
            Ok(_) => SaveStep::Saved,
            Err(CheckpointStoreError::Conflict) => SaveStep::Conflict,
            Err(error) if error.is_transient() => return Err(RetrySignal::Checkpoint { source: error }),
            Err(source) => {
                SaveStep::Poison(self.failure_record(stream_event, PoisonReason::CheckpointWrite { source }))
            }
        };
        match save_step {
            SaveStep::Saved => {}
            SaveStep::Conflict => return self.resolve_save_conflict(&reconciliation.next_checkpoint).await,
            SaveStep::Poison(failure) => return self.poison_failure(failure).await,
        }

        // Counted after the checkpoint save so a transient save failure followed
        // by redelivery does not double-count the (idempotent) publish/purge.
        match &reconciliation.action {
            ReconcileAction::Publish(_) => self.metrics.record_publish(),
            ReconcileAction::Purge(_) => self.metrics.record_purge(),
            ReconcileAction::Dispatch(_) | ReconcileAction::ArmNext { .. } | ReconcileAction::CheckpointOnly => {}
        }

        Ok(self.ack(outcome_from(reconciliation.next_checkpoint.last_outcome)))
    }

    /// Applies one reconciled side effect. Each schedule event maps to a single
    /// action, so the durable event id is a stable, unique `Nats-Msg-Id`.
    async fn apply_action(
        &self,
        action: &ReconcileAction,
        event_id: &str,
        trace_headers: &HeaderMap,
    ) -> Result<(), RetrySignal> {
        match action {
            ReconcileAction::Publish(request) => self
                .execution_schedules
                .upsert(request, event_id, trace_headers)
                .await
                .map_err(|source| RetrySignal::ExecutionSchedule { source }),
            ReconcileAction::Dispatch(request) => self
                .execution_schedules
                .dispatch(request, event_id, trace_headers)
                .await
                .map_err(|source| RetrySignal::ExecutionSchedule { source }),
            ReconcileAction::Purge(subject) => self
                .execution_schedules
                .purge(subject)
                .await
                .map_err(|source| RetrySignal::ExecutionSchedule { source }),
            ReconcileAction::ArmNext { schedule_id, now } => self.arm_next_occurrence(schedule_id, *now).await,
            ReconcileAction::CheckpointOnly => Ok(()),
        }
    }

    async fn arm_next_occurrence(&self, schedule_id: &ScheduleId, now: DateTime<Utc>) -> Result<(), RetrySignal> {
        let command = ScheduleNextOccurrence::new(schedule_id.clone(), now);
        match CommandExecution::new(&self.event_store, &command).execute().await {
            Ok(_) => Ok(()),
            Err(CommandError::Decide(rejection)) => match rejection {
                ScheduleNextOccurrenceError::AlreadyArmed { .. }
                | ScheduleNextOccurrenceError::AlreadyCompleted { .. }
                | ScheduleNextOccurrenceError::SchedulePaused { .. } => Ok(()),
                other => Err(RetrySignal::ArmSchedule {
                    source: Box::new(other),
                }),
            },
            Err(error) => Err(RetrySignal::ArmSchedule {
                source: Box::new(error),
            }),
        }
    }

    /// Another writer saved this schedule's checkpoint after our load. The
    /// winner's write has already landed, so a fresh read is conclusive: if it
    /// already covers this record's position the record is a duplicate and the
    /// side effects this attempt ran were idempotent re-runs; otherwise retry
    /// so the next delivery reconciles against the winner's checkpoint instead
    /// of re-running side effects against a stale revision. An absent entry
    /// (deleted out of band after the conflicting write) is signaled as a
    /// missing checkpoint rather than retried as a conflict, so redelivery
    /// reconciles from scratch instead of looping on a stale revision.
    ///
    /// The duplicate ack is only sound while writers for a key are
    /// serialized: this attempt's purge/publish already ran and could undo
    /// the winner's side effect if a second instance interleaved a different
    /// position. See the single-active-instance invariant documented in
    /// `worker/consumer.rs`.
    async fn resolve_save_conflict(
        &self,
        next_checkpoint: &ScheduleCheckpointRecord,
    ) -> Result<Processed, RetrySignal> {
        match self.checkpoints.load(&next_checkpoint.key()).await {
            Ok(Some(loaded))
                if loaded.record.last_applied_stream_position >= next_checkpoint.last_applied_stream_position =>
            {
                Ok(self.ack(ProcessedOutcome::DuplicateStale))
            }
            Ok(Some(_)) => Err(RetrySignal::Checkpoint {
                source: CheckpointStoreError::Conflict,
            }),
            Ok(None) => Err(RetrySignal::MissingCheckpoint {
                schedule_id: next_checkpoint.schedule_id.clone(),
            }),
            Err(source) => Err(RetrySignal::Checkpoint { source }),
        }
    }

    fn ack(&self, outcome: ProcessedOutcome) -> Processed {
        Processed {
            outcome,
            ack: AckAction::Ack,
        }
    }

    /// Writes a durable processing-failure record, then terminates the message.
    /// If recording the failure itself fails transiently the record is retried
    /// so the failure is durable before any ack.
    pub(super) fn failure_record(&self, stream_event: &StreamEvent, reason: PoisonReason) -> ProcessingFailureRecord {
        ProcessingFailureRecord::new(
            self.event_stream_name.clone(),
            stream_event.stream_position,
            Some(stream_event.event.id.to_string()),
            reason.to_string(),
            stream_event.recorded_at,
        )
    }

    pub(super) async fn poison_failure(&self, failure: ProcessingFailureRecord) -> Result<Processed, RetrySignal> {
        self.checkpoints
            .record_failure(&failure)
            .await
            .map_err(|source| RetrySignal::Checkpoint { source })?;
        Ok(Processed {
            outcome: ProcessedOutcome::DurableFailure,
            ack: AckAction::Term,
        })
    }

    /// Records that a processed record was a NATS redelivery, for backpressure
    /// and poison visibility.
    pub fn record_redelivery(&self) {
        self.metrics.record_redelivery();
    }
}

#[allow(clippy::expect_used)]
fn recover_corrupt_checkpoint(
    change: &ScheduleChange,
    stream_position: trogon_decider_runtime::StreamPosition,
    event_id: Option<&str>,
    revision: Option<u64>,
    now: DateTime<Utc>,
) -> Result<Option<Reconciliation>, ReconcileError> {
    let Some(_revision) = revision else {
        return Ok(None);
    };

    let (schedule_id, status) = match change {
        ScheduleChange::Created { .. } => return reconcile(None, change, stream_position, event_id, now).map(Some),
        ScheduleChange::Paused { schedule_id } => (schedule_id, ScheduleStatus::Paused),
        ScheduleChange::Removed { schedule_id } => (schedule_id, ScheduleStatus::Removed),
        ScheduleChange::Resumed { .. }
        | ScheduleChange::OccurrenceRecorded { .. }
        | ScheduleChange::OccurrenceScheduled { .. }
        | ScheduleChange::Completed { .. } => return Ok(None),
    };

    let key = ScheduleKey::derive(schedule_id);
    let subject = ScheduleSubject::execution(&key);
    Ok(Some(Reconciliation {
        action: ReconcileAction::Purge(subject),
        next_checkpoint: ScheduleCheckpointRecord {
            schedule_id: schedule_id.clone(),
            status,
            schedule: Schedule::every(Duration::from_secs(1)).expect("one second is a valid tombstone schedule"),
            delivery: Delivery::nats_event(CORRUPT_CHECKPOINT_PLACEHOLDER_ROUTE)
                .expect("tombstone delivery subject is valid"),
            message: ScheduleMessage {
                content: MessageContent::json("{}"),
                headers: ScheduleHeaders::default(),
            },
            last_applied_stream_position: stream_position,
            last_applied_event_id: event_id.map(str::to_string),
            last_outcome: ReconcileOutcome::Purged,
        },
    }))
}

fn outcome_from(outcome: ReconcileOutcome) -> ProcessedOutcome {
    match outcome {
        ReconcileOutcome::Published => ProcessedOutcome::Published,
        ReconcileOutcome::Purged => ProcessedOutcome::Purged,
        ReconcileOutcome::StoredPaused => ProcessedOutcome::StoredPaused,
        ReconcileOutcome::Unsupported => ProcessedOutcome::Unsupported,
        ReconcileOutcome::Expired => ProcessedOutcome::Expired,
        // DuplicateStale is short-circuited before this mapping.
        ReconcileOutcome::DuplicateStale => ProcessedOutcome::DuplicateStale,
        // Unknown only exists on records decoded from a newer deployment;
        // reconciliation always writes a concrete outcome to a next checkpoint.
        ReconcileOutcome::Unknown => ProcessedOutcome::DuplicateStale,
    }
}

#[cfg(test)]
mod tests;
