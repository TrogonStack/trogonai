use trogon_decider_nats::{JetStreamStoreError, SnapshotStoreError};
use trogon_decider_runtime::{StreamPosition, StreamWritePrecondition};

use crate::commands::domain::MessageHeadersError;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub enum SchedulerError {
    Event {
        context: &'static str,
        source: BoxError,
    },
    Kv {
        context: &'static str,
        source: BoxError,
    },
    Lease {
        context: &'static str,
        source: BoxError,
    },
    Schedule {
        context: &'static str,
        source: BoxError,
    },
    ScheduleAlreadyExists {
        id: String,
    },
    ScheduleNotFound {
        id: String,
    },
    OptimisticConcurrencyConflict {
        id: String,
        expected: StreamWritePrecondition,
        current_position: Option<StreamPosition>,
    },
    Serde(serde_json::Error),
    InvalidScheduleSpec {
        source: ScheduleSpecError,
    },
}

#[derive(Debug)]
pub enum ScheduleSpecError {
    InvalidId {
        id: String,
        source: trogon_nats::SubjectTokenViolation,
    },
    EverySecondsMustBePositive,
    InvalidCronExpression {
        expr: String,
        source: BoxError,
    },
    InvalidRRule {
        rrule: String,
        source: BoxError,
    },
    InvalidRRuleDateTime {
        field: &'static str,
        value: String,
        source: BoxError,
    },
    RRuleHasNoNextOccurrence {
        rrule: String,
    },
    InvalidTimezone {
        timezone: String,
    },
    InvalidRoute {
        route: String,
        source: trogon_nats::SubjectTokenViolation,
    },
    InvalidSamplingSource {
        subject: String,
        source: trogon_nats::SubjectTokenViolation,
    },
    TtlMustBePositive,
    InvalidHeaderName {
        name: String,
    },
    ReservedHeaderName {
        name: String,
    },
    InvalidHeaderValue {
        name: String,
    },
}

impl std::fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Event { context, source } => write!(f, "Event error: {context}: {source}"),
            Self::Kv { context, source } => write!(f, "KV error: {context}: {source}"),
            Self::Lease { context, source } => write!(f, "Lease error: {context}: {source}"),
            Self::Schedule { context, source } => {
                write!(f, "Schedule error: {context}: {source}")
            }
            Self::ScheduleAlreadyExists { id } => write!(f, "Schedule '{id}' already exists"),
            Self::ScheduleNotFound { id } => write!(f, "Schedule '{id}' not found"),
            Self::OptimisticConcurrencyConflict {
                id,
                expected,
                current_position,
            } => match current_position {
                Some(current_position) => write!(
                    f,
                    "OCC conflict for schedule '{id}': expected {expected:?}, current position is {current_position}"
                ),
                None => write!(
                    f,
                    "OCC conflict for schedule '{id}': expected {expected:?}, schedule has no current position"
                ),
            },
            Self::Serde(error) => write!(f, "Serialization error: {error}"),
            Self::InvalidScheduleSpec { source } => write!(f, "Invalid schedule spec: {source}"),
        }
    }
}

impl std::error::Error for SchedulerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Event { source, .. } => Some(source.as_ref()),
            Self::Kv { source, .. } => Some(source.as_ref()),
            Self::Lease { source, .. } => Some(source.as_ref()),
            Self::Schedule { source, .. } => Some(source.as_ref()),
            Self::Serde(error) => Some(error),
            Self::InvalidScheduleSpec { source } => Some(source),
            Self::ScheduleAlreadyExists { .. }
            | Self::ScheduleNotFound { .. }
            | Self::OptimisticConcurrencyConflict { .. } => None,
        }
    }
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

