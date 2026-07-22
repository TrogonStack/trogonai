use buffa::{Message as _, MessageName as _};

#[cfg(feature = "agents")]
use crate::agents::agents::v1::AgentProvisioned;
#[cfg(feature = "schedules")]
use crate::scheduler::schedules::v1::ScheduleOccurrenceRecorded;
#[cfg(feature = "schedules")]
use buffa::MessageField;

#[cfg(feature = "schedules")]
fn timestamp(seconds: i64) -> buffa_types::google::protobuf::Timestamp {
    buffa_types::google::protobuf::Timestamp {
        seconds,
        nanos: 0,
        ..buffa_types::google::protobuf::Timestamp::default()
    }
}

#[cfg(feature = "schedules")]
#[test]
fn decode_event_to_json_is_canonical_across_wire_orderings() {
    let event = ScheduleOccurrenceRecorded {
        schedule_id: "backup".to_string(),
        occurrence_sequence: Some(7),
        occurrence_at: MessageField::some(timestamp(1_700_000_000)),
        recorded_at: MessageField::some(timestamp(1_700_000_005)),
    };
    let canonical = event.encode_to_vec();

    // Re-encode the same message with field 2 (occurrence_sequence) emitted
    // before field 1; protobuf permits any field order, so this is a valid
    // alternate encoding that differs on the wire. Build each fragment with the
    // encoder rather than hand-rolling tags.
    let only_sequence = ScheduleOccurrenceRecorded {
        occurrence_sequence: Some(7),
        ..ScheduleOccurrenceRecorded::default()
    }
    .encode_to_vec();
    let without_sequence = ScheduleOccurrenceRecorded {
        occurrence_sequence: None,
        ..event.clone()
    }
    .encode_to_vec();
    let reordered = [only_sequence, without_sequence].concat();
    assert_ne!(canonical, reordered, "encodings must differ on the wire");

    let from_canonical = super::decode_event_to_json(ScheduleOccurrenceRecorded::FULL_NAME, &canonical);
    let from_reordered = super::decode_event_to_json(ScheduleOccurrenceRecorded::FULL_NAME, &reordered);

    assert!(matches!(from_canonical, Ok(Some(_))));
    assert_eq!(from_canonical, from_reordered);
}

#[cfg(feature = "agents")]
#[test]
fn decode_event_to_json_registers_agent_provisioned() {
    let event = AgentProvisioned {
        agent_id: "agent-1".to_string(),
        ..AgentProvisioned::default()
    };

    let from_full_name = super::decode_event_to_json(AgentProvisioned::FULL_NAME, &event.encode_to_vec());
    let from_type_url = super::decode_event_to_json(AgentProvisioned::TYPE_URL, &event.encode_to_vec());

    assert_eq!(from_full_name, from_type_url);
    let json = from_full_name.unwrap().unwrap();
    assert!(json.contains(r#""agentId":"agent-1""#), "{json}");
}

#[test]
fn decode_event_to_json_returns_none_for_unknown_type() {
    assert_eq!(
        super::decode_event_to_json("type.googleapis.com/trogonai.unknown.v1.Event", &[]),
        Ok(None)
    );
}

#[cfg(feature = "schedules")]
#[test]
fn decode_event_to_json_errors_on_malformed_known_payload() {
    let result = super::decode_event_to_json(ScheduleOccurrenceRecorded::FULL_NAME, b"\xff\xff\xff\xff");
    assert!(
        matches!(result, Err(super::EventDecodeError::Json { .. })),
        "{result:?}"
    );
}

#[cfg(feature = "agents")]
#[test]
fn decode_event_to_json_errors_on_malformed_known_agent_payload() {
    let result = super::decode_event_to_json(AgentProvisioned::FULL_NAME, b"\xff\xff\xff\xff");
    assert!(
        matches!(result, Err(super::EventDecodeError::Json { .. })),
        "{result:?}"
    );
}
