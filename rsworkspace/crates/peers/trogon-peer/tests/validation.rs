#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use buffa_types::google::protobuf::Timestamp;
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
        event: Some(PeerEventCase::from(proto::PeerDetached::default())),
    };
    assert_eq!(
        evolve(initial_state(), &unsupported),
        Err(EvolveError::UnsupportedEvent)
    );
}

fn command(now: DateTime<Utc>) -> Enroll {
    Enroll::new(
        identity("tenant", "caller", "installation", "session"),
        ConnectorId::new("connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        PeerDisplayName::new("Peer").unwrap(),
        now,
    )
}

fn attachment() -> proto::PeerAttachment {
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    proto::PeerAttachment {
        attachment: MessageField::some(proto::AttachmentRef {
            peer_id: enrollment().peer_id,
            connector_id: Some("connector".to_owned()),
            generation: Some(1),
        }),
        status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
        observed_at: MessageField::some(timestamp_from_datetime(&now)),
        expires_at: MessageField::some(timestamp_from_datetime(&(now + chrono::Duration::seconds(10)))),
    }
}

fn attached(value: proto::PeerAttachment) -> proto::PeerEvent {
    proto::PeerEvent {
        event: Some(
            proto::PeerAttached {
                attachment: MessageField::some(value),
            }
            .into(),
        ),
    }
}

#[test]
fn enrollment_cannot_be_decided_or_replayed_twice() {
    let event = enrolled(enrollment());
    let state = evolve(initial_state(), &event).unwrap();
    assert_eq!(evolve(state.clone(), &event), Err(EvolveError::AlreadyEnrolled));
    TestCase::<Enroll>::new()
        .given([event])
        .when(command(Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap()))
        .then_error(EnrollError::AlreadyEnrolled);
    let attachment_event = attached(attachment());
    let state = evolve(state, &attachment_event).unwrap();
    assert_eq!(evolve(state, &attachment_event), Err(EvolveError::InvalidAttachment));
}

#[test]
fn enrollment_replay_requires_an_enrollment_payload() {
    let event = proto::PeerEvent {
        event: Some(proto::PeerEnrolled::default().into()),
    };
    assert_eq!(evolve(initial_state(), &event), Err(EvolveError::MissingEnrollment));
    assert_eq!(
        evolve(initial_state(), &attached(attachment())),
        Err(EvolveError::MissingEnrollment)
    );
}

#[test]
fn enrollment_replay_reports_each_missing_identity_or_presentation_field() {
    for (field, value) in [
        (
            "enrollment.peer_id",
            proto::PeerEnrollment {
                peer_id: None,
                ..enrollment()
            },
        ),
        (
            "enrollment.tenant_id",
            proto::PeerEnrollment {
                tenant_id: None,
                ..enrollment()
            },
        ),
        (
            "enrollment.caller_id",
            proto::PeerEnrollment {
                caller_id: None,
                ..enrollment()
            },
        ),
        (
            "enrollment.installation_id",
            proto::PeerEnrollment {
                installation_id: None,
                ..enrollment()
            },
        ),
        (
            "enrollment.native_session_id",
            proto::PeerEnrollment {
                native_session_id: None,
                ..enrollment()
            },
        ),
        (
            "enrollment.collaboration_ref",
            proto::PeerEnrollment {
                collaboration_ref: None,
                ..enrollment()
            },
        ),
        (
            "enrollment.display_name",
            proto::PeerEnrollment {
                display_name: None,
                ..enrollment()
            },
        ),
        (
            "enrollment.enrolled_at",
            proto::PeerEnrollment {
                enrolled_at: MessageField::default(),
                ..enrollment()
            },
        ),
    ] {
        assert_eq!(
            evolve(initial_state(), &enrolled(value)),
            Err(EvolveError::MissingField { field })
        );
    }
}

#[test]
fn enrollment_replay_validates_every_owner_and_presentation_value() {
    for (field, value) in [
        (
            "enrollment.tenant_id",
            proto::PeerEnrollment {
                tenant_id: Some(String::new()),
                ..enrollment()
            },
        ),
        (
            "enrollment.caller_id",
            proto::PeerEnrollment {
                caller_id: Some(String::new()),
                ..enrollment()
            },
        ),
        (
            "enrollment.installation_id",
            proto::PeerEnrollment {
                installation_id: Some(String::new()),
                ..enrollment()
            },
        ),
        (
            "enrollment.native_session_id",
            proto::PeerEnrollment {
                native_session_id: Some(String::new()),
                ..enrollment()
            },
        ),
        (
            "enrollment.collaboration_ref",
            proto::PeerEnrollment {
                collaboration_ref: Some(String::new()),
                ..enrollment()
            },
        ),
        (
            "enrollment.display_name",
            proto::PeerEnrollment {
                display_name: Some(String::new()),
                ..enrollment()
            },
        ),
    ] {
        assert_eq!(
            evolve(initial_state(), &enrolled(value)),
            Err(EvolveError::InvalidValue {
                field,
                source: PeerValueError::Empty
            })
        );
    }
}

#[test]
fn attachment_replay_requires_payload_reference_identity_and_connector() {
    let state = evolve(initial_state(), &enrolled(enrollment())).unwrap();
    let missing_payload = proto::PeerEvent {
        event: Some(proto::PeerAttached::default().into()),
    };
    assert_eq!(
        evolve(state.clone(), &missing_payload),
        Err(EvolveError::MissingField { field: "attachment" })
    );
    let mut value = attachment();
    value.attachment = MessageField::default();
    assert_eq!(
        evolve(state.clone(), &attached(value)),
        Err(EvolveError::MissingField {
            field: "attachment.ref"
        })
    );
    for (reference, error) in [
        (
            proto::AttachmentRef {
                peer_id: None,
                ..attachment().attachment.into_option().unwrap()
            },
            EvolveError::MissingField {
                field: "attachment.peer_id",
            },
        ),
        (
            proto::AttachmentRef {
                peer_id: Some("other_peer".to_owned()),
                ..attachment().attachment.into_option().unwrap()
            },
            EvolveError::IdentityMismatch,
        ),
        (
            proto::AttachmentRef {
                connector_id: None,
                ..attachment().attachment.into_option().unwrap()
            },
            EvolveError::MissingField {
                field: "attachment.connector_id",
            },
        ),
        (
            proto::AttachmentRef {
                connector_id: Some("invalid.*".to_owned()),
                ..attachment().attachment.into_option().unwrap()
            },
            EvolveError::InvalidValue {
                field: "attachment.connector_id",
                source: PeerValueError::InvalidCharacter,
            },
        ),
    ] {
        let mut value = attachment();
        value.attachment = MessageField::some(reference);
        assert_eq!(evolve(state.clone(), &attached(value)), Err(error));
    }
}

#[test]
fn attachment_replay_cannot_predate_enrollment_or_omit_lease_expiry() {
    let state = evolve(initial_state(), &enrolled(enrollment())).unwrap();
    let mut value = attachment();
    value.expires_at = MessageField::default();
    assert_eq!(
        evolve(state.clone(), &attached(value)),
        Err(EvolveError::MissingField {
            field: "attachment.expires_at"
        })
    );
    let mut value = attachment();
    let earlier = Utc.with_ymd_and_hms(2026, 11, 2, 11, 59, 59).unwrap();
    value.observed_at = MessageField::some(timestamp_from_datetime(&earlier));
    value.expires_at = MessageField::some(timestamp_from_datetime(&(earlier + chrono::Duration::seconds(10))));
    assert_eq!(evolve(state, &attached(value)), Err(EvolveError::InvalidAttachment));
}

#[test]
fn replay_rejects_timestamp_range_and_nanosecond_violations() {
    for (seconds, nanos) in [
        (-62_135_596_801, 0),
        (253_402_300_800, 0),
        (i64::MIN, 0),
        (i64::MAX, 0),
        (0, -1),
        (0, 1_000_000_000),
        (59, 1_000_000_000),
        (59, i32::MAX),
    ] {
        let invalid = Timestamp {
            seconds,
            nanos,
            ..Default::default()
        };
        let mut value = enrollment();
        value.enrolled_at = MessageField::some(invalid.clone());
        assert_eq!(
            evolve(initial_state(), &enrolled(value)),
            Err(EvolveError::TimestampRange {
                field: "enrollment.enrolled_at"
            })
        );
        let state = evolve(initial_state(), &enrolled(enrollment())).unwrap();
        let mut value = attachment();
        value.observed_at = MessageField::some(invalid.clone());
        assert_eq!(
            evolve(state.clone(), &attached(value)),
            Err(EvolveError::TimestampRange {
                field: "attachment.observed_at"
            })
        );
        let mut value = attachment();
        value.expires_at = MessageField::some(invalid);
        assert_eq!(
            evolve(state, &attached(value)),
            Err(EvolveError::TimestampRange {
                field: "attachment.expires_at"
            })
        );
    }
}

#[test]
fn enrollment_accepts_replayable_timestamp_boundaries() {
    for seconds in [-62_135_596_800, 253_402_300_789] {
        let now = DateTime::<Utc>::from_timestamp(seconds, 999_999_999).unwrap();
        let mut expected_enrollment = enrollment();
        expected_enrollment.enrolled_at = MessageField::some(timestamp_from_datetime(&now));
        let mut expected_attachment = attachment();
        expected_attachment.observed_at = MessageField::some(timestamp_from_datetime(&now));
        expected_attachment.expires_at =
            MessageField::some(timestamp_from_datetime(&(now + chrono::Duration::seconds(10))));
        let result = TestCase::<Enroll>::new()
            .given_no_history()
            .when(command(now))
            .then([enrolled(expected_enrollment), attached(expected_attachment)]);
        assert!(result.resulting_state().attachment.is_set());
    }
}

#[test]
fn enrollment_rejects_time_before_the_protobuf_epoch_range_even_if_lease_fits() {
    let now = DateTime::<Utc>::from_timestamp(-62_135_596_801, 0).unwrap();
    TestCase::<Enroll>::new()
        .given_no_history()
        .when(command(now))
        .then_error(EnrollError::LeaseDeadlineOverflow);
}
