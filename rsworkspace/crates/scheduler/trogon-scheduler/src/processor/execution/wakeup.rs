use std::convert::Infallible;
use std::time::Duration;

use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::consumer::{AckPolicy, DeliverPolicy, ReplayPolicy};
use chrono::{DateTime, Utc};
use trogon_decider_runtime::{CommandError, CommandExecution, StreamAppend, StreamPosition, StreamRead};
use trogon_std::time::{EpochClock, SystemClock};

use crate::commands::domain::ScheduleId;
use crate::commands::{EvolveError, RecordScheduleOccurrence, RecordScheduleOccurrenceError};
use crate::processor::execution::reconciliation::{
    RRuleWakeupPayload, RRuleWakeupPayloadDecodeError, ScheduleKey, ScheduleSubject,
};
use trogonai_proto::scheduler::schedules::ScheduleEventPayloadError;

pub const RRULE_WAKEUP_FILTER: &str = "scheduler.schedules.execution.v1.rrule.>";
pub const RRULE_WAKEUP_CONSUMER: &str = "scheduler_rrule_wakeup_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RRuleWakeupObsoleteReason {
    Missing,
    Deleted,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RRuleWakeupOutcome {
    Recorded { stream_position: StreamPosition },
    DuplicateStale,
    Obsolete { reason: RRuleWakeupObsoleteReason },
}

#[derive(Debug, thiserror::Error)]
pub enum RRuleWakeupError {
    #[error("RRULE wakeup payload is invalid: {source}")]
    Payload {
        #[source]
        source: RRuleWakeupPayloadDecodeError,
    },
    #[error("RRULE wakeup subject '{actual}' does not match expected subject '{expected}'")]
    SubjectMismatch {
        expected: ScheduleSubject,
        actual: RRuleWakeupSubjectInput,
    },
    #[error("RRULE wakeup command was rejected: {source}")]
    CommandRejected {
        #[source]
        source: RecordScheduleOccurrenceError,
    },
    #[error("RRULE wakeup command failed permanently: {source}")]
    CommandPermanent {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("RRULE wakeup command failed transiently: {source}")]
    CommandTransient {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl RRuleWakeupError {
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::CommandTransient { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RRuleWakeupSubjectInput(String);

impl RRuleWakeupSubjectInput {
    fn new(subject: impl Into<String>) -> Self {
        Self(subject.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RRuleWakeupSubjectInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct RRuleWakeupProcessor<E, C = SystemClock> {
    event_store: E,
    clock: C,
}

impl<E> RRuleWakeupProcessor<E, SystemClock> {
    pub fn new(event_store: E) -> Self {
        Self::with_clock(event_store, SystemClock)
    }
}

impl<E, C> RRuleWakeupProcessor<E, C> {
    pub fn with_clock(event_store: E, clock: C) -> Self {
        Self { event_store, clock }
    }
}

impl<E, C> RRuleWakeupProcessor<E, C>
where
    E: StreamRead<str> + StreamAppend<str>,
    <E as StreamRead<str>>::Error: std::error::Error + Send + Sync + 'static,
    <E as StreamAppend<str>>::Error: std::error::Error + Send + Sync + 'static,
    C: EpochClock,
{
    pub async fn process(&self, subject: &str, payload: &[u8]) -> Result<RRuleWakeupOutcome, RRuleWakeupError> {
        let subject = RRuleWakeupSubjectInput::new(subject);
        let wakeup = RRuleWakeupPayload::decode(payload).map_err(|source| RRuleWakeupError::Payload { source })?;
        let schedule_id = wakeup.schedule_id().clone();
        let occurrence_at = wakeup.occurrence_at();
        ensure_subject_matches_payload(&subject, &schedule_id)?;

        let recorded_at = DateTime::<Utc>::from(self.clock.system_time());
        let command = RecordScheduleOccurrence::new(schedule_id, occurrence_at, recorded_at);
        match CommandExecution::new(&self.event_store, &command).execute().await {
            Ok(result) => Ok(RRuleWakeupOutcome::Recorded {
                stream_position: result.stream_position,
            }),
            Err(CommandError::Decide(source)) => wakeup_outcome_from_rejection(source),
            Err(error) => Err(command_error(error)),
        }
    }
}

pub fn rrule_wakeup_consumer_config() -> pull::Config {
    pull::Config {
        durable_name: Some(RRULE_WAKEUP_CONSUMER.to_string()),
        description: Some("records fired RRULE wakeups into schedule event streams".to_string()),
        deliver_policy: DeliverPolicy::All,
        ack_policy: AckPolicy::Explicit,
        ack_wait: Duration::from_secs(120),
        max_deliver: -1,
        filter_subject: RRULE_WAKEUP_FILTER.to_string(),
        replay_policy: ReplayPolicy::Instant,
        max_waiting: 32,
        max_ack_pending: 256,
        max_batch: 64,
        max_expires: Duration::from_secs(5),
        backoff: Vec::new(),
        ..Default::default()
    }
}

fn ensure_subject_matches_payload(
    subject: &RRuleWakeupSubjectInput,
    schedule_id: &ScheduleId,
) -> Result<(), RRuleWakeupError> {
    let expected = ScheduleSubject::rrule_wakeup(&ScheduleKey::derive(schedule_id));
    if subject.as_str() == expected.as_str() {
        return Ok(());
    }

    Err(RRuleWakeupError::SubjectMismatch {
        expected,
        actual: subject.clone(),
    })
}

fn wakeup_outcome_from_rejection(
    source: RecordScheduleOccurrenceError,
) -> Result<RRuleWakeupOutcome, RRuleWakeupError> {
    match source {
        RecordScheduleOccurrenceError::ScheduleNotFound { .. } => Ok(RRuleWakeupOutcome::Obsolete {
            reason: RRuleWakeupObsoleteReason::Missing,
        }),
        RecordScheduleOccurrenceError::ScheduleDeleted { .. } => Ok(RRuleWakeupOutcome::Obsolete {
            reason: RRuleWakeupObsoleteReason::Deleted,
        }),
        RecordScheduleOccurrenceError::OccurrenceAlreadyRecorded { .. }
        | RecordScheduleOccurrenceError::OccurrenceNotPending { .. } => Ok(RRuleWakeupOutcome::DuplicateStale),
        other => Err(RRuleWakeupError::CommandRejected { source: other }),
    }
}

type WakeupCommandError<ReadStreamError, AppendStreamError> = CommandError<
    RecordScheduleOccurrenceError,
    EvolveError,
    Infallible,
    ReadStreamError,
    AppendStreamError,
    ScheduleEventPayloadError,
    ScheduleEventPayloadError,
    ScheduleEventPayloadError,
>;

fn command_error<ReadStreamError, AppendStreamError>(
    error: WakeupCommandError<ReadStreamError, AppendStreamError>,
) -> RRuleWakeupError
where
    ReadStreamError: std::error::Error + Send + Sync + 'static,
    AppendStreamError: std::error::Error + Send + Sync + 'static,
{
    match error {
        CommandError::ReadStream(_) | CommandError::Append(_) => RRuleWakeupError::CommandTransient {
            source: Box::new(error),
        },
        other => RRuleWakeupError::CommandPermanent {
            source: Box::new(other),
        },
    }
}

#[cfg(test)]
mod tests;
