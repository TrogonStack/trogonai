use buffa::Message as _;
use trogon_decider_runtime::{
    EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity, EventPayloadError, EventType,
    InvalidSnapshotTypeName, SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
    SnapshotTypeName,
};

use super::{ScheduleEventCase, state_v1, v1};
use crate::codec::{decode_event_case, event_type, snapshot_type as proto_snapshot_type};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct StateSnapshotPayloadError(#[source] buffa::DecodeError);

pub type ScheduleEventPayloadError = EventPayloadError<buffa::DecodeError>;

impl EventEncode for v1::ScheduleEvent {
    type Error = ScheduleEventPayloadError;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        self.event
            .as_ref()
            .map(encode_schedule_event_case)
            .ok_or(ScheduleEventPayloadError::MissingEvent)
    }
}

impl EventDecode for v1::ScheduleEvent {
    type Error = ScheduleEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        match decode_schedule_event_case(event)? {
            Some(event) => Ok(EventDecodeOutcome::Decoded(v1::ScheduleEvent { event: Some(event) })),
            None => Ok(EventDecodeOutcome::Skipped),
        }
    }
}

impl EventIdentity for v1::ScheduleEvent {}

impl EventType for v1::ScheduleEvent {
    type Error = ScheduleEventPayloadError;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        self.event
            .as_ref()
            .map(schedule_event_case_type)
            .ok_or(ScheduleEventPayloadError::MissingEvent)
    }
}

fn encode_schedule_event_case(event: &ScheduleEventCase) -> Vec<u8> {
    match event {
        ScheduleEventCase::ScheduleCreated(inner) => inner.encode_to_vec(),
        ScheduleEventCase::SchedulePaused(inner) => inner.encode_to_vec(),
        ScheduleEventCase::ScheduleResumed(inner) => inner.encode_to_vec(),
        ScheduleEventCase::ScheduleRemoved(inner) => inner.encode_to_vec(),
        ScheduleEventCase::ScheduleOccurrenceRecorded(inner) => inner.encode_to_vec(),
        ScheduleEventCase::ScheduleOccurrenceScheduled(inner) => inner.encode_to_vec(),
        ScheduleEventCase::ScheduleCompleted(inner) => inner.encode_to_vec(),
    }
}

fn decode_schedule_event_case(event: EventData<'_>) -> Result<Option<ScheduleEventCase>, ScheduleEventPayloadError> {
    let Some(event) = decode_event_case::<v1::ScheduleCreated, ScheduleEventCase>(&event)
        .or_else(|| decode_event_case::<v1::SchedulePaused, ScheduleEventCase>(&event))
        .or_else(|| decode_event_case::<v1::ScheduleResumed, ScheduleEventCase>(&event))
        .or_else(|| decode_event_case::<v1::ScheduleRemoved, ScheduleEventCase>(&event))
        .or_else(|| decode_event_case::<v1::ScheduleOccurrenceRecorded, ScheduleEventCase>(&event))
        .or_else(|| decode_event_case::<v1::ScheduleOccurrenceScheduled, ScheduleEventCase>(&event))
        .or_else(|| decode_event_case::<v1::ScheduleCompleted, ScheduleEventCase>(&event))
    else {
        return Ok(None);
    };

    event.map(Some).map_err(ScheduleEventPayloadError::Decode)
}

fn schedule_event_case_type(event: &ScheduleEventCase) -> &'static str {
    match event {
        ScheduleEventCase::ScheduleCreated(_) => event_type::<v1::ScheduleCreated>(),
        ScheduleEventCase::SchedulePaused(_) => event_type::<v1::SchedulePaused>(),
        ScheduleEventCase::ScheduleResumed(_) => event_type::<v1::ScheduleResumed>(),
        ScheduleEventCase::ScheduleRemoved(_) => event_type::<v1::ScheduleRemoved>(),
        ScheduleEventCase::ScheduleOccurrenceRecorded(_) => event_type::<v1::ScheduleOccurrenceRecorded>(),
        ScheduleEventCase::ScheduleOccurrenceScheduled(_) => event_type::<v1::ScheduleOccurrenceScheduled>(),
        ScheduleEventCase::ScheduleCompleted(_) => event_type::<v1::ScheduleCompleted>(),
    }
}

impl SnapshotType for state_v1::State {
    type Error = InvalidSnapshotTypeName;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        proto_snapshot_type::<Self>()
    }
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
mod tests;
