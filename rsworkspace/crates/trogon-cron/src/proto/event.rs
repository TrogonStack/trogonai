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
    UnknownJobStatus {
        value: i32,
    },
    InvalidTimestamp {
        value: String,
        source: chrono::ParseError,
    },
    InvalidHeaders {
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl JobEventProtoError {
    pub fn invalid_headers(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::InvalidHeaders {
            source: Box::new(source),
        }
    }
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
            Self::InvalidHeaders { source } => write!(f, "protobuf headers are invalid: {source}"),
        }
    }
}

impl std::error::Error for JobEventProtoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidTimestamp { source, .. } => Some(source),
            Self::InvalidHeaders { source } => Some(source.as_ref()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_model::{
        JobDetails, JobEventDelivery, JobEventSamplingSource, JobEventSchedule, JobEventStatus, MessageContent,
        MessageEnvelope, MessageHeaders,
    };
    use protobuf::Parse as _;
    use trogon_cron_jobs_proto::v1;
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
}
