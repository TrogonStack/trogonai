use buffa::Message as _;
use trogon_eventsourcing::{EventDecode, EventEncode, EventIdentity, EventType, SnapshotSchema};

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

#[derive(Debug)]
pub enum JobEventPayloadError {
    Decode(buffa::DecodeError),
    MissingEvent,
    UnknownEventType { value: String },
}

impl std::fmt::Display for JobEventPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(source) => write!(f, "{source}"),
            Self::MissingEvent => f.write_str("protobuf job event is missing its oneof case"),
            Self::UnknownEventType { value } => write!(f, "unknown protobuf job event type '{value}'"),
        }
    }
}

impl std::error::Error for JobEventPayloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(source) => Some(source),
            Self::MissingEvent | Self::UnknownEventType { .. } => None,
        }
    }
}

impl EventEncode for v1::JobEvent {
    type Error = JobEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        match &self.event {
            Some(JobEventCase::JobAdded(inner)) => Ok(inner.encode_to_vec()),
            Some(JobEventCase::JobPaused(inner)) => Ok(inner.encode_to_vec()),
            Some(JobEventCase::JobResumed(inner)) => Ok(inner.encode_to_vec()),
            Some(JobEventCase::JobRemoved(inner)) => Ok(inner.encode_to_vec()),
            None => Err(JobEventPayloadError::MissingEvent),
        }
    }
}

impl EventDecode for v1::JobEvent {
    type Error = JobEventPayloadError;

    fn decode(event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<Self, Self::Error> {
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
                return Err(JobEventPayloadError::UnknownEventType {
                    value: value.to_string(),
                });
            }
        };
        event.map_err(JobEventPayloadError::Decode)
    }
}

impl EventIdentity for v1::JobEvent {}

impl EventType for v1::JobEvent {
    type Error = JobEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        match &self.event {
            Some(JobEventCase::JobAdded(_)) => Ok(JOB_ADDED_EVENT_TYPE),
            Some(JobEventCase::JobPaused(_)) => Ok(JOB_PAUSED_EVENT_TYPE),
            Some(JobEventCase::JobResumed(_)) => Ok(JOB_RESUMED_EVENT_TYPE),
            Some(JobEventCase::JobRemoved(_)) => Ok(JOB_REMOVED_EVENT_TYPE),
            None => Err(JobEventPayloadError::MissingEvent),
        }
    }
}

impl SnapshotSchema for state_v1::State {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.snapshots.jobs.v1.";
}

#[cfg(test)]
mod tests {
    use buffa::{Message as _, MessageField};
    use trogon_eventsourcing::{Event, EventDecode, EventEncode, StreamPosition};

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
    fn event_and_recorded_event_helpers_work() {
        let removed = v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        };
        let event = Event::from_domain_event(&removed).unwrap();
        assert_eq!(event.r#type, JOB_REMOVED_EVENT_TYPE);
        assert!(v1::JobRemoved::decode_from_slice(&event.content).is_ok());

        assert_eq!(event.decode::<v1::JobEvent>("cleanup").unwrap(), removed);

        let recorded = event.record(
            "cleanup",
            StreamPosition::try_new(1).unwrap(),
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
        assert_eq!(recorded.decode::<v1::JobEvent>().unwrap(), expected);
    }

    #[test]
    fn invalid_payload_fails_decode() {
        assert!(<v1::JobEvent as EventDecode>::decode(JOB_REMOVED_EVENT_TYPE, "cleanup", b"\0").is_err());
        assert!(<v1::JobEvent as EventDecode>::decode("trogon.cron.jobs.v1.Unknown", "cleanup", &[]).is_err());
    }

    #[test]
    fn job_added_round_trips_through_contract() {
        let event = job_added_event(30);

        let encoded = EventEncode::encode(&event).unwrap();
        let decoded = <v1::JobEvent as EventDecode>::decode(JOB_ADDED_EVENT_TYPE, "backup", &encoded).unwrap();

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
