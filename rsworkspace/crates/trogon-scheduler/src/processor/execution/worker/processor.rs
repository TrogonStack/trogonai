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

use chrono::{DateTime, Utc};
use tracing::Instrument;
use trogon_decider_runtime::StreamEvent;

use crate::commands::domain::{Delivery, MessageContent, Schedule, ScheduleHeaders, ScheduleId, ScheduleMessage};
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
    /// An enabled RRULE was recorded as unsupported by this processor version.
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
}

/// Reconciles persisted schedule events into execution schedules against
/// concrete NATS KV checkpoints and execution schedule writes.
#[derive(Debug, Clone)]
pub struct ScheduleProcessor<P, U, S> {
    execution_schedules: ExecutionScheduleWriter<P, U>,
    checkpoints: ScheduleCheckpointStore<S>,
    event_stream_name: String,
    metrics: Arc<ProcessorMetrics>,
}

impl<P, U, S> ScheduleProcessor<P, U, S>
where
    P: trogon_nats::jetstream::JetStreamPublisher,
    U: trogon_nats::jetstream::JetStreamSubjectPurger,
    S: trogon_nats::jetstream::JetStreamKvEntry
        + trogon_nats::jetstream::JetStreamKvGet
        + trogon_nats::jetstream::JetStreamKvCreate
        + trogon_nats::jetstream::JetStreamKeyValueUpdate
        + trogon_nats::jetstream::JetStreamKvKeys,
{
    /// Assembles a processor over its execution schedule writer and checkpoint store.
    pub fn new(
        execution_schedules: ExecutionScheduleWriter<P, U>,
        checkpoints: ScheduleCheckpointStore<S>,
        event_stream_name: impl Into<String>,
        metrics: Arc<ProcessorMetrics>,
    ) -> Self {
        Self {
            execution_schedules,
            checkpoints,
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
            "scheduler.process_schedule_event",
            stream_id = %stream_event.stream_id,
            stream_position = %stream_event.stream_position,
            schedule_key = tracing::field::Empty,
            event_type = tracing::field::Empty,
            outcome = tracing::field::Empty,
        );
        // Link this span to the trace recorded when the command produced the
        // event, instead of starting an orphaned trace.
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let _ = span.set_parent(extract_context(&stream_event.event.headers));

        let result = self
            .process_inner(stream_event, decoded, now)
            .instrument(span.clone())
            .await;
        if let Ok(processed) = &result {
            span.record("outcome", processed.outcome.metric_label());
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
        tracing::Span::current().record("schedule_key", key.simple());
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

        match &reconciliation.action {
            ReconcileAction::Publish(request) => {
                let trace_headers = execution_trace_headers(&stream_event.event.headers);
                self.execution_schedules
                    .upsert(request, event_id, &trace_headers)
                    .await
                    .map_err(|source| RetrySignal::ExecutionSchedule { source })?;
            }
            ReconcileAction::Purge(subject) => {
                self.execution_schedules
                    .purge(subject)
                    .await
                    .map_err(|source| RetrySignal::ExecutionSchedule { source })?;
            }
            ReconcileAction::CheckpointOnly => {}
        }

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
            ReconcileAction::CheckpointOnly => {}
        }

        Ok(self.ack(outcome_from(reconciliation.next_checkpoint.last_outcome)))
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
        ScheduleChange::Resumed { .. } => return Ok(None),
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
mod tests {
    use std::time::Duration;

    use buffa::MessageField;
    use chrono::{DateTime, Utc};
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use trogon_decider_runtime::{HeaderName, Headers, StreamPosition};
    use trogonai_proto::scheduler::schedules::v1;

    use super::super::testkit::{
        InMemoryExecution, InMemoryKv, foreign_stream_event, malformed_stream_event, recorded_at, schedule_id,
        stream_event, stream_event_with_headers,
    };
    use super::*;
    use crate::commands::domain::{
        Delivery, MessageContent, MessageEnvelope, Schedule, ScheduleEventDelivery, ScheduleEventSchedule,
        ScheduleEventStatus, ScheduleHeaders, ScheduleMessage,
    };
    use crate::processor::execution::checkpoints::{ReconcileOutcome, ScheduleCheckpointStore, ScheduleStatus};
    use crate::processor::execution::execution_schedules::ExecutionScheduleWriter;
    use crate::processor::execution::reconciliation::{ScheduleKey, ScheduleSubject, StreamRoutingId};

    type Processor = ScheduleProcessor<InMemoryExecution, InMemoryExecution, InMemoryKv>;

    struct Harness {
        processor: Processor,
        kv: InMemoryKv,
        execution: InMemoryExecution,
    }

    impl Harness {
        fn new() -> Self {
            let kv = InMemoryKv::new();
            let execution = InMemoryExecution::new();
            let writer = ExecutionScheduleWriter::new(execution.clone(), execution.clone());
            let processor = ScheduleProcessor::new(
                writer,
                ScheduleCheckpointStore::new(kv.clone()),
                "SCHEDULER_SCHEDULE_EVENTS",
                Arc::new(ProcessorMetrics::new()),
            );
            Self {
                processor,
                kv,
                execution,
            }
        }

        fn subject(&self, id: &str) -> ScheduleSubject {
            ScheduleSubject::execution(&key_for_stream(id))
        }

        fn checkpoint_key(&self, id: &str) -> String {
            format!("v1.{}", key_for_stream(id).simple())
        }

        async fn process(&self, event: &v1::ScheduleEvent, id: &str, position: u64) -> Processed {
            self.processor
                .process(&stream_event(event, id, position), recorded_at())
                .await
                .expect("durable outcome")
        }
    }

    fn now() -> DateTime<Utc> {
        recorded_at()
    }

    fn message() -> ScheduleMessage {
        ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::default(),
        }
    }

    fn created(id: &str, status: ScheduleEventStatus, schedule: Schedule) -> v1::ScheduleEvent {
        created_with_delivery(id, status, schedule, Delivery::nats_event("agent.run").unwrap())
    }

    fn created_with_delivery(
        id: &str,
        status: ScheduleEventStatus,
        schedule: Schedule,
        delivery: Delivery,
    ) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: id.to_string(),
                    status: MessageField::some(v1::ScheduleStatus::from(status)),
                    schedule: MessageField::some(
                        v1::Schedule::try_from(&ScheduleEventSchedule::from(&schedule)).unwrap(),
                    ),
                    delivery: MessageField::some(
                        v1::Delivery::try_from(&ScheduleEventDelivery::from(&delivery)).unwrap(),
                    ),
                    message: MessageField::some(v1::Message::from(&MessageEnvelope::from(&message()))),
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

    fn every() -> Schedule {
        Schedule::every(Duration::from_secs(30)).unwrap()
    }

    fn key_for_stream(id: &str) -> ScheduleKey {
        ScheduleKey::for_stream(&StreamRoutingId::from(id))
    }

    #[tokio::test]
    async fn enabled_create_publishes_and_persists() {
        let harness = Harness::new();
        let id = "orders/created";

        let processed = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::Published);
        assert_eq!(processed.ack, AckAction::Ack);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
        assert!(harness.kv.contains(&harness.checkpoint_key(id)));
    }

    #[tokio::test]
    async fn enabled_create_matches_the_e2e_io_contract_with_mocks() {
        let harness = Harness::new();
        let id = "orders/created";
        let key = key_for_stream(id);
        let subject = harness.subject(id);
        let event = created(id, ScheduleEventStatus::Scheduled, every());
        let stream_event = stream_event(&event, id, 1);
        let event_id = stream_event.event.id.to_string();

        let processed = harness.processor.process(&stream_event, now()).await.unwrap();

        assert_eq!(processed.outcome, ProcessedOutcome::Published);
        assert_eq!(processed.ack, AckAction::Ack);
        assert_eq!(harness.execution.scheduled_count(subject.as_str()), 1);

        let headers = harness.execution.headers_for(subject.as_str()).unwrap();
        assert_eq!(headers.get("Nats-Schedule").unwrap().as_str(), "@every 30s");
        assert_eq!(headers.get("Nats-Schedule-Target").unwrap().as_str(), "agent.run");
        assert_eq!(headers.get("Nats-Msg-Id").unwrap().as_str(), event_id.as_str());
        assert_eq!(headers.get("Content-Type").unwrap().as_str(), "application/json");
        assert_eq!(headers.get("Trogon-Schedule-Key").unwrap().as_str(), key.simple());
        assert_eq!(
            headers.get("Trogon-Schedule-Id-B64").unwrap().as_str(),
            "b3JkZXJzL2NyZWF0ZWQ"
        );
        assert_eq!(
            harness.execution.payload_for(subject.as_str()).unwrap().as_ref(),
            br#"{"ok":true}"#
        );

        let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
            .load(&key)
            .await
            .unwrap()
            .unwrap()
            .record;
        assert_eq!(checkpoint.schedule_id.as_str(), id);
        assert_eq!(checkpoint.key(), key);
        assert_eq!(checkpoint.subject(), subject);
        assert_eq!(checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 1);
        assert_eq!(checkpoint.last_applied_event_id.as_deref(), Some(event_id.as_str()));
        assert_eq!(checkpoint.last_outcome, ReconcileOutcome::Published);
        assert_eq!(checkpoint.message.content.as_slice(), br#"{"ok":true}"#);
        assert_eq!(checkpoint.message.content.content_type().as_str(), "application/json");
    }

    #[tokio::test]
    async fn trace_context_is_present_on_the_execution_publish() {
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        let harness = Harness::new();
        let id = "orders/created";
        let traceparent = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let headers = Headers::one(HeaderName::new("traceparent").unwrap(), traceparent).unwrap();
        let event = created(id, ScheduleEventStatus::Scheduled, every());

        self_process_with_headers(&harness, &event, id, 1, headers).await;

        let published = harness.execution.headers_for(harness.subject(id).as_str()).unwrap();
        assert!(
            published
                .get("traceparent")
                .unwrap()
                .as_str()
                .contains("4bf92f3577b34da6a3ce929d0e0e4736"),
            "execution publish carries the originating trace"
        );
    }

    async fn self_process_with_headers(
        harness: &Harness,
        event: &v1::ScheduleEvent,
        id: &str,
        position: u64,
        headers: Headers,
    ) -> Processed {
        harness
            .processor
            .process(&stream_event_with_headers(event, id, position, headers), now())
            .await
            .expect("durable outcome")
    }

    #[tokio::test]
    async fn past_due_at_expires_without_publishing() {
        let harness = Harness::new();
        let id = "orders/once";
        let past = Schedule::At {
            at: DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };

        let processed = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, past), id, 1)
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::Expired);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    }

    #[tokio::test]
    async fn resuming_a_past_due_at_reports_a_distinct_resumed_expired_outcome() {
        let harness = Harness::new();
        let id = "orders/once";
        let past = Schedule::At {
            at: DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        };

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, past), id, 1)
            .await;
        harness.process(&paused(id), id, 2).await;
        let processed = harness.process(&resumed(id), id, 3).await;

        assert_eq!(processed.outcome, ProcessedOutcome::ResumedExpired);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    }

    #[tokio::test]
    async fn enabled_rrule_is_unsupported_without_publishing() {
        let harness = Harness::new();
        let id = "recurring";
        let rrule = Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap();

        let processed = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, rrule), id, 1)
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::Unsupported);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    }

    #[tokio::test]
    async fn paused_create_stores_without_publishing() {
        let harness = Harness::new();
        let id = "orders/created";

        let processed = harness
            .process(&created(id, ScheduleEventStatus::Paused, every()), id, 1)
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::StoredPaused);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
        assert!(harness.kv.contains(&harness.checkpoint_key(id)));
    }

    #[tokio::test]
    async fn pause_then_resume_purges_then_republishes() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

        let paused = harness.process(&paused(id), id, 2).await;
        assert_eq!(paused.outcome, ProcessedOutcome::Purged);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

        let resumed = harness.process(&resumed(id), id, 3).await;
        assert_eq!(resumed.outcome, ProcessedOutcome::Published);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    }

    #[tokio::test]
    async fn remove_purges_and_marks_removed() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        let removed = harness.process(&removed(id), id, 2).await;

        assert_eq!(removed.outcome, ProcessedOutcome::Purged);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    }

    #[tokio::test]
    async fn duplicate_redelivery_after_ack_is_a_no_op() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

        // Redeliver the exact same record (same stream position).
        let again = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(again.outcome, ProcessedOutcome::DuplicateStale);
        // No second schedule was published.
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    }

    #[tokio::test]
    async fn stale_record_below_watermark_is_a_no_op() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 5)
            .await;
        // A record below the stored watermark (position 3 < 5).
        let stale = harness.process(&paused(id), id, 3).await;
        assert_eq!(stale.outcome, ProcessedOutcome::DuplicateStale);
        // The schedule was not purged by the stale pause.
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    }

    #[tokio::test]
    async fn crash_after_execution_schedule_write_before_checkpoint_write_retries_and_converges() {
        let harness = Harness::new();
        let id = "orders/created";
        // Execution schedule write (purge+publish) succeeds, but the checkpoint create fails transiently.
        harness.kv.fail_next_create();

        let error = harness
            .processor
            .process(
                &stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1),
                now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, RetrySignal::Checkpoint { .. }));
        assert!(!harness.kv.contains(&harness.checkpoint_key(id)));

        // Redelivery: purge-then-publish converges to exactly one schedule and
        // the checkpoint write now succeeds.
        let retried = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(retried.outcome, ProcessedOutcome::Published);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
        assert!(harness.kv.contains(&harness.checkpoint_key(id)));
    }

    #[tokio::test]
    async fn transient_execution_schedule_failure_does_not_advance_checkpoint() {
        let harness = Harness::new();
        let id = "orders/created";
        harness.execution.fail_next_publish();

        let error = harness
            .processor
            .process(
                &stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1),
                now(),
            )
            .await
            .unwrap_err();

        assert!(matches!(error, RetrySignal::ExecutionSchedule { .. }));
        assert!(!harness.kv.contains(&harness.checkpoint_key(id)));
    }

    #[tokio::test]
    async fn malformed_record_is_durably_recorded_before_term() {
        let harness = Harness::new();

        let processed = harness
            .processor
            .process(&malformed_stream_event(9), now())
            .await
            .expect("durable outcome");

        assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
        assert_eq!(processed.ack, AckAction::Term);
        assert!(harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.9"));
    }

    #[tokio::test]
    async fn foreign_event_is_skipped_and_acked() {
        let harness = Harness::new();

        let processed = harness
            .processor
            .process(&foreign_stream_event(4), now())
            .await
            .expect("durable outcome");

        assert_eq!(processed.outcome, ProcessedOutcome::SkippedForeign);
        assert_eq!(processed.ack, AckAction::Ack);
    }

    #[tokio::test]
    async fn mismatched_stream_routing_is_durably_recorded_before_term() {
        let harness = Harness::new();
        let id = "orders/created";
        let mut event = stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1);
        event.stream_id = "wrong-stream".to_string();

        let processed = harness.processor.process(&event, now()).await.expect("durable outcome");

        assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
        assert_eq!(processed.ack, AckAction::Term);
        assert!(harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.1"));
    }

    #[tokio::test]
    async fn schedule_change_without_checkpoint_is_a_transient_retry_not_a_poison() {
        let harness = Harness::new();
        let id = "orders/created";

        // A pause arrives before any checkpoint exists (e.g. KV loss before replay).
        let error = harness
            .processor
            .process(&stream_event(&paused(id), id, 2), now())
            .await
            .unwrap_err();

        assert!(matches!(error, RetrySignal::MissingCheckpoint { .. }));
        assert_eq!(
            error.to_string(),
            "no checkpoint yet for schedule 'orders/created', retrying"
        );
        // It is not written as a poison record.
        assert!(!harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.2"));
        let _ = schedule_id(id);
    }

    #[tokio::test]
    async fn stale_corrupt_checkpoint_create_redelivery_repairs_the_record() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
        harness.kv.corrupt_definition(&harness.checkpoint_key(id));

        let again = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(again.outcome, ProcessedOutcome::Published);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

        let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
            .load(&key_for_stream(id))
            .await
            .expect("repaired checkpoint decodes")
            .unwrap()
            .record;
        assert_eq!(checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 1);
    }

    #[tokio::test]
    async fn truncated_corrupt_checkpoint_redelivery_repairs_then_resume_succeeds() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
        // Truncate the blob inside the trailing nested snapshot; the envelope
        // fields parsed before the truncation point identify the duplicate.
        harness.kv.truncate_tail(&harness.checkpoint_key(id), 5);

        let again = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(again.outcome, ProcessedOutcome::Published);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

        // The rewritten record is decodable again, so a later pause/resume
        // cycle reconciles instead of hitting the corrupt path.
        let paused = harness.process(&paused(id), id, 2).await;
        assert_eq!(paused.outcome, ProcessedOutcome::Purged);
        let resumed = harness.process(&resumed(id), id, 3).await;
        assert_eq!(resumed.outcome, ProcessedOutcome::Published);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    }

    #[tokio::test]
    async fn corrupt_resumed_duplicate_acks_without_repair() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        harness.process(&paused(id), id, 2).await;
        harness.process(&resumed(id), id, 3).await;
        harness.kv.truncate_tail(&harness.checkpoint_key(id), 5);

        let again = harness.process(&resumed(id), id, 3).await;

        assert_eq!(again.outcome, ProcessedOutcome::DuplicateStale);
        assert_eq!(again.ack, AckAction::Ack);
    }

    #[tokio::test]
    async fn inflated_corrupt_watermark_does_not_skip_newer_events() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
        harness.kv.corrupt_definition(&harness.checkpoint_key(id));
        harness.kv.inflate_watermark(&harness.checkpoint_key(id), 99);

        let processed = harness.process(&paused(id), id, 2).await;
        assert_eq!(processed.outcome, ProcessedOutcome::Purged);
        assert_eq!(processed.ack, AckAction::Ack);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    }

    #[tokio::test]
    async fn corrupt_cached_checkpoint_control_event_purges_and_overwrites_tombstone() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
        // Corrupt the rebuildable cache entry so it can no longer be decoded.
        harness.kv.corrupt(&harness.checkpoint_key(id));

        let processed = harness.process(&paused(id), id, 2).await;
        assert_eq!(processed.outcome, ProcessedOutcome::Purged);
        assert_eq!(processed.ack, AckAction::Ack);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);

        let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
            .load(&key_for_stream(id))
            .await
            .unwrap()
            .unwrap()
            .record;
        assert_eq!(checkpoint.status, ScheduleStatus::Paused);
        assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 2);

        let processed = harness
            .processor
            .process(&stream_event(&resumed(id), id, 3), now())
            .await
            .expect("durable outcome");
        assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
        assert_eq!(processed.ack, AckAction::Term);
    }

    #[tokio::test]
    async fn corrupt_cached_checkpoint_create_event_rebuilds_checkpoint() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        harness.kv.corrupt(&harness.checkpoint_key(id));

        let processed = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 2)
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::Published);
        assert_eq!(processed.ack, AckAction::Ack);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

        let checkpoint = ScheduleCheckpointStore::new(harness.kv.clone())
            .load(&key_for_stream(id))
            .await
            .unwrap()
            .unwrap()
            .record;
        assert_eq!(checkpoint.status, ScheduleStatus::Scheduled);
        assert_eq!(checkpoint.last_applied_stream_position.as_u64(), 2);
    }

    #[tokio::test]
    async fn transient_load_failure_is_retried() {
        let harness = Harness::new();
        let id = "orders/created";
        harness.kv.fail_next_entry();

        let error = harness
            .processor
            .process(
                &stream_event(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1),
                now(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, RetrySignal::Checkpoint { .. }));
        assert!(error.to_string().starts_with("transient checkpoint failure:"));
    }

    #[tokio::test]
    async fn transient_purge_failure_is_retried() {
        let harness = Harness::new();
        let id = "orders/created";
        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;

        harness.execution.fail_next_purge();
        let error = harness
            .processor
            .process(&stream_event(&paused(id), id, 2), now())
            .await
            .unwrap_err();
        assert!(matches!(error, RetrySignal::ExecutionSchedule { .. }));
        assert!(error.to_string().starts_with("transient execution schedule failure:"));
    }

    #[tokio::test]
    async fn checkpoint_create_race_loser_acks_duplicate_stale() {
        let harness = Harness::new();
        let id = "orders/created";

        // The winner already applied position 1 and saved its checkpoint.
        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);

        // The loser loaded before the winner's first write landed, so it sees
        // no checkpoint and its save is a create that hits AlreadyExists.
        harness.kv.miss_next_entry();
        let processed = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::DuplicateStale);
        assert_eq!(processed.ack, AckAction::Ack);
        // The loser's idempotent upsert did not duplicate the schedule.
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    }

    #[tokio::test]
    async fn checkpoint_update_cas_loss_retries_and_converges() {
        let harness = Harness::new();
        let id = "orders/created";
        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;

        // The update loses a CAS race; the reloaded checkpoint does not cover
        // this position yet, so the record retries instead of poisoning.
        harness.kv.conflict_next_update();
        let error = harness
            .processor
            .process(&stream_event(&paused(id), id, 2), now())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            RetrySignal::Checkpoint {
                source: CheckpointStoreError::Conflict
            }
        ));
        assert!(!harness.kv.contains("failure.v1.SCHEDULER_SCHEDULE_EVENTS.2"));

        // Redelivery reconciles against the fresh checkpoint and converges.
        let retried = harness.process(&paused(id), id, 2).await;
        assert_eq!(retried.outcome, ProcessedOutcome::Purged);
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 0);
    }

    #[tokio::test]
    async fn transient_checkpoint_update_failure_is_retried() {
        let harness = Harness::new();
        let id = "orders/created";
        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;

        // The remove purges fine, but the checkpoint update write fails transiently.
        harness.kv.fail_next_update();
        let error = harness
            .processor
            .process(&stream_event(&removed(id), id, 2), now())
            .await
            .unwrap_err();
        assert!(matches!(error, RetrySignal::Checkpoint { .. }));
    }

    #[tokio::test]
    async fn kv_loss_is_recovered_by_replaying_creation() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert!(harness.kv.contains(&harness.checkpoint_key(id)));

        // Lose the rebuildable checkpoint cache.
        harness.kv.clear();
        assert!(!harness.kv.contains(&harness.checkpoint_key(id)));

        // A durable reset replays creation (DeliverPolicy::All); the checkpoint is
        // rebuilt and the execution schedule converges to exactly one message.
        let replayed = harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        assert_eq!(replayed.outcome, ProcessedOutcome::Published);
        assert!(harness.kv.contains(&harness.checkpoint_key(id)));
        assert_eq!(harness.execution.scheduled_count(harness.subject(id).as_str()), 1);
    }

    #[test]
    fn record_redelivery_is_infallible() {
        Harness::new().processor.record_redelivery();
    }

    #[tokio::test]
    async fn corrupt_cached_checkpoint_resume_event_is_poisoned() {
        let harness = Harness::new();
        let id = "orders/created";

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        harness.kv.corrupt(&harness.checkpoint_key(id));

        let processed = harness.process(&resumed(id), id, 2).await;

        assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
        assert_eq!(processed.ack, AckAction::Term);
    }

    #[test]
    fn outcome_from_maps_duplicate_stale() {
        assert_eq!(
            outcome_from(ReconcileOutcome::DuplicateStale),
            ProcessedOutcome::DuplicateStale
        );
    }

    #[test]
    fn recover_corrupt_checkpoint_without_revision_is_a_no_op() {
        let change = ScheduleChange::Paused {
            schedule_id: schedule_id("orders/created"),
        };
        assert!(
            recover_corrupt_checkpoint(&change, StreamPosition::try_new(2).unwrap(), Some("evt"), None, now())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn recover_corrupt_checkpoint_removed_event_purges() {
        let id = schedule_id("orders/created");
        let change = ScheduleChange::Removed {
            schedule_id: id.clone(),
        };
        let recovery = recover_corrupt_checkpoint(
            &change,
            StreamPosition::try_new(2).unwrap(),
            Some("evt"),
            Some(1),
            now(),
        )
        .unwrap()
        .expect("removed corrupt recovery should purge");

        assert!(matches!(recovery.action, ReconcileAction::Purge(_)));
        assert_eq!(recovery.next_checkpoint.status, ScheduleStatus::Removed);
    }

    #[tokio::test]
    async fn schedule_request_failure_during_create_is_poisoned() {
        let harness = Harness::new();
        let id = "orders/created";
        let subject = ScheduleSubject::execution(&key_for_stream(id));
        let delivery = Delivery::nats_event(subject.as_str()).unwrap();

        let processed = harness
            .process(
                &created_with_delivery(id, ScheduleEventStatus::Scheduled, every(), delivery),
                id,
                1,
            )
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
        assert_eq!(processed.ack, AckAction::Term);
    }

    #[tokio::test]
    async fn corrupt_create_with_invalid_delivery_is_poisoned_on_schedule_request() {
        let harness = Harness::new();
        let id = "orders/created";
        let subject = ScheduleSubject::execution(&key_for_stream(id));
        let delivery = Delivery::nats_event(subject.as_str()).unwrap();

        harness
            .process(&created(id, ScheduleEventStatus::Scheduled, every()), id, 1)
            .await;
        harness.kv.corrupt(&harness.checkpoint_key(id));

        let processed = harness
            .process(
                &created_with_delivery(id, ScheduleEventStatus::Scheduled, every(), delivery),
                id,
                2,
            )
            .await;

        assert_eq!(processed.outcome, ProcessedOutcome::DurableFailure);
        assert_eq!(processed.ack, AckAction::Term);
    }
}