impl std::fmt::Display for ScheduleSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId { id, source } => write!(f, "schedule id '{id}' is invalid: {source:?}"),
            Self::EverySecondsMustBePositive => f.write_str("every_sec must be >= 1"),
            Self::InvalidCronExpression { expr, source } => {
                write!(f, "cron expression '{expr}' is invalid: {source}")
            }
            Self::InvalidRRule { rrule, source } => {
                write!(f, "RRULE '{rrule}' is invalid: {source}")
            }
            Self::InvalidRRuleDateTime { field, value, source } => {
                write!(f, "RRULE {field} timestamp '{value}' is invalid: {source}")
            }
            Self::RRuleHasNoNextOccurrence { rrule } => {
                write!(f, "RRULE '{rrule}' has no next occurrence")
            }
            Self::InvalidTimezone { timezone } => {
                write!(f, "timezone '{timezone}' is invalid")
            }
            Self::InvalidRoute { route, source } => {
                write!(f, "route '{route}' is invalid: {source:?}")
            }
            Self::InvalidSamplingSource { subject, source } => {
                write!(f, "sampling source '{subject}' is invalid: {source:?}")
            }
            Self::TtlMustBePositive => f.write_str("ttl_sec must be >= 1"),
            Self::InvalidHeaderName { name } => write!(f, "header name '{name}' is invalid"),
            Self::ReservedHeaderName { name } => {
                write!(f, "header name '{name}' is reserved by the scheduler")
            }
            Self::InvalidHeaderValue { name } => {
                write!(f, "header '{name}' contains an invalid value")
            }
        }
    }
}

impl std::error::Error for ScheduleSpecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidId { source, .. } => Some(source),
            Self::InvalidCronExpression { source, .. } => Some(source.as_ref()),
            Self::InvalidRRule { source, .. } => Some(source.as_ref()),
            Self::InvalidRRuleDateTime { source, .. } => Some(source.as_ref()),
            Self::InvalidRoute { source, .. } => Some(source),
            Self::InvalidSamplingSource { source, .. } => Some(source),
            Self::EverySecondsMustBePositive
            | Self::RRuleHasNoNextOccurrence { .. }
            | Self::InvalidTimezone { .. }
            | Self::TtlMustBePositive
            | Self::InvalidHeaderName { .. }
            | Self::ReservedHeaderName { .. }
            | Self::InvalidHeaderValue { .. } => None,
        }
    }
}

