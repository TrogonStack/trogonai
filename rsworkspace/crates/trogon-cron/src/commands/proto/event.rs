use chrono::{DateTime, Utc};
use trogon_cron_jobs_proto::v1;

use crate::commands::domain::{
    Job, JobDetails, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus, MessageContent,
    MessageEnvelope, MessageHeaders, MessageHeadersError,
};

pub use trogon_cron_jobs_proto::{
    JOB_ADDED_EVENT_TYPE, JOB_PAUSED_EVENT_TYPE, JOB_REMOVED_EVENT_TYPE, JOB_RESUMED_EVENT_TYPE, JobEventCodec,
    JobEventCodecError,
};

#[derive(Debug)]
pub enum JobEventProtoError {
    MissingEvent,
    MissingJobDetails,
    MissingSchedule,
    MissingDelivery,
    MissingMessage,
    MissingScheduleKind,
    MissingDeliveryKind,
    MissingSamplingSourceKind,
    UnknownJobStatus { value: i32 },
    InvalidTimestamp { value: String, source: chrono::ParseError },
    InvalidHeaders(MessageHeadersError),
}

impl std::fmt::Display for JobEventProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEvent => f.write_str("protobuf job event is missing its oneof case"),
            Self::MissingJobDetails => f.write_str("protobuf job_added is missing job details"),
            Self::MissingSchedule => f.write_str("protobuf job details are missing schedule"),
            Self::MissingDelivery => f.write_str("protobuf job details are missing delivery"),
            Self::MissingMessage => f.write_str("protobuf job details are missing message"),
            Self::MissingScheduleKind => f.write_str("protobuf job schedule is missing its oneof case"),
            Self::MissingDeliveryKind => f.write_str("protobuf job delivery is missing its oneof case"),
            Self::MissingSamplingSourceKind => f.write_str("protobuf sampling source is missing its oneof case"),
            Self::UnknownJobStatus { value } => write!(f, "protobuf job status '{value}' is unknown"),
            Self::InvalidTimestamp { value, source } => {
                write!(f, "protobuf timestamp '{value}' is invalid: {source}")
            }
            Self::InvalidHeaders(source) => write!(f, "protobuf headers are invalid: {source}"),
        }
    }
}

impl std::error::Error for JobEventProtoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidTimestamp { source, .. } => Some(source),
            Self::InvalidHeaders(source) => Some(source),
            Self::MissingEvent
            | Self::MissingJobDetails
            | Self::MissingSchedule
            | Self::MissingDelivery
            | Self::MissingMessage
            | Self::MissingScheduleKind
            | Self::MissingDeliveryKind
            | Self::MissingSamplingSourceKind
            | Self::UnknownJobStatus { .. } => None,
        }
    }
}

impl From<&JobDetails> for v1::JobDetails {
    fn from(value: &JobDetails) -> Self {
        let mut job = v1::JobDetails::new();
        job.set_status(v1::JobStatus::from(value.status));
        job.set_schedule(v1::JobSchedule::from(&value.schedule));
        job.set_delivery(v1::JobDelivery::from(&value.delivery));
        job.set_message(v1::JobMessage::from(&value.message));
        job
    }
}

impl From<&Job> for v1::JobDetails {
    fn from(value: &Job) -> Self {
        Self::from(&JobDetails::from(value))
    }
}

impl TryFrom<v1::JobDetails> for JobDetails {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobDetails) -> Result<Self, Self::Error> {
        if !value.has_schedule() {
            return Err(JobEventProtoError::MissingSchedule);
        }
        if !value.has_delivery() {
            return Err(JobEventProtoError::MissingDelivery);
        }
        if !value.has_message() {
            return Err(JobEventProtoError::MissingMessage);
        }
        Ok(Self {
            status: value.status().try_into()?,
            schedule: value.schedule().to_owned().try_into()?,
            delivery: value.delivery().to_owned().try_into()?,
            message: value.message().to_owned().try_into()?,
        })
    }
}

impl From<JobEventStatus> for v1::JobStatus {
    fn from(value: JobEventStatus) -> Self {
        match value {
            JobEventStatus::Enabled => Self::Enabled,
            JobEventStatus::Disabled => Self::Disabled,
        }
    }
}

impl TryFrom<v1::JobStatus> for JobEventStatus {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobStatus) -> Result<Self, Self::Error> {
        match i32::from(value) {
            1 => Ok(Self::Enabled),
            2 => Ok(Self::Disabled),
            other => Err(JobEventProtoError::UnknownJobStatus { value: other }),
        }
    }
}

