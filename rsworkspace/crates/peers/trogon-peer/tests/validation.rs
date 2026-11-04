#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use chrono::{DateTime, TimeZone, Utc};
use trogon_decider::testing::TestCase;
use trogon_peer::commands::state::{evolve, initial_state};
use trogon_peer::{
    CallerId, CollaborationRef, ConnectorId, Enroll, EnrollError, EnrollmentIdentity, EvolveError, InstallationId,
    NativeSessionId, PeerDisplayName, PeerValueError, TenantId, TrustedPeerOwner,
};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{PeerEventCase, v1alpha1 as proto};

fn identity(tenant: &str, caller: &str, installation: &str, session: &str) -> EnrollmentIdentity {
    EnrollmentIdentity::new(
        TrustedPeerOwner::new(TenantId::new(tenant).unwrap(), CallerId::new(caller).unwrap()),
        InstallationId::new(installation).unwrap(),
        NativeSessionId::new(session).unwrap(),
    )
}

fn enrollment() -> proto::PeerEnrollment {
    let id = identity("tenant", "caller", "installation", "session");
    proto::PeerEnrollment {
        peer_id: Some(id.peer_id().to_string()),
        tenant_id: Some("tenant".to_owned()),
        caller_id: Some("caller".to_owned()),
        installation_id: Some("installation".to_owned()),
        native_session_id: Some("session".to_owned()),
        collaboration_ref: Some("collaboration".to_owned()),
        display_name: Some("Peer".to_owned()),
        enrolled_at: MessageField::some(timestamp_from_datetime(
            &Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap(),
        )),
    }
}

fn enrolled(value: proto::PeerEnrollment) -> proto::PeerEvent {
    proto::PeerEvent {
        event: Some(
            proto::PeerEnrolled {
                enrollment: MessageField::some(value),
            }
            .into(),
        ),
    }
}

#[test]
fn identity_is_stable_and_isolates_every_tuple_component() {
    let baseline = identity("tenant", "caller", "installation", "session");
    assert_eq!(baseline, identity("tenant", "caller", "installation", "session"));
    for changed in [
        identity("other", "caller", "installation", "session"),
        identity("tenant", "other", "installation", "session"),
        identity("tenant", "caller", "other", "session"),
        identity("tenant", "caller", "installation", "other"),
    ] {
        assert_ne!(baseline.peer_id(), changed.peer_id());
    }
    assert_ne!(
        identity("a", "bc", "installation", "session").peer_id(),
        identity("ab", "c", "installation", "session").peer_id()
    );
}

#[test]
fn routing_identifiers_reject_empty_oversized_and_non_token_inputs() {
    for invalid in [
        "".to_owned(),
        "x".repeat(129),
        "two.tokens".to_owned(),
        "wild*".to_owned(),
        "space value".to_owned(),
        "é".to_owned(),
    ] {
        assert!(TenantId::new(&invalid).is_err());
        assert!(CallerId::new(&invalid).is_err());
        assert!(ConnectorId::new(&invalid).is_err());
        assert!(InstallationId::new(&invalid).is_err());
    }
    assert!(TenantId::new("x".repeat(128)).is_ok());
    assert!(CallerId::new("A_9-z").is_ok());
}

#[test]
fn native_correlation_and_display_values_are_utf8_bounded_and_control_free() {
    assert_eq!(NativeSessionId::new(""), Err(PeerValueError::Empty));
    assert!(NativeSessionId::new("s".repeat(256)).is_ok());
    assert!(NativeSessionId::new("s".repeat(257)).is_err());
    assert!(CollaborationRef::new("r".repeat(256)).is_ok());
    assert!(CollaborationRef::new("r".repeat(257)).is_err());
    assert!(PeerDisplayName::new("é".repeat(40)).is_ok());
    assert!(PeerDisplayName::new("é".repeat(41)).is_err());
    for bad in ["line\nbreak", "nul\0byte", "delete\u{7f}"] {
        assert!(NativeSessionId::new(bad).is_err());
        assert!(CollaborationRef::new(bad).is_err());
        assert!(PeerDisplayName::new(bad).is_err());
    }
}

#[test]
fn enrollment_replay_rejects_missing_invalid_or_misattributed_identity() {
    let mut missing = enrollment();
    missing.caller_id = None;
    assert!(matches!(
        evolve(initial_state(), &enrolled(missing)),
        Err(EvolveError::MissingField { .. })
    ));
    let mut invalid = enrollment();
    invalid.tenant_id = Some("tenant.*".to_owned());
    assert!(matches!(
        evolve(initial_state(), &enrolled(invalid)),
        Err(EvolveError::InvalidValue { .. })
    ));
    let mut altered = enrollment();
    altered.native_session_id = Some("replacement".to_owned());
    assert_eq!(
        evolve(initial_state(), &enrolled(altered)),
        Err(EvolveError::IdentityMismatch)
    );
    let mut unknown_time = enrollment();
    unknown_time.enrolled_at = MessageField::default();
    assert!(matches!(
        evolve(initial_state(), &enrolled(unknown_time)),
        Err(EvolveError::MissingField { .. })
    ));
}

#[test]
fn attachment_replay_rejects_invalid_generation_status_or_lease() {
    let value = enrollment();
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let state = evolve(initial_state(), &enrolled(value.clone())).unwrap();
    let valid = proto::PeerAttachment {
        attachment: MessageField::some(proto::AttachmentRef {
            peer_id: value.peer_id,
            connector_id: Some("connector".to_owned()),
            generation: Some(1),
        }),
        status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
        observed_at: MessageField::some(timestamp_from_datetime(&now)),
        expires_at: MessageField::some(timestamp_from_datetime(&(now + chrono::Duration::seconds(10)))),
    };
    for mutation in 0..5 {
        let mut changed = valid.clone();
        match mutation {
            0 => changed.attachment = MessageField::default(),
            1 => {
                let mut reference = changed.attachment.as_option().unwrap().clone();
                reference.generation = Some(0);
                changed.attachment = MessageField::some(reference);
            }
            2 => changed.status = Some(buffa::EnumValue::from(77)),
            3 => changed.expires_at = MessageField::some(timestamp_from_datetime(&now)),
            _ => changed.observed_at = MessageField::default(),
        }
        let event = proto::PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: MessageField::some(changed),
                }
                .into(),
            ),
        };
        assert!(evolve(state.clone(), &event).is_err());
    }
}

#[test]
fn enrollment_rejects_unrepresentable_lease_deadlines_without_panicking() {
    for now in [
        DateTime::<Utc>::MAX_UTC,
        Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 55).unwrap(),
    ] {
        let command = Enroll::new(
            identity("tenant", "caller", "installation", "session"),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            PeerDisplayName::new("Peer").unwrap(),
            now,
        );
        TestCase::<Enroll>::new()
            .given_no_history()
            .when(command)
            .then_error(EnrollError::LeaseDeadlineOverflow);
    }
}

#[test]
fn replay_rejects_absent_or_unhandled_event_cases() {
    assert_eq!(
        evolve(initial_state(), &proto::PeerEvent::default()),
        Err(EvolveError::MissingEvent)
    );
    let unsupported = proto::PeerEvent {
        event: Some(PeerEventCase::from(proto::PeerBatchPrepared::default())),
    };
    assert_eq!(
        evolve(initial_state(), &unsupported),
        Err(EvolveError::UnsupportedEvent)
    );
}
