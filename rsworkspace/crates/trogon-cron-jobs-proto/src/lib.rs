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
        job_event_eq(protobuf::AsView::as_view(self), protobuf::AsView::as_view(other))
    }
}

impl Eq for v1::JobEvent {}

fn job_event_eq(left: v1::JobEventView<'_>, right: v1::JobEventView<'_>) -> bool {
    match (left.event(), right.event()) {
        (v1::job_event::EventOneof::JobAdded(left), v1::job_event::EventOneof::JobAdded(right)) => {
            job_added_eq(left, right)
        }
        (v1::job_event::EventOneof::JobPaused(_), v1::job_event::EventOneof::JobPaused(_))
        | (v1::job_event::EventOneof::JobResumed(_), v1::job_event::EventOneof::JobResumed(_))
        | (v1::job_event::EventOneof::JobRemoved(_), v1::job_event::EventOneof::JobRemoved(_))
        | (v1::job_event::EventOneof::not_set(_), v1::job_event::EventOneof::not_set(_)) => true,
        _ => false,
    }
}

fn job_added_eq(left: v1::JobAddedView<'_>, right: v1::JobAddedView<'_>) -> bool {
    left.has_job() == right.has_job() && job_details_eq(left.job(), right.job())
}

fn job_details_eq(left: v1::JobDetailsView<'_>, right: v1::JobDetailsView<'_>) -> bool {
    left.has_status() == right.has_status()
        && left.status() == right.status()
        && left.has_schedule() == right.has_schedule()
        && job_schedule_eq(left.schedule(), right.schedule())
        && left.has_delivery() == right.has_delivery()
        && job_delivery_eq(left.delivery(), right.delivery())
        && left.has_message() == right.has_message()
        && job_message_eq(left.message(), right.message())
}

fn job_schedule_eq(left: v1::JobScheduleView<'_>, right: v1::JobScheduleView<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (v1::job_schedule::KindOneof::At(left), v1::job_schedule::KindOneof::At(right)) => {
            left.has_at() == right.has_at() && left.at().to_string() == right.at().to_string()
        }
        (v1::job_schedule::KindOneof::Every(left), v1::job_schedule::KindOneof::Every(right)) => {
            left.has_every_sec() == right.has_every_sec() && left.every_sec() == right.every_sec()
        }
        (v1::job_schedule::KindOneof::Cron(left), v1::job_schedule::KindOneof::Cron(right)) => {
            left.has_expr() == right.has_expr()
                && left.expr().to_string() == right.expr().to_string()
                && left.has_timezone() == right.has_timezone()
                && left.timezone().to_string() == right.timezone().to_string()
        }
        (v1::job_schedule::KindOneof::not_set(_), v1::job_schedule::KindOneof::not_set(_)) => true,
        _ => false,
    }
}

fn job_delivery_eq(left: v1::JobDeliveryView<'_>, right: v1::JobDeliveryView<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (v1::job_delivery::KindOneof::NatsEvent(left), v1::job_delivery::KindOneof::NatsEvent(right)) => {
            left.has_route() == right.has_route()
                && left.route().to_string() == right.route().to_string()
                && left.has_ttl_sec() == right.has_ttl_sec()
                && left.ttl_sec() == right.ttl_sec()
                && left.has_source() == right.has_source()
                && job_sampling_source_eq(left.source(), right.source())
        }
        (v1::job_delivery::KindOneof::not_set(_), v1::job_delivery::KindOneof::not_set(_)) => true,
        _ => false,
    }
}

fn job_sampling_source_eq(left: v1::JobSamplingSourceView<'_>, right: v1::JobSamplingSourceView<'_>) -> bool {
    match (left.kind(), right.kind()) {
        (
            v1::job_sampling_source::KindOneof::LatestFromSubject(left),
            v1::job_sampling_source::KindOneof::LatestFromSubject(right),
        ) => left.has_subject() == right.has_subject() && left.subject().to_string() == right.subject().to_string(),
        (v1::job_sampling_source::KindOneof::not_set(_), v1::job_sampling_source::KindOneof::not_set(_)) => true,
        _ => false,
    }
}

fn job_message_eq(left: v1::JobMessageView<'_>, right: v1::JobMessageView<'_>) -> bool {
    left.has_content() == right.has_content()
        && left.content().to_string() == right.content().to_string()
        && left.headers().len() == right.headers().len()
        && left.headers().iter().zip(right.headers().iter()).all(|(left, right)| {
            left.has_name() == right.has_name()
                && left.name().to_string() == right.name().to_string()
                && left.has_value() == right.has_value()
                && left.value().to_string() == right.value().to_string()
        })
}

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

    fn job_added_event(every_sec: u64) -> v1::JobEvent {
        let mut event = v1::JobEvent::new();
        let mut added = v1::JobAdded::new();
        added.set_job(job_details(every_sec));
        event.set_job_added(added);
        event
    }

    fn job_details(every_sec: u64) -> v1::JobDetails {
        let mut details = v1::JobDetails::new();
        details.set_status(v1::JobStatus::Enabled);
        details.set_schedule(every_schedule(every_sec));
        details.set_delivery(nats_delivery());
        details.set_message(message());
        details
    }

    fn every_schedule(every_sec: u64) -> v1::JobSchedule {
        let mut schedule = v1::JobSchedule::new();
        let mut every = v1::EverySchedule::new();
        every.set_every_sec(every_sec);
        schedule.set_every(every);
        schedule
    }

    fn nats_delivery() -> v1::JobDelivery {
        let mut delivery = v1::JobDelivery::new();
        let mut nats = v1::NatsEventDelivery::new();
        nats.set_route("cron.jobs.backup");
        delivery.set_nats_event(nats);
        delivery
    }

    fn message() -> v1::JobMessage {
        let mut message = v1::JobMessage::new();
        message.set_content(r#"{"job":"backup"}"#);
        let mut header = v1::Header::new();
        header.set_name("content-type");
        header.set_value("application/json");
        message.headers_mut().push(header);
        message
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
    fn job_event_partial_eq_handles_empty_event_variants() {
        let mut left = v1::JobEvent::new();
        left.set_job_paused(v1::JobPaused::new());
        let mut right = v1::JobEvent::new();
        right.set_job_paused(v1::JobPaused::new());
        let mut different = v1::JobEvent::new();
        different.set_job_removed(v1::JobRemoved::new());

        assert_eq!(left, right);
        assert_ne!(left, different);
    }

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