impl From<&JobEventSchedule> for v1::JobSchedule {
    fn from(value: &JobEventSchedule) -> Self {
        let mut schedule = v1::JobSchedule::new();
        match value {
            JobEventSchedule::At { at } => {
                let mut inner = v1::AtSchedule::new();
                inner.set_at(at.to_rfc3339());
                schedule.set_at(inner);
            }
            JobEventSchedule::Every { every_sec } => {
                let mut inner = v1::EverySchedule::new();
                inner.set_every_sec(*every_sec);
                schedule.set_every(inner);
            }
            JobEventSchedule::Cron { expr, timezone } => {
                let mut inner = v1::CronSchedule::new();
                inner.set_expr(expr.as_str());
                if let Some(timezone) = timezone {
                    inner.set_timezone(timezone.as_str());
                }
                schedule.set_cron(inner);
            }
        }
        schedule
    }
}

impl TryFrom<v1::JobSchedule> for JobEventSchedule {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobSchedule) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_schedule::KindOneof::At(inner) => {
                let at = inner.at().to_string();
                let parsed = DateTime::parse_from_rfc3339(&at)
                    .map_err(|source| JobEventProtoError::InvalidTimestamp { value: at, source })?
                    .with_timezone(&Utc);
                Ok(Self::At { at: parsed })
            }
            v1::job_schedule::KindOneof::Every(inner) => Ok(Self::Every {
                every_sec: inner.every_sec(),
            }),
            v1::job_schedule::KindOneof::Cron(inner) => Ok(Self::Cron {
                expr: inner.expr().to_string(),
                timezone: inner.has_timezone().then(|| inner.timezone().to_string()),
            }),
            v1::job_schedule::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingScheduleKind),
        }
    }
}

impl From<&JobEventDelivery> for v1::JobDelivery {
    fn from(value: &JobEventDelivery) -> Self {
        let mut delivery = v1::JobDelivery::new();
        match value {
            JobEventDelivery::NatsEvent { route, ttl_sec, source } => {
                let mut inner = v1::NatsEventDelivery::new();
                inner.set_route(route.as_str());
                if let Some(ttl_sec) = ttl_sec {
                    inner.set_ttl_sec(*ttl_sec);
                }
                if let Some(source) = source {
                    inner.set_source(v1::JobSamplingSource::from(source));
                }
                delivery.set_nats_event(inner);
            }
        }
        delivery
    }
}

impl TryFrom<v1::JobDelivery> for JobEventDelivery {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobDelivery) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_delivery::KindOneof::NatsEvent(inner) => Ok(Self::NatsEvent {
                route: inner.route().to_string(),
                ttl_sec: inner.has_ttl_sec().then(|| inner.ttl_sec()),
                source: inner
                    .has_source()
                    .then(|| inner.source().to_owned().try_into())
                    .transpose()?,
            }),
            v1::job_delivery::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingDeliveryKind),
        }
    }
}

impl From<&JobEventSamplingSource> for v1::JobSamplingSource {
    fn from(value: &JobEventSamplingSource) -> Self {
        let mut source = v1::JobSamplingSource::new();
        match value {
            JobEventSamplingSource::LatestFromSubject { subject } => {
                let mut inner = v1::LatestFromSubjectSampling::new();
                inner.set_subject(subject.as_str());
                source.set_latest_from_subject(inner);
            }
        }
        source
    }
}

impl TryFrom<v1::JobSamplingSource> for JobEventSamplingSource {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobSamplingSource) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_sampling_source::KindOneof::LatestFromSubject(inner) => Ok(Self::LatestFromSubject {
                subject: inner.subject().to_string(),
            }),
            v1::job_sampling_source::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingSamplingSourceKind),
        }
    }
}

impl From<&MessageEnvelope> for v1::JobMessage {
    fn from(value: &MessageEnvelope) -> Self {
        let mut message = v1::JobMessage::new();
        message.set_content(value.content.as_str());
        for (name, val) in value.headers.as_slice() {
            let mut header = v1::Header::new();
            header.set_name(name.as_str());
            header.set_value(val.as_str());
            message.headers_mut().push(header);
        }
        message
    }
}

