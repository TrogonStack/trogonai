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
mod tests {
    use buffa::{Message as _, MessageField};
    use trogon_decider_runtime::{
        EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType, SnapshotPayloadData, SnapshotPayloadDecode,
        SnapshotPayloadEncode,
    };

    use super::*;

    fn schedule_created() -> v1::ScheduleCreated {
        v1::ScheduleCreated {
            schedule_id: "backup".to_string(),
            status: MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: MessageField::some(v1::Schedule {
                kind: Some(
                    v1::schedule::Every {
                        every: MessageField::some(buffa_types::google::protobuf::Duration {
                            seconds: 30,
                            nanos: 0,
                            ..buffa_types::google::protobuf::Duration::default()
                        }),
                    }
                    .into(),
                ),
            }),
            delivery: MessageField::some(v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: "scheduler.schedules.backup".to_string(),
                        ttl: MessageField::none(),
                        source: MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: MessageField::some(v1::Message {
                content: MessageField::some(crate::content::v1alpha1::Content {
                    content_type: "application/json".to_string(),
                    data: br#"{"job":"backup"}"#.to_vec(),
                }),
                headers: Vec::new(),
            }),
        }
    }

    fn timestamp() -> buffa_types::google::protobuf::Timestamp {
        buffa_types::google::protobuf::Timestamp::from_unix(1_451_600_400, 0)
    }

    #[test]
    fn event_encode_writes_inner_event_payload() {
        let inner = schedule_created();
        let event = v1::ScheduleEvent {
            event: Some(inner.clone().into()),
        };

        let encoded = EventEncode::encode(&event).unwrap();

        assert_eq!(v1::ScheduleCreated::decode_from_slice(&encoded).unwrap(), inner);
    }

    #[test]
    fn event_encode_rejects_missing_event_case() {
        let event = v1::ScheduleEvent { event: None };

        assert!(matches!(
            EventEncode::encode(&event),
            Err(ScheduleEventPayloadError::MissingEvent)
        ));
    }

    #[test]
    fn event_encode_writes_all_lifecycle_event_payloads() {
        let paused = v1::SchedulePaused {
            schedule_id: "backup".to_string(),
        };
        let resumed = v1::ScheduleResumed {
            schedule_id: "backup".to_string(),
        };
        let removed = v1::ScheduleRemoved {
            schedule_id: "backup".to_string(),
        };
        let occurrence_recorded = v1::ScheduleOccurrenceRecorded {
            schedule_id: "backup".to_string(),
            occurrence_sequence: Some(1),
            occurrence_at: MessageField::some(timestamp()),
            recorded_at: MessageField::some(timestamp()),
        };
        let occurrence_scheduled = v1::ScheduleOccurrenceScheduled {
            schedule_id: "backup".to_string(),
            occurrence_sequence: Some(2),
            occurrence_at: MessageField::some(timestamp()),
            scheduled_at: MessageField::some(timestamp()),
        };
        let completed = v1::ScheduleCompleted {
            schedule_id: "backup".to_string(),
            last_occurrence_sequence: Some(2),
        };

        let encoded = EventEncode::encode(&v1::ScheduleEvent {
            event: Some(paused.clone().into()),
        })
        .unwrap();
        assert_eq!(v1::SchedulePaused::decode_from_slice(&encoded).unwrap(), paused);

        let encoded = EventEncode::encode(&v1::ScheduleEvent {
            event: Some(resumed.clone().into()),
        })
        .unwrap();
        assert_eq!(v1::ScheduleResumed::decode_from_slice(&encoded).unwrap(), resumed);

        let encoded = EventEncode::encode(&v1::ScheduleEvent {
            event: Some(removed.clone().into()),
        })
        .unwrap();
        assert_eq!(v1::ScheduleRemoved::decode_from_slice(&encoded).unwrap(), removed);

        let encoded = EventEncode::encode(&v1::ScheduleEvent {
            event: Some(occurrence_recorded.clone().into()),
        })
        .unwrap();
        assert_eq!(
            v1::ScheduleOccurrenceRecorded::decode_from_slice(&encoded).unwrap(),
            occurrence_recorded
        );

        let encoded = EventEncode::encode(&v1::ScheduleEvent {
            event: Some(occurrence_scheduled.clone().into()),
        })
        .unwrap();
        assert_eq!(
            v1::ScheduleOccurrenceScheduled::decode_from_slice(&encoded).unwrap(),
            occurrence_scheduled
        );

        let encoded = EventEncode::encode(&v1::ScheduleEvent {
            event: Some(completed.clone().into()),
        })
        .unwrap();
        assert_eq!(v1::ScheduleCompleted::decode_from_slice(&encoded).unwrap(), completed);
    }

    #[test]
    fn event_decode_dispatches_by_generated_full_name() {
        let inner = schedule_created();
        let encoded = inner.encode_to_vec();

        let decoded = <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
            <v1::ScheduleCreated as buffa::MessageName>::FULL_NAME,
            &encoded,
        ))
        .unwrap();

