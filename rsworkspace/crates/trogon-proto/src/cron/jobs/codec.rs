use buffa::Message as _;
use trogon_decider_runtime::{
    EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity, EventPayloadError, EventType,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
};

use super::{JobEventCase, state_v1, v1};
use crate::codec::{decode_event_case, event_type};

#[derive(Debug)]
pub struct StateSnapshotPayloadError(buffa::DecodeError);

pub type JobEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl std::fmt::Display for StateSnapshotPayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for StateSnapshotPayloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl EventEncode for v1::JobEvent {
    type Error = JobEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        encode_job_event_payload(self)
    }
}

impl EventDecode for v1::JobEvent {
    type Error = JobEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        decode_job_event_payload(event)
    }
}

impl EventIdentity for v1::JobEvent {}

impl EventType for v1::JobEvent {
    type Error = JobEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        job_event_type(self)
    }
}

fn encode_job_event_payload(event: &v1::JobEvent) -> Result<Vec<u8>, JobEventPayloadError> {
    event
        .event
        .as_ref()
        .map(encode_job_event_case)
        .ok_or(JobEventPayloadError::MissingEvent)
}

fn decode_job_event_payload(event: EventData<'_>) -> Result<EventDecodeOutcome<v1::JobEvent>, JobEventPayloadError> {
    match decode_job_event_case(event)? {
        Some(event) => Ok(EventDecodeOutcome::Decoded(v1::JobEvent { event: Some(event) })),
        None => Ok(EventDecodeOutcome::Skipped),
    }
}

fn job_event_type(event: &v1::JobEvent) -> Result<&'static str, JobEventPayloadError> {
    event
        .event
        .as_ref()
        .map(job_event_case_type)
        .ok_or(JobEventPayloadError::MissingEvent)
}

fn encode_job_event_case(event: &JobEventCase) -> Vec<u8> {
    match event {
        JobEventCase::JobAdded(inner) => inner.encode_to_vec(),
        JobEventCase::JobPaused(inner) => inner.encode_to_vec(),
        JobEventCase::JobResumed(inner) => inner.encode_to_vec(),
        JobEventCase::JobRemoved(inner) => inner.encode_to_vec(),
    }
}

fn decode_job_event_case(event: EventData<'_>) -> Result<Option<JobEventCase>, JobEventPayloadError> {
    let Some(event) = decode_event_case::<v1::JobAdded, JobEventCase>(&event)
        .or_else(|| decode_event_case::<v1::JobPaused, JobEventCase>(&event))
        .or_else(|| decode_event_case::<v1::JobResumed, JobEventCase>(&event))
        .or_else(|| decode_event_case::<v1::JobRemoved, JobEventCase>(&event))
    else {
        return Ok(None);
    };

    event.map(Some).map_err(JobEventPayloadError::Decode)
}

fn job_event_case_type(event: &JobEventCase) -> &'static str {
    match event {
        JobEventCase::JobAdded(_) => event_type::<v1::JobAdded>(),
        JobEventCase::JobPaused(_) => event_type::<v1::JobPaused>(),
        JobEventCase::JobResumed(_) => event_type::<v1::JobResumed>(),
        JobEventCase::JobRemoved(_) => event_type::<v1::JobRemoved>(),
    }
}

impl SnapshotType for state_v1::State {
    const SNAPSHOT_STREAM_PREFIX: &'static str = "cron.command.snapshots.jobs.v1.";
}

impl SnapshotPayloadEncode for state_v1::State {
    type Error = std::convert::Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(self.encode_to_vec())
    }
}

impl SnapshotPayloadDecode for state_v1::State {
    type Error = StateSnapshotPayloadError;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        Self::decode_from_slice(payload.payload).map_err(StateSnapshotPayloadError)
    }
}

#[cfg(test)]
mod tests {
    use buffa::{Message as _, MessageField};
    use trogon_decider_runtime::{
        EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, SnapshotPayloadData, SnapshotPayloadDecode,
        SnapshotPayloadEncode,
    };

    use super::*;

    fn job_added() -> v1::JobAdded {
        v1::JobAdded {
            job: MessageField::some(v1::JobDetails {
                status: v1::JobStatus::JOB_STATUS_ENABLED,
                schedule: MessageField::some(v1::JobSchedule {
                    kind: Some(v1::EverySchedule { every_sec: 30 }.into()),
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
                    headers: Vec::new(),
                }),
            }),
        }
    }

    #[test]
    fn event_encode_writes_inner_event_payload() {
        let inner = job_added();
        let event = v1::JobEvent {
            event: Some(inner.clone().into()),
        };

        let encoded = EventEncode::encode(&event).unwrap();

        assert_eq!(v1::JobAdded::decode_from_slice(&encoded).unwrap(), inner);
    }

    #[test]
    fn event_encode_rejects_missing_event_case() {
        let event = v1::JobEvent { event: None };

        assert!(matches!(
            EventEncode::encode(&event),
            Err(JobEventPayloadError::MissingEvent)
        ));
    }

    #[test]
    fn event_decode_dispatches_by_generated_full_name() {
        let inner = job_added();
        let encoded = inner.encode_to_vec();

        let decoded = <v1::JobEvent as EventDecode>::decode(EventData::new(
            <v1::JobAdded as buffa::MessageName>::FULL_NAME,
            &encoded,
        ))
        .unwrap();

        let decoded = decoded.into_decoded().unwrap();
        assert!(matches!(decoded.event, Some(JobEventCase::JobAdded(_))));
    }

    #[test]
    fn event_decode_skips_unknown_event_type() {
        assert!(matches!(
            <v1::JobEvent as EventDecode>::decode(EventData::new("trogon.cron.jobs.v1.Unknown", &[])),
            Ok(EventDecodeOutcome::Skipped)
        ));
    }

    #[test]
    fn event_decode_preserves_payload_decode_errors() {
        assert!(matches!(
            <v1::JobEvent as EventDecode>::decode(EventData::new(
                <v1::JobRemoved as buffa::MessageName>::FULL_NAME,
                b"\0"
            )),
            Err(JobEventPayloadError::Decode(_))
        ));
    }

    #[test]
    fn event_type_returns_inner_event_full_name() {
        let event = v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        };

        assert_eq!(
            event.event_type().unwrap(),
            <v1::JobRemoved as buffa::MessageName>::FULL_NAME
        );
    }

    #[test]
    fn event_type_rejects_missing_event_case() {
        let event = v1::JobEvent { event: None };

        assert!(matches!(event.event_type(), Err(JobEventPayloadError::MissingEvent)));
    }

    #[test]
    fn state_snapshot_round_trips_through_snapshot_traits() {
        let state = state_v1::State {
            state: Some(buffa::EnumValue::from(
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED,
            )),
        };

        let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
        let decoded = <state_v1::State as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded)).unwrap();

        assert_eq!(decoded, state);
    }

    #[test]
    fn state_snapshot_decode_preserves_payload_decode_errors() {
        assert!(<state_v1::State as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(b"\0")).is_err());
    }
}
