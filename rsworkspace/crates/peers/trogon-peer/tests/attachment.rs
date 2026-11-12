#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use chrono::{DateTime, TimeZone, Utc};
use trogon_decider::{Decider, WritePrecondition, testing::TestCase};
use trogon_peer::commands::state::{evolve, initial_state};
use trogon_peer::{
    Attach, AttachError, CallerId, CollaborationRef, ConnectorId, EnrollmentIdentity, EvolveError, InstallationId,
    NativeSessionId, TenantId, TrustedPeerOwner,
};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};

fn identity() -> EnrollmentIdentity {
    EnrollmentIdentity::new(
        TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("caller").unwrap()),
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new("session").unwrap(),
    )
}

fn attachment(
    identity: &EnrollmentIdentity,
    connector: &str,
    generation: u64,
    now: DateTime<Utc>,
) -> proto::PeerAttachment {
    proto::PeerAttachment {
        attachment_ref: MessageField::some(proto::AttachmentRef {
            peer_id: Some(identity.peer_id().to_string()),
            connector_id: Some(connector.to_owned()),
            generation: Some(generation),
        }),
        status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
        observed_at: MessageField::some(timestamp_from_datetime(&now)),
        expires_at: MessageField::some(timestamp_from_datetime(&(now + chrono::Duration::seconds(10)))),
    }
}

fn state(now: DateTime<Utc>) -> PeerState {
    let identity = identity();
    PeerState {
        enrollment: MessageField::some(proto::PeerEnrollment {
            peer_id: Some(identity.peer_id().to_string()),
            tenant_id: Some("tenant".to_owned()),
            caller_id: Some("caller".to_owned()),
            installation_id: Some("installation".to_owned()),
            native_session_id: Some("session".to_owned()),
            collaboration_ref: Some("collaboration".to_owned()),
            display_name: Some("Peer".to_owned()),
            enrolled_at: MessageField::some(timestamp_from_datetime(&now)),
        }),
        attachment: MessageField::some(attachment(&identity, "old_connector", 1, now)),
        availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
        next_intake_ordinal: Some(19),
        inbox: vec![proto::InboxEntry::default()],
        outbox: vec![proto::OutboxEntry::default()],
        batches: vec![proto::PeerBatch::default()],
        dispositions: vec![proto::RecipientReceipt::default()],
        ..Default::default()
    }
}

#[test]
fn an_expired_attachment_advances_generation_without_replacing_mailbox_history() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(10);
    let before = state(start);
    let next = attachment(&identity(), "new_connector", 2, now);
    let command = Attach::new(
        identity(),
        ConnectorId::new("new_connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        now,
    );
    let mut expected = before.clone();
    expected.attachment = MessageField::some(next.clone());
    expected.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into());
    TestCase::<Attach>::new()
        .given_state(before)
        .when(command)
        .then([proto::PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: MessageField::some(next),
                }
                .into(),
            ),
        }])
        .then_state(expected);
}

#[test]
fn replacement_does_not_report_the_previous_connectors_blocked_condition() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(10);
    let mut before = state(start);
    before.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED.into());
    before.condition = Some(proto::PeerCondition::PEER_CONDITION_RATE_LIMIT.into());
    let next = attachment(&identity(), "new_connector", 2, now);
    let mut expected = before.clone();
    expected.attachment = MessageField::some(next.clone());
    expected.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into());
    expected.condition = None;
    TestCase::<Attach>::new()
        .given_state(before)
        .when(command("new_connector", "collaboration", now))
        .then([proto::PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: MessageField::some(next),
                }
                .into(),
            ),
        }])
        .then_state(expected);
}

fn command(connector: &str, collaboration: &str, now: DateTime<Utc>) -> Attach {
    Attach::new(
        identity(),
        ConnectorId::new(connector).unwrap(),
        CollaborationRef::new(collaboration).unwrap(),
        now,
    )
}

#[test]
fn active_attachment_is_idempotent_for_its_owner_and_excludes_other_connectors() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(2);
    TestCase::<Attach>::new()
        .given_state(state(start))
        .when(command("old_connector", "collaboration", now))
        .then_error(AttachError::AlreadyAttached);
    TestCase::<Attach>::new()
        .given_state(state(start))
        .when(command("new_connector", "collaboration", now))
        .then_error(AttachError::InUse);
}

#[test]
fn attachment_cannot_change_immutable_identity_or_collaboration() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(10);
    TestCase::<Attach>::new()
        .given_state(state(start))
        .when(command("new_connector", "other", now))
        .then_error(AttachError::CollaborationMismatch);
    let other = EnrollmentIdentity::new(
        TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("other").unwrap()),
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new("session").unwrap(),
    );
    TestCase::<Attach>::new()
        .given_state(state(start))
        .when(Attach::new(
            other,
            ConnectorId::new("new_connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            now,
        ))
        .then_error(AttachError::IdentityMismatch);
    TestCase::<Attach>::new()
        .given_no_history()
        .when(command("new_connector", "collaboration", now))
        .then_error(AttachError::MissingEnrollment);
}

