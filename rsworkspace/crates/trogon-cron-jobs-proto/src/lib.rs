#![allow(clippy::all)]

use protobuf::Parse as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer, ser::SerializeStruct};
use trogon_eventsourcing::{CanonicalEventCodec, EventCodec, EventEnvelopeCodec, SnapshotSchema};

pub mod trogon {
    pub mod cron {
        pub mod jobs {
            pub mod v1 {
                include!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/gen/trogon/cron/jobs/v1/generated.rs"
                ));
            }

            pub mod state {
                pub mod v1 {
                    include!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/src/gen/trogon/cron/jobs/state/v1/generated.rs"
                    ));
                }
            }
        }
    }
}

pub use trogon::cron::jobs::state::v1 as state_v1;
pub use trogon::cron::jobs::v1;

// TODO: Replace these hardcoded full names once protobuf Rust exposes generated
// message full-name metadata: https://github.com/protocolbuffers/protobuf/pull/27111
pub const JOB_ADDED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobAdded";
pub const JOB_PAUSED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobPaused";
pub const JOB_RESUMED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobResumed";
pub const JOB_REMOVED_EVENT_TYPE: &str = "trogon.cron.jobs.v1.JobRemoved";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobEventCodec;

#[derive(Debug)]
pub enum JobEventCodecError {
    Decode(protobuf::ParseError),
    Encode(protobuf::SerializeError),
    MissingEvent,
    UnknownEventType { value: String },
}

impl std::fmt::Display for JobEventCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Decode(source) => write!(f, "{source}"),
            Self::Encode(source) => write!(f, "{source}"),
            Self::MissingEvent => f.write_str("protobuf job event is missing its oneof case"),
            Self::UnknownEventType { value } => write!(f, "unknown protobuf job event type '{value}'"),
        }
    }
}

impl std::error::Error for JobEventCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(source) => Some(source),
            Self::Encode(source) => Some(source),
            Self::MissingEvent | Self::UnknownEventType { .. } => None,
        }
    }
}

impl EventCodec<v1::JobEvent> for JobEventCodec {
    type Error = JobEventCodecError;

    fn encode(&self, value: &v1::JobEvent) -> Result<Vec<u8>, Self::Error> {
        encode_job_event(value)
    }

    fn decode(&self, event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<v1::JobEvent, Self::Error> {
        decode_job_event(event_type, payload)
    }
}

impl EventEnvelopeCodec<v1::JobEvent> for JobEventCodec {
    fn event_type(&self, value: &v1::JobEvent) -> Result<&'static str, Self::Error> {
        job_event_type(value)
    }
}

impl CanonicalEventCodec for v1::JobEvent {
    type Codec = JobEventCodec;

    fn canonical_codec() -> Self::Codec {
        JobEventCodec
    }
}

fn job_event_type(event: &v1::JobEvent) -> Result<&'static str, JobEventCodecError> {
    match event.event() {
        v1::job_event::EventOneof::JobAdded(_) => Ok(JOB_ADDED_EVENT_TYPE),
        v1::job_event::EventOneof::JobPaused(_) => Ok(JOB_PAUSED_EVENT_TYPE),
        v1::job_event::EventOneof::JobResumed(_) => Ok(JOB_RESUMED_EVENT_TYPE),
        v1::job_event::EventOneof::JobRemoved(_) => Ok(JOB_REMOVED_EVENT_TYPE),
        v1::job_event::EventOneof::not_set(_) => Err(JobEventCodecError::MissingEvent),
    }
}

fn encode_job_event(event: &v1::JobEvent) -> Result<Vec<u8>, JobEventCodecError> {
    match event.event() {
        v1::job_event::EventOneof::JobAdded(inner) => {
            protobuf::Serialize::serialize(&inner).map_err(JobEventCodecError::Encode)
        }
        v1::job_event::EventOneof::JobPaused(inner) => {
            protobuf::Serialize::serialize(&inner).map_err(JobEventCodecError::Encode)
        }
        v1::job_event::EventOneof::JobResumed(inner) => {
            protobuf::Serialize::serialize(&inner).map_err(JobEventCodecError::Encode)
        }
        v1::job_event::EventOneof::JobRemoved(inner) => {
            protobuf::Serialize::serialize(&inner).map_err(JobEventCodecError::Encode)
        }
        v1::job_event::EventOneof::not_set(_) => Err(JobEventCodecError::MissingEvent),
    }
}

fn decode_job_event(event_type: &str, payload: &[u8]) -> Result<v1::JobEvent, JobEventCodecError> {
    let mut event = v1::JobEvent::new();
    match event_type {
        JOB_ADDED_EVENT_TYPE => {
            event.set_job_added(v1::JobAdded::parse(payload).map_err(JobEventCodecError::Decode)?);
        }
        JOB_PAUSED_EVENT_TYPE => {
            event.set_job_paused(v1::JobPaused::parse(payload).map_err(JobEventCodecError::Decode)?);
        }
        JOB_RESUMED_EVENT_TYPE => {
            event.set_job_resumed(v1::JobResumed::parse(payload).map_err(JobEventCodecError::Decode)?);
        }
        JOB_REMOVED_EVENT_TYPE => {
            event.set_job_removed(v1::JobRemoved::parse(payload).map_err(JobEventCodecError::Decode)?);
        }
        value => {
            return Err(JobEventCodecError::UnknownEventType {
                value: value.to_string(),
            });
        }
    }
    Ok(event)
}

impl PartialEq for v1::JobEvent {
    fn eq(&self, other: &Self) -> bool {
        match (
            protobuf::Serialize::serialize(self),
            protobuf::Serialize::serialize(other),
        ) {
            (Ok(left), Ok(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for v1::JobEvent {}

impl SnapshotSchema for state_v1::State {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.job.v3.";
}

impl Serialize for state_v1::State {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("State", 1)?;
        state.serialize_field("state", &i32::from(self.state()))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for state_v1::State {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct SnapshotState {
            state: i32,
        }

        let value = SnapshotState::deserialize(deserializer)?.state;
        let mut state = state_v1::State::new();
        state.set_state(state_v1::StateValue::from(value));
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_snapshot_serializes_as_message_shape() {
        let mut state = state_v1::State::new();
        state.set_state(state_v1::StateValue::PresentEnabled);

        let json = serde_json::to_string(&state).unwrap();

        assert_eq!(json, r#"{"state":2}"#);
    }

    #[test]
    fn state_snapshot_deserializes_message_shape() {
        let state: state_v1::State = serde_json::from_str(r#"{"state":3}"#).unwrap();

        assert_eq!(state.state(), state_v1::StateValue::PresentDisabled);
    }
}