        let decoded = decoded.into_decoded().unwrap();
        assert!(matches!(decoded.event, Some(ScheduleEventCase::ScheduleCreated(_))));
    }

    #[test]
    fn event_decode_dispatches_all_lifecycle_event_types() {
        let paused = v1::SchedulePaused {
            schedule_id: "backup".to_string(),
        };
        let resumed = v1::ScheduleResumed {
            schedule_id: "backup".to_string(),
        };
        let removed = v1::ScheduleRemoved {
            schedule_id: "backup".to_string(),
        };
        let occurrence_recorded = v1::ScheduleOccurrenceRecorded {
            schedule_id: "backup".to_string(),
            occurrence_sequence: Some(1),
            occurrence_at: MessageField::some(timestamp()),
            recorded_at: MessageField::some(timestamp()),
        };
        let occurrence_scheduled = v1::ScheduleOccurrenceScheduled {
            schedule_id: "backup".to_string(),
            occurrence_sequence: Some(2),
            occurrence_at: MessageField::some(timestamp()),
            scheduled_at: MessageField::some(timestamp()),
        };
        let completed = v1::ScheduleCompleted {
            schedule_id: "backup".to_string(),
            last_occurrence_sequence: Some(2),
        };

        let decoded = <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
            <v1::SchedulePaused as buffa::MessageName>::FULL_NAME,
            &paused.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(decoded.event, Some(ScheduleEventCase::SchedulePaused(_))));

        let decoded = <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
            <v1::ScheduleResumed as buffa::MessageName>::FULL_NAME,
            &resumed.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(decoded.event, Some(ScheduleEventCase::ScheduleResumed(_))));

        let decoded = <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
            <v1::ScheduleRemoved as buffa::MessageName>::FULL_NAME,
            &removed.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(decoded.event, Some(ScheduleEventCase::ScheduleRemoved(_))));

        let decoded = <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
            <v1::ScheduleOccurrenceRecorded as buffa::MessageName>::FULL_NAME,
            &occurrence_recorded.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(
            decoded.event,
            Some(ScheduleEventCase::ScheduleOccurrenceRecorded(_))
        ));

        let decoded = <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
            <v1::ScheduleOccurrenceScheduled as buffa::MessageName>::FULL_NAME,
            &occurrence_scheduled.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(
            decoded.event,
            Some(ScheduleEventCase::ScheduleOccurrenceScheduled(_))
        ));

        let decoded = <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
            <v1::ScheduleCompleted as buffa::MessageName>::FULL_NAME,
            &completed.encode_to_vec(),
        ))
        .unwrap()
        .into_decoded()
        .unwrap();
        assert!(matches!(decoded.event, Some(ScheduleEventCase::ScheduleCompleted(_))));
    }

    #[test]
    fn event_decode_skips_unknown_event_type() {
        assert!(matches!(
            <v1::ScheduleEvent as EventDecode>::decode(EventData::new("trogonai.scheduler.schedules.v1.Unknown", &[])),
            Ok(EventDecodeOutcome::Skipped)
        ));
    }

    #[test]
    fn event_decode_preserves_payload_decode_errors() {
        assert!(matches!(
            <v1::ScheduleEvent as EventDecode>::decode(EventData::new(
                <v1::ScheduleRemoved as buffa::MessageName>::FULL_NAME,
                b"\0"
            )),
            Err(ScheduleEventPayloadError::Decode(_))
        ));
    }

    #[test]
    fn event_type_returns_inner_event_full_name() {
        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        };

        assert_eq!(
            event.event_type().unwrap(),
            <v1::ScheduleRemoved as buffa::MessageName>::FULL_NAME
        );
    }

    #[test]
    fn event_type_returns_all_lifecycle_event_full_names() {
        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: "backup".to_string(),
                    ..schedule_created()
                }
                .into(),
            ),
        };
        assert_eq!(
            event.event_type().unwrap(),
            <v1::ScheduleCreated as buffa::MessageName>::FULL_NAME
        );

        let event = v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        };
        assert_eq!(
            event.event_type().unwrap(),
            <v1::SchedulePaused as buffa::MessageName>::FULL_NAME
        );

        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleResumed {
                    schedule_id: "backup".to_string(),
                }
                .into(),
            ),
        };
        assert_eq!(
            event.event_type().unwrap(),
            <v1::ScheduleResumed as buffa::MessageName>::FULL_NAME
        );

        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: Some(1),
                    occurrence_at: MessageField::some(timestamp()),
                    recorded_at: MessageField::some(timestamp()),
                }
                .into(),
            ),
        };
        assert_eq!(
            event.event_type().unwrap(),
            <v1::ScheduleOccurrenceRecorded as buffa::MessageName>::FULL_NAME
        );

        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceScheduled {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: Some(2),
                    occurrence_at: MessageField::some(timestamp()),
                    scheduled_at: MessageField::some(timestamp()),
                }
                .into(),
            ),
        };
        assert_eq!(
            event.event_type().unwrap(),
            <v1::ScheduleOccurrenceScheduled as buffa::MessageName>::FULL_NAME
        );

        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCompleted {
                    schedule_id: "backup".to_string(),
                    last_occurrence_sequence: Some(2),
                }
                .into(),
            ),
        };
        assert_eq!(
            event.event_type().unwrap(),
            <v1::ScheduleCompleted as buffa::MessageName>::FULL_NAME
        );
    }

    #[test]
    fn event_type_rejects_missing_event_case() {
        let event = v1::ScheduleEvent { event: None };

        assert!(matches!(
            event.event_type(),
            Err(ScheduleEventPayloadError::MissingEvent)
        ));
    }

    #[test]
    fn state_snapshot_round_trips_through_snapshot_traits() {
        let state = state_v1::State {
            state: Some(buffa::EnumValue::from(
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED,
            )),
            last_occurrence_at: MessageField::some(buffa_types::google::protobuf::Timestamp::from_unix(
                1_451_600_400,
                0,
            )),
            last_occurrence_sequence: Some(7),
            schedule: MessageField::default(),
            pending_occurrence_at: MessageField::default(),
            completed: Some(true),
        };

        let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
        let decoded = <state_v1::State as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded)).unwrap();

        assert_eq!(decoded, state);
    }

    #[test]
    fn state_snapshot_type_uses_generated_full_name() {
        assert_eq!(
            <state_v1::State as SnapshotType>::snapshot_type().unwrap().as_str(),
            <state_v1::State as buffa::MessageName>::FULL_NAME
        );
    }

    #[test]
    fn state_snapshot_decode_preserves_payload_decode_errors() {
        let error = <state_v1::State as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(b"\0")).unwrap_err();

        assert!(!error.to_string().is_empty());
        assert!(std::error::Error::source(&error).is_some());
    }
}
