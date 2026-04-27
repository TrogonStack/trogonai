use trogon_eventsourcing::StreamState;
use trogon_eventsourcing::nats::jetstream::JetStreamStoreError;

use crate::read_model::MessageHeadersError;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug)]
pub enum CronError {
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
    JobAlreadyExists {
        id: String,
    },
    JobNotFound {
        id: String,
    },
    OptimisticConcurrencyConflict {
        id: String,
        expected: StreamState,
        current_version: Option<u64>,
    },
    Serde(serde_json::Error),
    InvalidJobSpec {
        source: JobSpecError,
    },
}

#[derive(Debug)]
pub enum JobSpecError {
    InvalidId {
        id: String,
        source: trogon_nats::SubjectTokenViolation,
    },
    EverySecondsMustBePositive,
    InvalidCronExpression {
        expr: String,
        source: BoxError,
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

impl std::fmt::Display for CronError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Event { context, source } => write!(f, "Event error: {context}: {source}"),
            Self::Kv { context, source } => write!(f, "KV error: {context}: {source}"),
            Self::Lease { context, source } => write!(f, "Lease error: {context}: {source}"),
            Self::Schedule { context, source } => {
                write!(f, "Schedule error: {context}: {source}")
            }
            Self::JobAlreadyExists { id } => write!(f, "Job '{id}' already exists"),
            Self::JobNotFound { id } => write!(f, "Job '{id}' not found"),
            Self::OptimisticConcurrencyConflict {
                id,
                expected,
                current_version,
            } => match current_version {
                Some(current_version) => write!(
                    f,
                    "OCC conflict for job '{id}': expected {expected:?}, current version is {current_version}"
                ),
                None => write!(
                    f,
                    "OCC conflict for job '{id}': expected {expected:?}, job has no current version"
                ),
            },
            Self::Serde(error) => write!(f, "Serialization error: {error}"),
            Self::InvalidJobSpec { source } => write!(f, "Invalid job spec: {source}"),
        }
    }
}

impl std::error::Error for CronError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Event { source, .. } => Some(source.as_ref()),
            Self::Kv { source, .. } => Some(source.as_ref()),
            Self::Lease { source, .. } => Some(source.as_ref()),
            Self::Schedule { source, .. } => Some(source.as_ref()),
            Self::Serde(error) => Some(error),
            Self::InvalidJobSpec { source } => Some(source),
            Self::JobAlreadyExists { .. } | Self::JobNotFound { .. } | Self::OptimisticConcurrencyConflict { .. } => {
                None
            }
        }
    }
}

impl CronError {
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

    pub fn invalid_job_spec(source: JobSpecError) -> Self {
        Self::InvalidJobSpec { source }
    }
}

impl std::fmt::Display for JobSpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId { id, source } => write!(f, "job id '{id}' is invalid: {source:?}"),
            Self::EverySecondsMustBePositive => f.write_str("every_sec must be >= 1"),
            Self::InvalidCronExpression { expr, source } => {
                write!(f, "cron expression '{expr}' is invalid: {source}")
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

impl std::error::Error for JobSpecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidId { source, .. } => Some(source),
            Self::InvalidCronExpression { source, .. } => Some(source.as_ref()),
            Self::InvalidRoute { source, .. } => Some(source),
            Self::InvalidSamplingSource { source, .. } => Some(source),
            Self::EverySecondsMustBePositive
            | Self::InvalidTimezone { .. }
            | Self::TtlMustBePositive
            | Self::InvalidHeaderName { .. }
            | Self::ReservedHeaderName { .. }
            | Self::InvalidHeaderValue { .. } => None,
        }
    }
}

impl From<serde_json::Error> for CronError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value)
    }
}

impl From<crate::proto::JobEventCodecError> for CronError {
    fn from(value: crate::proto::JobEventCodecError) -> Self {
        Self::event_source("failed to encode or decode job event payload", value)
    }
}

impl From<MessageHeadersError> for JobSpecError {
    fn from(value: MessageHeadersError) -> Self {
        match value {
            MessageHeadersError::InvalidName { name } => Self::InvalidHeaderName { name },
            MessageHeadersError::InvalidValue { name } => Self::InvalidHeaderValue { name },
        }
    }
}

impl From<trogon_eventsourcing::SnapshotStoreError> for CronError {
    fn from(value: trogon_eventsourcing::SnapshotStoreError) -> Self {
        match value {
            trogon_eventsourcing::SnapshotStoreError::Kv { context, source } => Self::Kv { context, source },
            trogon_eventsourcing::SnapshotStoreError::InvalidSnapshotKey { key } => {
                Self::event_source("failed to decode stream snapshot key", std::io::Error::other(key))
            }
            trogon_eventsourcing::SnapshotStoreError::MissingCheckpointName { key_prefix } => Self::event_source(
                "failed to resolve stream snapshot checkpoint key",
                std::io::Error::other(key_prefix),
            ),
            trogon_eventsourcing::SnapshotStoreError::Serde(source) => Self::Serde(source),
        }
    }
}

