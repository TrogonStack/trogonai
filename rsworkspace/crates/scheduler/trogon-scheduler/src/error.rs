use trogon_decider_nats::{JetStreamStoreError, OptimisticConcurrencyConflictError, SnapshotStoreError};
use trogon_decider_runtime::{StreamPosition, StreamWritePrecondition};

use crate::commands::domain::MessageHeadersError;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("Event error: {context}: {source}")]
    Event { context: &'static str, source: BoxError },
    #[error("KV error: {context}: {source}")]
    Kv { context: &'static str, source: BoxError },
    #[error("Lease error: {context}: {source}")]
    Lease { context: &'static str, source: BoxError },
    #[error("Schedule error: {context}: {source}")]
    Schedule { context: &'static str, source: BoxError },
    #[error("Schedule '{id}' already exists")]
    ScheduleAlreadyExists { id: String },
    #[error("Schedule '{id}' not found")]
    ScheduleNotFound { id: String },
    #[error("OCC conflict for schedule '{id}': expected {expected:?}, {}", occ_position_detail(.current_position))]
    OptimisticConcurrencyConflict {
        id: String,
        expected: StreamWritePrecondition,
        current_position: Option<StreamPosition>,
    },
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("Invalid schedule spec: {source}")]
    InvalidScheduleSpec { source: ScheduleSpecError },
}

fn occ_position_detail(current_position: &Option<StreamPosition>) -> String {
    match current_position {
        Some(current_position) => format!("current position is {current_position}"),
        None => "schedule has no current position".to_string(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleSpecError {
    #[error("schedule id '{id}' is invalid: {source:?}")]
    InvalidId {
        id: String,
        source: trogon_nats::SubjectTokenViolationError,
    },
    #[error("every_sec must be >= 1")]
    EverySecondsMustBePositive,
    #[error("cron expression '{expr}' is invalid: {source}")]
    InvalidCronExpression { expr: String, source: BoxError },
    #[error("RRULE '{rrule}' is invalid: {source}")]
    InvalidRRule { rrule: String, source: BoxError },
    #[error("RRULE {field} timestamp '{value}' is invalid: {source}")]
    InvalidRRuleDateTime {
        field: &'static str,
        value: String,
        source: BoxError,
    },
    #[error("RRULE '{rrule}' has no next occurrence")]
    RRuleHasNoNextOccurrence { rrule: String },
    #[error("timezone '{timezone}' is invalid")]
    InvalidTimezone { timezone: String },
    #[error("route '{route}' is invalid: {source:?}")]
    InvalidRoute {
        route: String,
        source: trogon_nats::SubjectTokenViolationError,
    },
    #[error("sampling source '{subject}' is invalid: {source:?}")]
    InvalidSamplingSource {
        subject: String,
        source: trogon_nats::SubjectTokenViolationError,
    },
    #[error("ttl_sec must be >= 1")]
    TtlMustBePositive,
    #[error("header name '{name}' is invalid")]
    InvalidHeaderName { name: String },
    #[error("header name '{name}' is reserved by the scheduler")]
    ReservedHeaderName { name: String },
    #[error("header '{name}' contains an invalid value")]
    InvalidHeaderValue { name: String },
}

impl SchedulerError {
    pub fn event_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Event {
            context,
            source: Box::new(source),
        }
    }

    pub fn kv_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Kv {
            context,
            source: Box::new(source),
        }
    }

    pub fn lease_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Lease {
            context,
            source: Box::new(source),
        }
    }

    pub fn schedule_source<E>(context: &'static str, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        Self::Schedule {
            context,
            source: Box::new(source),
        }
    }

    pub fn invalid_schedule_spec(source: ScheduleSpecError) -> Self {
        Self::InvalidScheduleSpec { source }
    }
}

impl From<trogonai_proto::scheduler::schedules::ScheduleEventPayloadError> for SchedulerError {
    fn from(value: trogonai_proto::scheduler::schedules::ScheduleEventPayloadError) -> Self {
        Self::event_source("failed to encode or decode schedule event payload", value)
    }
}

impl From<MessageHeadersError> for ScheduleSpecError {
    fn from(value: MessageHeadersError) -> Self {
        match value {
            MessageHeadersError::InvalidName { name } => Self::InvalidHeaderName { name },
            MessageHeadersError::InvalidValue { name } => Self::InvalidHeaderValue { name },
        }
    }
}

impl<PayloadError, SnapshotTypeError> From<SnapshotStoreError<PayloadError, SnapshotTypeError>> for SchedulerError
where
    PayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError: std::error::Error + Send + Sync + 'static,
{
    fn from(value: SnapshotStoreError<PayloadError, SnapshotTypeError>) -> Self {
        match value {
            SnapshotStoreError::Kv(source) => Self::kv_source("failed to access stream snapshot storage", source),
            SnapshotStoreError::Codec(source) => {
                Self::event_source("failed to encode or decode stream snapshot", source)
            }
            SnapshotStoreError::InvalidSnapshotKey { key } => {
                Self::event_source("failed to decode stream snapshot key", std::io::Error::other(key))
            }
            SnapshotStoreError::MissingCheckpointName { snapshot_type } => Self::event_source(
                "failed to resolve stream snapshot checkpoint key",
                std::io::Error::other(snapshot_type.to_string()),
            ),
        }
    }
}

impl<SnapshotPayloadError, SnapshotTypeError>
    From<JetStreamStoreError<SchedulerError, SnapshotPayloadError, SnapshotTypeError>> for SchedulerError
where
    SnapshotPayloadError: std::error::Error + Send + Sync + 'static,
    SnapshotTypeError: std::error::Error + Send + Sync + 'static,
{
    fn from(value: JetStreamStoreError<SchedulerError, SnapshotPayloadError, SnapshotTypeError>) -> Self {
        match value {
            JetStreamStoreError::ResolveSubject(source) => source,
            JetStreamStoreError::ReadStream(source) => {
                Self::event_source("failed to read schedule stream while catching up command state", source)
            }
            JetStreamStoreError::AppendStream(source) => {
                Self::event_source("failed to append schedule event batch", source)
            }
            JetStreamStoreError::Snapshot(source) => Self::from(source),
            JetStreamStoreError::Codec(source) => source,
            JetStreamStoreError::OptimisticConcurrencyConflict(source) => match source {
                OptimisticConcurrencyConflictError::WithPosition {
                    stream_id,
                    expected,
                    current_position,
                } => Self::OptimisticConcurrencyConflict {
                    id: stream_id,
                    expected,
                    current_position: Some(current_position),
                },
                OptimisticConcurrencyConflictError::NoPosition { stream_id, expected } => {
                    Self::OptimisticConcurrencyConflict {
                        id: stream_id,
                        expected,
                        current_position: None,
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests;
