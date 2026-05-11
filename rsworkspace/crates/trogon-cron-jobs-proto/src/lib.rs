use buffa::Message as _;
use trogon_eventsourcing::{CanonicalEventCodec, EventCodec, EventIdentity, EventType, SnapshotSchema};

#[allow(clippy::all)]
#[path = "gen/mod.rs"]
pub mod generated;

pub use generated::trogon::cron::jobs::state::v1 as state_v1;
pub use generated::trogon::cron::jobs::v1;
pub use generated::trogon::cron::jobs::v1::__buffa::oneof::job_event::Event as JobEventCase;

// TODO: Replace these manual names with generated full names once Buffa exposes
// them without `ExtensionSet`/unknown-fields coupling.
// https://github.com/anthropics/buffa/pull/108
pub const JOB_ADDED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobAdded";
pub const JOB_PAUSED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobPaused";
pub const JOB_RESUMED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobResumed";
pub const JOB_REMOVED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobRemoved";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobEventCodec;

#[derive(Debug)]
pub enum JobEventCodecError {
    Decode(buffa::DecodeError),
    MissingEvent,
    UnknownEventType { value: String },
}

impl std::fmt::Display for JobEventCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(source) => write!(f, "{source}"),
            Self::MissingEvent => f.write_str("protobuf job event is missing its oneof case"),
            Self::UnknownEventType { value } => write!(f, "unknown protobuf job event type '{value}'"),
        }
    }
}

impl std::error::Error for JobEventCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(source) => Some(source),
            Self::MissingEvent | Self::UnknownEventType { .. } => None,
        }
    }
}

impl EventCodec<v1::JobEvent> for JobEventCodec {
    type Error = JobEventCodecError;

    fn encode(&self, value: &v1::JobEvent) -> Result<Vec<u8>, Self::Error> {
        match &value.event {
            Some(JobEventCase::JobAdded(inner)) => Ok(inner.encode_to_vec()),
            Some(JobEventCase::JobPaused(inner)) => Ok(inner.encode_to_vec()),
            Some(JobEventCase::JobResumed(inner)) => Ok(inner.encode_to_vec()),
            Some(JobEventCase::JobRemoved(inner)) => Ok(inner.encode_to_vec()),
            None => Err(JobEventCodecError::MissingEvent),
        }
    }

    fn decode(&self, event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<v1::JobEvent, Self::Error> {
        let event = match event_type {
            JOB_ADDED_EVENT_TYPE => v1::JobAdded::decode_from_slice(payload).map(|event| v1::JobEvent {
                event: Some(event.into()),
            }),
            JOB_PAUSED_EVENT_TYPE => v1::JobPaused::decode_from_slice(payload).map(|event| v1::JobEvent {
                event: Some(event.into()),
            }),
            JOB_RESUMED_EVENT_TYPE => v1::JobResumed::decode_from_slice(payload).map(|event| v1::JobEvent {
                event: Some(event.into()),
            }),
            JOB_REMOVED_EVENT_TYPE => v1::JobRemoved::decode_from_slice(payload).map(|event| v1::JobEvent {
                event: Some(event.into()),
            }),
            value => {
                return Err(JobEventCodecError::UnknownEventType {
                    value: value.to_string(),
                });
            }
        };
        event.map_err(JobEventCodecError::Decode)
    }
}

impl EventIdentity for v1::JobEvent {}

impl EventType for v1::JobEvent {
    type Error = JobEventCodecError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        match &self.event {
            Some(JobEventCase::JobAdded(_)) => Ok(JOB_ADDED_EVENT_TYPE),
            Some(JobEventCase::JobPaused(_)) => Ok(JOB_PAUSED_EVENT_TYPE),
            Some(JobEventCase::JobResumed(_)) => Ok(JOB_RESUMED_EVENT_TYPE),
            Some(JobEventCase::JobRemoved(_)) => Ok(JOB_REMOVED_EVENT_TYPE),
            None => Err(JobEventCodecError::MissingEvent),
        }
    }
}

impl CanonicalEventCodec for v1::JobEvent {
    type Codec = JobEventCodec;

