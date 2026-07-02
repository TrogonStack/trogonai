use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

use super::*;

fn agent(id: &str) -> AgentId {
    AgentId::parse(id).unwrap()
}

#[test]
fn agent_killed_round_trips_through_encode_and_decode() {
    let event = KillSwitchEvent::AgentKilled {
        agent_id: agent("agent-1"),
        reason: KillReason::RingBreach,
        occurred_at: OccurredAt::now(),
    };
    let event_type = event.event_type().unwrap();
    let payload = event.encode().unwrap();

    let decoded = KillSwitchEvent::decode(EventData::new(event_type, &payload)).unwrap();
    assert_eq!(decoded, EventDecodeOutcome::Decoded(event));
}

#[test]
fn agent_revived_round_trips_through_encode_and_decode() {
    let event = KillSwitchEvent::AgentRevived {
        agent_id: agent("agent-1"),
        occurred_at: OccurredAt::now(),
    };
    let event_type = event.event_type().unwrap();
    let payload = event.encode().unwrap();

    let decoded = KillSwitchEvent::decode(EventData::new(event_type, &payload)).unwrap();
    assert_eq!(decoded, EventDecodeOutcome::Decoded(event));
}

#[test]
fn event_type_names_are_stable() {
    assert_eq!(AGENT_KILLED_EVENT_TYPE, "trogon.kill_switch.agent_killed.v1");
    assert_eq!(AGENT_REVIVED_EVENT_TYPE, "trogon.kill_switch.agent_revived.v1");
}

#[test]
fn decode_skips_unrelated_event_types() {
    let outcome = KillSwitchEvent::decode(EventData::new("some.other.event.v1", b"{}")).unwrap();
    assert_eq!(outcome, EventDecodeOutcome::Skipped);
}

#[test]
fn decode_rejects_malformed_payload() {
    let error = KillSwitchEvent::decode(EventData::new(AGENT_KILLED_EVENT_TYPE, b"not json")).unwrap_err();
    assert!(matches!(error, KillSwitchEventCodecError::Deserialize(_)));
}

#[test]
fn decode_rejects_invalid_agent_id_in_payload() {
    let payload = serde_json::to_vec(&AgentKilledPayload {
        agent_id: String::new(),
        reason: "manual".to_string(),
        occurred_at: OccurredAt::now().to_rfc3339().unwrap(),
    })
    .unwrap();
    let error = KillSwitchEvent::decode(EventData::new(AGENT_KILLED_EVENT_TYPE, &payload)).unwrap_err();
    assert!(matches!(error, KillSwitchEventCodecError::AgentId(_)));
}

#[test]
fn decode_rejects_unknown_kill_reason_in_payload() {
    let payload = serde_json::to_vec(&AgentKilledPayload {
        agent_id: "agent-1".to_string(),
        reason: "not_a_real_reason".to_string(),
        occurred_at: OccurredAt::now().to_rfc3339().unwrap(),
    })
    .unwrap();
    let error = KillSwitchEvent::decode(EventData::new(AGENT_KILLED_EVENT_TYPE, &payload)).unwrap_err();
    assert!(matches!(error, KillSwitchEventCodecError::KillReason(_)));
}

#[test]
fn decode_rejects_malformed_timestamp_in_payload() {
    let payload = serde_json::to_vec(&AgentRevivedPayload {
        agent_id: "agent-1".to_string(),
        occurred_at: "not-a-timestamp".to_string(),
    })
    .unwrap();
    let error = KillSwitchEvent::decode(EventData::new(AGENT_REVIVED_EVENT_TYPE, &payload)).unwrap_err();
    assert!(matches!(error, KillSwitchEventCodecError::OccurredAt(_)));
}