impl TryFrom<v1::JobMessage> for MessageEnvelope {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobMessage) -> Result<Self, Self::Error> {
        let headers = value
            .headers()
            .iter()
            .map(|header| (header.name().to_string(), header.value().to_string()))
            .collect::<Vec<_>>();

        Ok(Self {
            content: MessageContent::new(value.content().to_string()),
            headers: MessageHeaders::new(headers).map_err(JobEventProtoError::InvalidHeaders)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Parse as _;
    use trogon_eventsourcing::{EventCodec, EventData};

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let mut removed = v1::JobEvent::new();
        removed.set_job_removed(v1::JobRemoved::new());
        let event = EventData::new_with_codec("cleanup", &JobEventCodec, removed.clone()).unwrap();
        assert_eq!(event.stream_id(), "cleanup");
        assert_eq!(event.event_type, JOB_REMOVED_EVENT_TYPE);
        assert!(v1::JobRemoved::parse(&event.payload).is_ok());
        assert_eq!(
            event.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );

        assert_eq!(event.decode_data_with(&JobEventCodec).unwrap(), removed);

        let recorded = event.record(
            "cron.jobs.events.cleanup",
            None,
            Some(9),
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );
        assert_eq!(recorded.stream_id(), "cleanup");
        assert_eq!(recorded.recorded_stream_id, "cron.jobs.events.cleanup");
        assert_eq!(recorded.log_position, Some(9));
        assert_eq!(
            recorded.subject_with_prefix("cron.events.jobs."),
            "cron.events.jobs.cleanup"
        );
        let mut expected = v1::JobEvent::new();
        expected.set_job_removed(v1::JobRemoved::new());
        assert_eq!(recorded.decode_data_with(&JobEventCodec).unwrap(), expected);
    }

    #[test]
    fn invalid_payload_fails_decode() {
        assert!(JobEventCodec.decode(JOB_REMOVED_EVENT_TYPE, "cleanup", b"\0").is_err());
        assert!(
            JobEventCodec
                .decode("trogon.cron.jobs.v1.Unknown", "cleanup", &[])
                .is_err()
        );
    }

    #[test]
    fn job_added_round_trips_through_contract() {
        let details = JobDetails {
            status: JobEventStatus::Enabled,
            schedule: JobEventSchedule::Cron {
                expr: "0 * * * * *".to_string(),
                timezone: Some("UTC".to_string()),
            },
            delivery: JobEventDelivery::NatsEvent {
                route: "ops.backup".to_string(),
                ttl_sec: Some(30),
                source: Some(JobEventSamplingSource::LatestFromSubject {
                    subject: "events.backup".to_string(),
                }),
            },
            message: MessageEnvelope {
                content: MessageContent::from_static("hello"),
                headers: MessageHeaders::new([("x-kind", "backup")]).unwrap(),
            },
        };

        let mut inner = v1::JobAdded::new();
        inner.set_job(v1::JobDetails::from(&details));
        let mut event = v1::JobEvent::new();
        event.set_job_added(inner);
        let encoded = JobEventCodec.encode(&event).unwrap();
        let decoded = JobEventCodec.decode(JOB_ADDED_EVENT_TYPE, "backup", &encoded).unwrap();

        assert_eq!(decoded, event);
        let v1::job_event::EventOneof::JobAdded(inner) = decoded.event() else {
            panic!("decoded event was not job_added");
        };
        assert_eq!(JobDetails::try_from(inner.job().to_owned()).unwrap(), details);
        assert!(matches!(
            JobEventCodec
                .decode(JOB_ADDED_EVENT_TYPE, "backup", &encoded)
                .unwrap()
                .event(),
            v1::job_event::EventOneof::JobAdded(_)
        ));
    }

    #[test]
    fn unknown_status_is_rejected() {
        let mut details = v1::JobDetails::new();
        details.set_status(v1::JobStatus::from(99));
        details.set_schedule(v1::JobSchedule::from(&JobEventSchedule::Every { every_sec: 30 }));
        details.set_delivery(v1::JobDelivery::from(&JobEventDelivery::NatsEvent {
            route: "ops.backup".to_string(),
            ttl_sec: None,
            source: None,
        }));
        details.set_message(v1::JobMessage::from(&MessageEnvelope {
            content: MessageContent::from_static("hello"),
            headers: MessageHeaders::default(),
        }));

        let error = JobDetails::try_from(details).unwrap_err();
        assert!(matches!(error, JobEventProtoError::UnknownJobStatus { value: 99 }));
    }
}
