use super::*;

#[test]
fn round_trips_every_variant_through_as_str_and_parse() {
    for reason in [
        KillReason::BehavioralDrift,
        KillReason::RateLimit,
        KillReason::RingBreach,
        KillReason::Manual,
    ] {
        assert_eq!(KillReason::parse(reason.as_str()).unwrap(), reason);
    }
}

#[test]
fn matches_agt_wire_names() {
    assert_eq!(KillReason::BehavioralDrift.as_str(), "behavioral_drift");
    assert_eq!(KillReason::RateLimit.as_str(), "rate_limit");
    assert_eq!(KillReason::RingBreach.as_str(), "ring_breach");
    assert_eq!(KillReason::Manual.as_str(), "manual");
}

#[test]
fn rejects_unknown_reason() {
    let error = KillReason::parse("quarantine_timeout").unwrap_err();
    assert_eq!(
        error,
        KillReasonError::Unknown {
            raw: "quarantine_timeout".to_string()
        }
    );
    assert_eq!(error.to_string(), "unknown kill reason: 'quarantine_timeout'");
}

#[test]
fn displays_as_wire_name() {
    assert_eq!(KillReason::Manual.to_string(), "manual");
}
