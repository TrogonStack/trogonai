use buffa::Message as _;
use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

use super::*;

fn agent_provisioned() -> v1::AgentProvisioned {
    v1::AgentProvisioned {
        agent_id: "agent-1".to_string(),
        annotations: [("example.com/source".to_string(), "import".to_string())]
            .into_iter()
            .collect(),
        transient_annotations: [("example.com/trace".to_string(), "trace-1".to_string())]
            .into_iter()
            .collect(),
        ..v1::AgentProvisioned::default()
    }
}

#[test]
fn event_encode_writes_inner_event_payload() {
    let inner = agent_provisioned();
    let event = v1::AgentEvent {
        event: Some(inner.clone().into()),
    };

    let encoded = EventEncode::encode(&event).unwrap();

    assert_eq!(v1::AgentProvisioned::decode_from_slice(&encoded).unwrap(), inner);
}

#[test]
fn event_encode_rejects_missing_event_case() {
    let event = v1::AgentEvent { event: None };

    assert!(matches!(
        EventEncode::encode(&event),
        Err(AgentEventPayloadError::MissingEvent)
    ));
}

#[test]
fn event_decode_dispatches_by_generated_full_name() {
    let inner = agent_provisioned();
    let encoded = inner.encode_to_vec();

    let decoded = <v1::AgentEvent as EventDecode>::decode(EventData::new(
        <v1::AgentProvisioned as buffa::MessageName>::FULL_NAME,
        &encoded,
    ))
    .unwrap();

    let decoded = decoded.into_decoded().unwrap();
    assert!(matches!(decoded.event, Some(AgentEventCase::AgentProvisioned(_))));
}

#[test]
fn event_decode_skips_unknown_event_type() {
    assert!(matches!(
        <v1::AgentEvent as EventDecode>::decode(EventData::new("trogonai.agents.agents.v1.Unknown", &[])),
        Ok(EventDecodeOutcome::Skipped)
    ));
}

#[test]
fn event_decode_preserves_payload_decode_errors() {
    assert!(matches!(
        <v1::AgentEvent as EventDecode>::decode(EventData::new(
            <v1::AgentProvisioned as buffa::MessageName>::FULL_NAME,
            b"\0"
        )),
        Err(AgentEventPayloadError::Decode(_))
    ));
}

#[test]
fn event_type_returns_inner_event_full_name() {
    let event = v1::AgentEvent {
        event: Some(agent_provisioned().into()),
    };

    assert_eq!(
        event.event_type().unwrap(),
        <v1::AgentProvisioned as buffa::MessageName>::FULL_NAME
    );
}

#[test]
fn event_type_rejects_missing_event_case() {
    let event = v1::AgentEvent { event: None };

    assert!(matches!(event.event_type(), Err(AgentEventPayloadError::MissingEvent)));
}