impl From<JetStreamStoreError<CronError>> for CronError {
    fn from(value: JetStreamStoreError<CronError>) -> Self {
        match value {
            JetStreamStoreError::ResolveSubject(source) | JetStreamStoreError::ProjectAppend(source) => source,
            JetStreamStoreError::ReadStream(source) => {
                Self::event_source("failed to read job stream while catching up command state", source)
            }
            JetStreamStoreError::AppendStream(source) => Self::event_source("failed to append job event batch", source),
            JetStreamStoreError::Snapshot(source) => Self::from(source),
            JetStreamStoreError::Codec(source) => source,
            JetStreamStoreError::OptimisticConcurrencyConflict {
                stream_id,
                expected,
                current_version,
            } => Self::OptimisticConcurrencyConflict {
                id: stream_id,
                expected,
                current_version,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_eventsourcing::StreamState;
    use trogon_nats::SubjectTokenViolation;

    #[test]
    fn invalid_job_spec_display_mentions_field() {
        let error = CronError::invalid_job_spec(JobSpecError::TtlMustBePositive);
        assert_eq!(error.to_string(), "Invalid job spec: ttl_sec must be >= 1");
    }

    #[test]
    fn kv_source_preserves_source() {
        let error = CronError::kv_source("open bucket", std::io::Error::other("boom"));
        assert_eq!(error.to_string(), "KV error: open bucket: boom");
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn cron_error_display_and_sources_cover_remaining_variants() {
        let event = CronError::event_source("replay", std::io::Error::other("broken"));
        assert_eq!(event.to_string(), "Event error: replay: broken");
        assert!(std::error::Error::source(&event).is_some());

        let lease = CronError::lease_source("renew", std::io::Error::other("lost"));
        assert_eq!(lease.to_string(), "Lease error: renew: lost");
        assert!(std::error::Error::source(&lease).is_some());

        let schedule = CronError::schedule_source("upsert", std::io::Error::other("rejected"));
        assert_eq!(schedule.to_string(), "Schedule error: upsert: rejected");
        assert!(std::error::Error::source(&schedule).is_some());

        let already_exists = CronError::JobAlreadyExists {
            id: "job-1".to_string(),
        };
        assert_eq!(already_exists.to_string(), "Job 'job-1' already exists");
        assert!(std::error::Error::source(&already_exists).is_none());

        let not_found = CronError::JobNotFound {
            id: "missing".to_string(),
        };
        assert_eq!(not_found.to_string(), "Job 'missing' not found");
        assert!(std::error::Error::source(&not_found).is_none());

        let occ_missing = CronError::OptimisticConcurrencyConflict {
            id: "job-1".to_string(),
            expected: StreamState::NoStream,
            current_version: None,
        };
        assert!(occ_missing.to_string().contains("job has no current version"));
        assert!(std::error::Error::source(&occ_missing).is_none());

        let occ_current = CronError::OptimisticConcurrencyConflict {
            id: "job-1".to_string(),
            expected: StreamState::StreamRevision(3),
            current_version: Some(4),
        };
        assert!(occ_current.to_string().contains("current version is 4"));
        assert!(std::error::Error::source(&occ_current).is_none());

        let serde_error: CronError = serde_json::from_str::<serde_json::Value>("{").unwrap_err().into();
        assert!(serde_error.to_string().starts_with("Serialization error:"));
        assert!(std::error::Error::source(&serde_error).is_some());
    }

    #[test]
    fn job_spec_error_display_and_sources_cover_remaining_variants() {
        let invalid_id = JobSpecError::InvalidId {
            id: "".to_string(),
            source: SubjectTokenViolation::Empty,
        };
        assert!(invalid_id.to_string().contains("job id '' is invalid"));
        assert!(std::error::Error::source(&invalid_id).is_some());

        let every = JobSpecError::EverySecondsMustBePositive;
        assert_eq!(every.to_string(), "every_sec must be >= 1");

        let invalid_cron = JobSpecError::InvalidCronExpression {
            expr: "bad cron".to_string(),
            source: Box::new(std::io::Error::other("invalid fields")),
        };
        assert!(
            invalid_cron
                .to_string()
                .contains("cron expression 'bad cron' is invalid")
        );
        assert!(std::error::Error::source(&invalid_cron).is_some());

        let timezone = JobSpecError::InvalidTimezone {
            timezone: "Mars/Base".to_string(),
        };
        assert_eq!(timezone.to_string(), "timezone 'Mars/Base' is invalid");
        assert!(std::error::Error::source(&timezone).is_none());

        let route = JobSpecError::InvalidRoute {
            route: "agent.>".to_string(),
            source: SubjectTokenViolation::InvalidCharacter('>'),
        };
        assert!(route.to_string().contains("route 'agent.>' is invalid"));
        assert!(std::error::Error::source(&route).is_some());

        let sampling = JobSpecError::InvalidSamplingSource {
            subject: "sensors.>".to_string(),
            source: SubjectTokenViolation::InvalidCharacter('>'),
        };
        assert!(sampling.to_string().contains("sampling source 'sensors.>' is invalid"));
        assert!(std::error::Error::source(&sampling).is_some());

        let invalid_header_name = JobSpecError::InvalidHeaderName { name: "\n".to_string() };
        assert_eq!(invalid_header_name.to_string(), "header name '\n' is invalid");

        let reserved = JobSpecError::ReservedHeaderName {
            name: "Nats-Schedule".to_string(),
        };
        assert_eq!(
            reserved.to_string(),
            "header name 'Nats-Schedule' is reserved by the scheduler"
        );

        let invalid_header_value = JobSpecError::InvalidHeaderValue {
            name: "x-kind".to_string(),
        };
        assert_eq!(
            invalid_header_value.to_string(),
            "header 'x-kind' contains an invalid value"
        );
        assert!(std::error::Error::source(&invalid_header_value).is_none());
    }
}
