#![allow(clippy::expect_used)]

use super::*;

#[test]
fn a_broker_get_response_cannot_substitute_another_subject_or_revision() {
    let valid = StreamMessage {
        subject: "$KV.scope.key".into(),
        sequence: 42,
        headers: Default::default(),
        payload: Bytes::from_static(b"approved-snapshot"),
        time: std::time::SystemTime::UNIX_EPOCH.into(),
    };
    let scoped = ScopedRegistrationMessage::new("$KV.scope.key", 42, valid.clone()).expect("exact native response");
    assert_eq!(scoped.payload(), valid.payload.as_ref());
    assert_eq!(scoped.into_payload(), valid.payload);
    let oversized = StreamMessage {
        payload: Bytes::from(vec![0; MAX_SNAPSHOT_BYTES + 1]),
        ..valid.clone()
    };
    assert!(matches!(
        ScopedRegistrationMessage::new("$KV.scope.key", 42, oversized),
        Err(RegistrationError::Bounds)
    ));
    for invalid in [
        StreamMessage {
            subject: "$KV.other.key".into(),
            ..valid.clone()
        },
        StreamMessage {
            sequence: 41,
            ..valid.clone()
        },
        StreamMessage {
            sequence: 43,
            ..valid.clone()
        },
    ] {
        assert!(matches!(
            ScopedRegistrationMessage::new("$KV.scope.key", 42, invalid),
            Err(RegistrationError::HeadChanged)
        ));
    }
}