#[test]
fn attachment_rejects_clock_regression_and_exhausted_generation() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    TestCase::<Attach>::new()
        .given_state(state(start))
        .when(command(
            "new_connector",
            "collaboration",
            start - chrono::Duration::seconds(1),
        ))
        .then_error(AttachError::ClockRegression);
    let mut exhausted = state(start);
    exhausted.attachment = MessageField::some(attachment(&identity(), "old_connector", u64::MAX, start));
    TestCase::<Attach>::new()
        .given_state(exhausted)
        .when(command(
            "new_connector",
            "collaboration",
            start + chrono::Duration::seconds(10),
        ))
        .then_error(AttachError::GenerationOverflow);
}

#[test]
fn replacement_replay_rejects_gaps_and_overlap_with_a_live_attachment() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    for (generation, at) in [
        (3, start + chrono::Duration::seconds(10)),
        (2, start + chrono::Duration::seconds(9)),
    ] {
        let event = proto::PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: MessageField::some(attachment(&identity(), "new_connector", generation, at)),
                }
                .into(),
            ),
        };
        assert!(trogon_peer::commands::state::evolve(state(start), &event).is_err());
    }
}

fn enrollment_history(start: DateTime<Utc>) -> [proto::PeerEvent; 2] {
    let initial = state(start);
    [
        proto::PeerEvent {
            event: Some(
                proto::PeerEnrolled {
                    enrollment: initial.enrollment,
                }
                .into(),
            ),
        },
        proto::PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: initial.attachment,
                }
                .into(),
            ),
        },
    ]
}

#[test]
fn attachment_timestamp_preserves_valid_nanoseconds() {
    let start = DateTime::from_timestamp(1_483_228_779, 0).unwrap();
    for nanos in [0, 999_999_999] {
        let now = DateTime::from_timestamp(1_483_228_799, nanos).unwrap();
        let next = attachment(&identity(), "new_connector", 2, now);
        TestCase::<Attach>::new()
            .given(enrollment_history(start))
            .when(command("new_connector", "collaboration", now))
            .then([proto::PeerEvent {
                event: Some(
                    proto::PeerAttached {
                        attachment: MessageField::some(next),
                    }
                    .into(),
                ),
            }]);
    }
}

#[test]
fn attachment_timestamp_rejects_leap_seconds_before_protobuf_conversion() {
    let start = DateTime::from_timestamp(1_483_228_779, 0).unwrap();
    for nanos in [1_000_000_000, 1_500_000_000, 1_999_999_999] {
        let now = DateTime::from_timestamp(1_483_228_799, nanos).unwrap();
        TestCase::<Attach>::new()
            .given(enrollment_history(start))
            .when(command("new_connector", "collaboration", now))
            .then_error(AttachError::LeaseDeadlineOverflow);
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

fn replayed_state(start: DateTime<Utc>) -> PeerState {
    enrollment_history(start)
        .iter()
        .fold(initial_state(), |state, event| evolve(state, event).unwrap())
}

#[test]
fn attachment_metadata_routes_to_the_enrolled_stream_with_a_revision_fence() {
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let command = command("new_connector", "collaboration", now);
    assert_eq!(command.identity(), &identity());
    assert_eq!(command.stream_id(), identity().peer_id());
    assert_eq!(command.connector_id(), &ConnectorId::new("new_connector").unwrap());
    assert_eq!(
        command.collaboration(),
        &CollaborationRef::new("collaboration").unwrap()
    );
    assert_eq!(command.now(), now);
    assert_eq!(Attach::WRITE_PRECONDITION, WritePrecondition::StreamUnchanged);
}

#[test]
fn attachment_lease_excludes_connectors_until_the_exact_expiry() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let expiry = start + chrono::Duration::seconds(10);
    for (connector, error) in [
        ("old_connector", AttachError::AlreadyAttached),
        ("new_connector", AttachError::InUse),
    ] {
        TestCase::<Attach>::new()
            .given(enrollment_history(start))
            .when(command(
                connector,
                "collaboration",
                expiry - chrono::Duration::nanoseconds(1),
            ))
            .then_error(error);
        for now in [expiry, expiry + chrono::Duration::nanoseconds(1)] {
            let next = attachment(&identity(), connector, 2, now);
            let mut expected = replayed_state(start);
            expected.attachment = MessageField::some(next.clone());
            TestCase::<Attach>::new()
                .given(enrollment_history(start))
                .when(command(connector, "collaboration", now))
                .then([attached(next)])
                .then_state(expected);
        }
    }
}

#[test]
fn attachment_lease_deadline_must_fit_the_protobuf_timestamp_range() {
    let start = DateTime::from_timestamp(253_402_300_769, 0).unwrap();
    let now = DateTime::from_timestamp(253_402_300_789, 999_999_999).unwrap();
    TestCase::<Attach>::new()
        .given(enrollment_history(start))
        .when(command("new_connector", "collaboration", now))
        .then([attached(attachment(&identity(), "new_connector", 2, now))]);
    for now in [
        DateTime::from_timestamp(253_402_300_790, 0).unwrap(),
        DateTime::<Utc>::MAX_UTC,
    ] {
        TestCase::<Attach>::new()
            .given(enrollment_history(start))
            .when(command("new_connector", "collaboration", now))
            .then_error(AttachError::LeaseDeadlineOverflow);
    }
}

#[test]
fn attachment_requires_the_initial_attachment_after_enrollment() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let [enrolled, _] = enrollment_history(start);
    TestCase::<Attach>::new()
        .given([enrolled])
        .when(command("new_connector", "collaboration", start))
        .then_error(AttachError::InvalidState(EvolveError::MissingField {
            field: "attachment",
        }));
}