    fn canonical_codec() -> Self::Codec {
        JobEventCodec
    }
}

impl SnapshotSchema for state_v1::State {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.snapshots.jobs.v1.";
}

#[cfg(test)]
mod tests {
    use buffa::{Message as _, MessageField};
    use trogon_eventsourcing::{EventCodec, EventData};

    use super::*;

    fn job_added_event(every_sec: u64) -> v1::JobEvent {
        v1::JobEvent {
            event: Some(
                v1::JobAdded {
                    job: MessageField::some(job_details(every_sec)),
                }
                .into(),
            ),
        }
    }

    fn job_details(every_sec: u64) -> v1::JobDetails {
        v1::JobDetails {
            status: v1::JobStatus::JOB_STATUS_ENABLED,
            schedule: MessageField::some(v1::JobSchedule {
                kind: Some(v1::EverySchedule { every_sec }.into()),
            }),
            delivery: MessageField::some(v1::JobDelivery {
                kind: Some(
                    v1::NatsEventDelivery {
                        route: "cron.jobs.backup".to_string(),
                        ttl_sec: None,
                        source: MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: MessageField::some(v1::JobMessage {
                content: r#"{"job":"backup"}"#.to_string(),
                headers: vec![v1::Header {
                    name: "content-type".to_string(),
                    value: "application/json".to_string(),
                }],
            }),
        }
    }

    #[test]
    fn job_event_partial_eq_compares_fields() {
        assert_eq!(job_added_event(30), job_added_event(30));
    }

    #[test]
    fn job_event_partial_eq_detects_nested_differences() {
        assert_ne!(job_added_event(30), job_added_event(60));
    }

    #[test]
    fn job_details_partial_eq_compares_fields() {
        assert_eq!(job_details(30), job_details(30));
        assert_ne!(job_details(30), job_details(60));
    }

    #[test]
    fn job_event_partial_eq_handles_empty_event_variants() {
        let left = v1::JobEvent {
            event: Some(v1::JobPaused {}.into()),
        };
        let right = v1::JobEvent {
            event: Some(v1::JobPaused {}.into()),
        };
        let different = v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        };

        assert_eq!(left, right);
        assert_ne!(left, different);
    }

    #[test]
    fn event_data_and_recorded_event_helpers_work() {
        let removed = v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        };
        let event = EventData::from_event("cleanup", &JobEventCodec, &removed).unwrap();
        assert_eq!(event.stream_id(), "cleanup");
        assert_eq!(event.event_type, JOB_REMOVED_EVENT_TYPE);
        assert!(v1::JobRemoved::decode_from_slice(&event.payload).is_ok());
        assert_eq!(
            event.subject_with_prefix("cron.jobs.events."),
            "cron.jobs.events.cleanup"
        );

        assert_eq!(event.decode_data_with(&JobEventCodec).unwrap(), removed);

        let recorded = event.record(
            None,
            chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        );
        assert_eq!(recorded.stream_id(), "cleanup");
        assert_eq!(
            recorded.subject_with_prefix("cron.jobs.events."),
            "cron.jobs.events.cleanup"
        );
        let expected = v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        };
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
        let event = job_added_event(30);

        let encoded = JobEventCodec.encode(&event).unwrap();
        let decoded = JobEventCodec.decode(JOB_ADDED_EVENT_TYPE, "backup", &encoded).unwrap();

        assert_eq!(decoded, event);
        assert!(matches!(decoded.event, Some(JobEventCase::JobAdded(_))));
    }

    #[test]
    fn state_snapshot_serializes_as_message_shape() {
        let state = state_v1::State {
            state: Some(buffa::EnumValue::from(
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED,
            )),
        };

        let json = serde_json::to_string(&state).unwrap();

        assert_eq!(json, r#"{"state":"STATE_VALUE_PRESENT_ENABLED"}"#);
    }

    #[test]
    fn state_snapshot_deserializes_message_shape() {
        let state: state_v1::State = serde_json::from_str(r#"{"state":"STATE_VALUE_PRESENT_DISABLED"}"#).unwrap();

        assert_eq!(
            state.state.and_then(|value| value.as_known()),
            Some(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)
        );
    }
}