impl From<serde_json::Error> for SchedulerError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value)
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
            JetStreamStoreError::OptimisticConcurrencyConflict(source) => Self::OptimisticConcurrencyConflict {
                id: source.stream_id,
                expected: source.expected,
                current_position: source.current_position,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_decider_runtime::{StreamPosition, StreamWritePrecondition};
    use trogon_nats::SubjectTokenViolation;

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test stream position must be non-zero")
    }

    #[test]
    fn invalid_schedule_spec_display_mentions_field() {
        let error = SchedulerError::invalid_schedule_spec(ScheduleSpecError::TtlMustBePositive);
        assert_eq!(error.to_string(), "Invalid schedule spec: ttl_sec must be >= 1");
    }

    #[test]
    fn kv_source_preserves_source() {
        let error = SchedulerError::kv_source("open bucket", std::io::Error::other("boom"));
        assert_eq!(error.to_string(), "KV error: open bucket: boom");
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn scheduler_error_display_and_sources_cover_remaining_variants() {
        let event = SchedulerError::event_source("replay", std::io::Error::other("broken"));
        assert_eq!(event.to_string(), "Event error: replay: broken");
        assert!(std::error::Error::source(&event).is_some());

        let lease = SchedulerError::lease_source("renew", std::io::Error::other("lost"));
        assert_eq!(lease.to_string(), "Lease error: renew: lost");
        assert!(std::error::Error::source(&lease).is_some());

        let schedule = SchedulerError::schedule_source("upsert", std::io::Error::other("rejected"));
        assert_eq!(schedule.to_string(), "Schedule error: upsert: rejected");
        assert!(std::error::Error::source(&schedule).is_some());

        let already_exists = SchedulerError::ScheduleAlreadyExists {
            id: "job-1".to_string(),
        };
        assert_eq!(already_exists.to_string(), "Schedule 'job-1' already exists");
        assert!(std::error::Error::source(&already_exists).is_none());

        let not_found = SchedulerError::ScheduleNotFound {
            id: "missing".to_string(),
        };
        assert_eq!(not_found.to_string(), "Schedule 'missing' not found");
        assert!(std::error::Error::source(&not_found).is_none());

        let occ_missing = SchedulerError::OptimisticConcurrencyConflict {
            id: "job-1".to_string(),
            expected: StreamWritePrecondition::NoStream,
            current_position: None,
        };
        assert!(occ_missing.to_string().contains("schedule has no current position"));
        assert!(std::error::Error::source(&occ_missing).is_none());

        let occ_current = SchedulerError::OptimisticConcurrencyConflict {
            id: "job-1".to_string(),
            expected: StreamWritePrecondition::At(position(3)),
            current_position: Some(position(4)),
        };
        assert!(occ_current.to_string().contains("current position is 4"));
        assert!(std::error::Error::source(&occ_current).is_none());

        let serde_error: SchedulerError = serde_json::from_str::<serde_json::Value>("{").unwrap_err().into();
        assert!(serde_error.to_string().starts_with("Serialization error:"));
        assert!(std::error::Error::source(&serde_error).is_some());
    }

    #[test]
    fn job_spec_error_display_and_sources_cover_remaining_variants() {
        let invalid_id = ScheduleSpecError::InvalidId {
            id: "".to_string(),
            source: SubjectTokenViolation::Empty,
        };
        assert!(invalid_id.to_string().contains("schedule id '' is invalid"));
        assert!(std::error::Error::source(&invalid_id).is_some());

        let every = ScheduleSpecError::EverySecondsMustBePositive;
        assert_eq!(every.to_string(), "every_sec must be >= 1");

        let invalid_cron = ScheduleSpecError::InvalidCronExpression {
            expr: "bad cron".to_string(),
            source: Box::new(std::io::Error::other("invalid fields")),
        };
        assert!(
            invalid_cron
                .to_string()
                .contains("cron expression 'bad cron' is invalid")
        );
        assert!(std::error::Error::source(&invalid_cron).is_some());

        let timezone = ScheduleSpecError::InvalidTimezone {
            timezone: "Mars/Base".to_string(),
        };
        assert_eq!(timezone.to_string(), "timezone 'Mars/Base' is invalid");
        assert!(std::error::Error::source(&timezone).is_none());

        let route = ScheduleSpecError::InvalidRoute {
            route: "agent.>".to_string(),
            source: SubjectTokenViolation::InvalidCharacter('>'),
        };
        assert!(route.to_string().contains("route 'agent.>' is invalid"));
        assert!(std::error::Error::source(&route).is_some());

        let sampling = ScheduleSpecError::InvalidSamplingSource {
            subject: "sensors.>".to_string(),
            source: SubjectTokenViolation::InvalidCharacter('>'),
        };
        assert!(sampling.to_string().contains("sampling source 'sensors.>' is invalid"));
        assert!(std::error::Error::source(&sampling).is_some());

        let invalid_header_name = ScheduleSpecError::InvalidHeaderName { name: "\n".to_string() };
        assert_eq!(invalid_header_name.to_string(), "header name '\n' is invalid");

        let reserved = ScheduleSpecError::ReservedHeaderName {
            name: "Nats-Schedule".to_string(),
        };
        assert_eq!(
            reserved.to_string(),
            "header name 'Nats-Schedule' is reserved by the scheduler"
        );

        let invalid_header_value = ScheduleSpecError::InvalidHeaderValue {
            name: "x-kind".to_string(),
        };
        assert_eq!(
            invalid_header_value.to_string(),
            "header 'x-kind' contains an invalid value"
        );
        assert!(std::error::Error::source(&invalid_header_value).is_none());
    }
}
