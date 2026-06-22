use trogon_decider::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventType};

use super::*;

#[test]
fn turn_on_try_from_requires_light_id() {
    assert!(
        TurnOnCommand::try_from(v1::TurnOn {
            light_id: String::new()
        })
        .is_err()
    );
    assert_eq!(
        TurnOnCommand::try_from(v1::TurnOn {
            light_id: "kitchen".to_string()
        })
        .unwrap()
        .light_id,
        "kitchen"
    );
}

#[test]
fn light_event_round_trips() {
    let event = v1::LightEvent {
        event: Some(LightEventCase::from(v1::LightTurnedOn {
            light_id: "kitchen".to_string(),
            turn_on_count: 1,
        })),
    };
    let encoded = EventEncode::encode(&event).unwrap();
    let decoded = <v1::LightEvent as EventDecode>::decode(EventData::new(
        <v1::LightEvent as EventType>::event_type(&event).unwrap(),
        &encoded,
    ))
    .unwrap()
    .into_decoded()
    .unwrap();
    assert_eq!(decoded, event);
}

#[test]
fn light_event_skips_unknown_type() {
    let outcome =
        <v1::LightEvent as EventDecode>::decode(EventData::new("trogonai.example.light.v1.Unknown", &[])).unwrap();
    assert_eq!(outcome, EventDecodeOutcome::Skipped);
}
