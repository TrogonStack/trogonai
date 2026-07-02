use super::*;

#[test]
fn round_trips_alive() {
    let bytes = encode_kill_status(&KillSwitchState::Alive).unwrap();
    assert_eq!(decode_kill_status(&bytes).unwrap(), KillSwitchState::Alive);
}

#[test]
fn round_trips_killed() {
    let state = KillSwitchState::Killed {
        reason: KillReason::RingBreach,
        since: OccurredAt::now(),
    };
    let bytes = encode_kill_status(&state).unwrap();
    assert_eq!(decode_kill_status(&bytes).unwrap(), state);
}

#[test]
fn wire_uses_a_tagged_json_shape() {
    let bytes = encode_kill_status(&KillSwitchState::Alive).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "alive");
}

#[test]
fn decode_rejects_unknown_reason() {
    let json = br#"{"status":"killed","reason":"not_a_reason","since":"2026-07-02T00:00:00Z"}"#;
    let error = decode_kill_status(json).unwrap_err();
    assert!(matches!(error, KillStatusWireError::KillReason(_)));
}

#[test]
fn decode_rejects_malformed_timestamp() {
    let json = br#"{"status":"killed","reason":"manual","since":"not-a-timestamp"}"#;
    let error = decode_kill_status(json).unwrap_err();
    assert!(matches!(error, KillStatusWireError::OccurredAt(_)));
}

#[test]
fn decode_rejects_malformed_json() {
    let error = decode_kill_status(b"not json").unwrap_err();
    assert!(matches!(error, KillStatusWireError::Deserialize(_)));
}