#[test]
fn attachment_rejects_missing_or_invalid_lease_fields_in_a_persisted_snapshot() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    // Persisted protobuf snapshots can be corrupt even when replayed events cannot produce these states.
    let mut missing_expiry = replayed_state(start);
    missing_expiry.attachment = MessageField::some(proto::PeerAttachment {
        expires_at: MessageField::default(),
        ..missing_expiry.attachment.into_option().unwrap()
    });
    TestCase::<Attach>::new()
        .given_state(missing_expiry)
        .when(command(
            "new_connector",
            "collaboration",
            start + chrono::Duration::seconds(10),
        ))
        .then_error(AttachError::InvalidState(EvolveError::MissingField {
            field: "attachment.expires_at",
        }));
    let mut missing_generation = replayed_state(start);
    let mut value = missing_generation.attachment.into_option().unwrap();
    value.attachment_ref = MessageField::some(proto::AttachmentRef {
        generation: None,
        ..value.attachment_ref.into_option().unwrap()
    });
    missing_generation.attachment = MessageField::some(value);
    TestCase::<Attach>::new()
        .given_state(missing_generation)
        .when(command(
            "new_connector",
            "collaboration",
            start + chrono::Duration::seconds(10),
        ))
        .then_error(AttachError::InvalidState(EvolveError::InvalidAttachment));
}

#[test]
fn first_attachment_replay_requires_generation_one() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let [enrolled, _] = enrollment_history(start);
    let before = evolve(initial_state(), &enrolled).unwrap();
    for generation in [0, 2, u64::MAX] {
        assert_eq!(
            evolve(
                before.clone(),
                &attached(attachment(&identity(), "new_connector", generation, start))
            ),
            Err(EvolveError::InvalidAttachment)
        );
    }
    let next = attachment(&identity(), "new_connector", 1, start);
    let after = evolve(before, &attached(next.clone())).unwrap();
    assert_eq!(after.attachment.as_option(), Some(&next));
    assert_eq!(
        after.availability,
        Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into())
    );
}

#[test]
fn attached_events_cannot_smuggle_a_detached_or_unknown_status() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let [enrolled, _] = enrollment_history(start);
    let before = evolve(initial_state(), &enrolled).unwrap();
    for status in [
        None,
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_UNSPECIFIED.into()),
        Some(buffa::EnumValue::from(77)),
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED.into()),
    ] {
        let mut next = attachment(&identity(), "new_connector", 1, start);
        next.status = status;
        if status == Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED.into()) {
            next.expires_at = next.observed_at.clone();
        }
        assert_eq!(
            evolve(before.clone(), &attached(next)),
            Err(EvolveError::InvalidAttachment)
        );
    }
}

#[test]
fn replacement_replay_requires_the_next_generation_and_monotonic_observation() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let observed = start + chrono::Duration::seconds(20);
    let [enrolled, _] = enrollment_history(start);
    let before = evolve(initial_state(), &enrolled).unwrap();
    let before = evolve(before, &attached(attachment(&identity(), "old_connector", 1, observed))).unwrap();
    for (generation, now) in [
        (1, observed + chrono::Duration::seconds(10)),
        (3, observed + chrono::Duration::seconds(10)),
        (2, observed - chrono::Duration::nanoseconds(1)),
        (
            2,
            observed + chrono::Duration::seconds(10) - chrono::Duration::nanoseconds(1),
        ),
    ] {
        assert_eq!(
            evolve(
                before.clone(),
                &attached(attachment(&identity(), "new_connector", generation, now))
            ),
            Err(EvolveError::InvalidAttachment)
        );
    }
    let next = attachment(
        &identity(),
        "new_connector",
        2,
        observed + chrono::Duration::seconds(10),
    );
    let mut expected = before.clone();
    expected.attachment = MessageField::some(next.clone());
    assert_eq!(evolve(before, &attached(next)), Ok(expected));
}

#[test]
fn replacement_clears_the_previous_connectors_condition_and_preserves_progress() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(10);
    let mut before = state(start);
    before.condition = Some(proto::PeerCondition::PEER_CONDITION_AUTHENTICATION.into());
    before.active_at = MessageField::some(timestamp_from_datetime(&start));
    let next = attachment(&identity(), "new_connector", 2, now);
    let mut expected = before.clone();
    expected.attachment = MessageField::some(next.clone());
    expected.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into());
    expected.condition = None;
    TestCase::<Attach>::new()
        .given_state(before)
        .when(command("new_connector", "collaboration", now))
        .then([proto::PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: MessageField::some(next),
                }
                .into(),
            ),
        }])
        .then_state(expected);
}
