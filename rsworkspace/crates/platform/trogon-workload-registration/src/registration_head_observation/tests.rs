#![allow(clippy::expect_used)]

use serde_json::json;

use super::*;

fn native_state(messages: u64, last: u64) -> State {
    serde_json::from_value(json!({
        "messages":messages,"bytes":0,"first_seq":if messages==0 {last+1} else {last.saturating_sub(messages-1)},
        "first_ts":"2026-01-01T00:00:00Z","last_seq":last,"last_ts":"2026-01-01T00:00:00Z","consumer_count":0,
    }))
    .expect("native SDK stream state DTO")
}

#[test]
fn expired_live_state_keeps_its_global_floor_but_missing_control_history_is_not_recovery() {
    for plane in [RegistrationPlane::Control, RegistrationPlane::Live] {
        let fresh = RegistrationHeadObservation::new(plane, &native_state(0, 0)).expect("fresh epoch");
        assert_eq!(fresh.last_sequence(), 0);
        assert!(fresh.retained_sequence().is_none());
        let retained = RegistrationHeadObservation::new(plane, &native_state(1, 9)).expect("retained complete head");
        assert_eq!(retained.last_sequence(), 9);
        assert_eq!(retained.retained_sequence().expect("retained revision").get(), 9);
        assert!(matches!(
            RegistrationHeadObservation::new(plane, &native_state(2, 9)),
            Err(RegistrationError::HeadChanged)
        ));
        assert!(matches!(
            RegistrationHeadObservation::new(plane, &native_state(1, 0)),
            Err(RegistrationError::HeadChanged)
        ));
    }
    let expired = RegistrationHeadObservation::new(RegistrationPlane::Live, &native_state(0, 9))
        .expect("live payload may expire without resetting CAS");
    assert_eq!(expired.last_sequence(), 9);
    assert!(expired.retained_sequence().is_none());
    assert!(matches!(
        RegistrationHeadObservation::new(RegistrationPlane::Control, &native_state(0, 9)),
        Err(RegistrationError::HeadChanged)
    ));
}
