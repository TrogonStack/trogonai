#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use chrono::{DateTime, TimeZone, Utc};
use trogon_decider::{Decider, testing::TestCase};
use trogon_peer::{
    AttachmentRef, CallerId, ConnectorId, EnrollmentIdentity, Heartbeat, HeartbeatError, InstallationId,
    NativeSessionId, PeerId, PeerObservation, PeerObservationError, TenantId, TrustedPeerOwner,
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

fn reference(connector: &str, generation: u64) -> AttachmentRef {
    AttachmentRef::new(
        PeerId::parse(identity().peer_id().to_string()).unwrap(),
        ConnectorId::new(connector).unwrap(),
        generation,
    )
    .unwrap()
}
fn heartbeat(connector: &str, generation: u64, now: DateTime<Utc>, active_at: Option<DateTime<Utc>>) -> Heartbeat {
    Heartbeat::new(
        identity().owner().clone(),
        reference(connector, generation),
        PeerObservation::new(proto::PeerAvailability::PEER_AVAILABILITY_BUSY, None, active_at).unwrap(),
        now,
    )
}

#[test]
fn heartbeat_renews_only_connectivity_without_fabricating_progress() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(2);
    let before = state(start);
    let renewed = attachment(&identity(), "old_connector", 1, now);
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerHeartbeatRecorded {
                attachment: MessageField::some(renewed.clone()),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                ..Default::default()
            }
            .into(),
        ),
    };
    let mut expected = before.clone();
    expected.attachment = MessageField::some(renewed);
    let command = heartbeat("old_connector", 1, now, None);
    assert_eq!(command.owner(), identity().owner());
    assert_eq!(command.attachment(), &reference("old_connector", 1));
    assert_eq!(command.stream_id(), identity().peer_id());
    assert_eq!(
        command.observation().availability(),
        proto::PeerAvailability::PEER_AVAILABILITY_BUSY
    );
    assert_eq!(command.observation().condition(), None);
    assert_eq!(command.observation().active_at(), None);
    assert_eq!(command.now(), now);
    TestCase::<Heartbeat>::new()
        .given_state(before)
        .when(command)
        .then([event])
        .then_state(expected);
}

#[test]
fn heartbeat_cannot_revive_an_expired_or_replaced_connector() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    TestCase::<Heartbeat>::new()
        .given_state(state(start))
        .when(heartbeat(
            "old_connector",
            1,
            start + chrono::Duration::seconds(10),
            None,
        ))
        .then_error(HeartbeatError::Expired);
    for (connector, generation) in [("old_connector", 2), ("other", 1)] {
        TestCase::<Heartbeat>::new()
            .given_state(state(start))
            .when(heartbeat(
                connector,
                generation,
                start + chrono::Duration::seconds(2),
                None,
            ))
            .then_error(HeartbeatError::Fenced);
    }
}

#[test]
fn heartbeat_rejects_clock_and_activity_regression() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    TestCase::<Heartbeat>::new()
        .given_state(state(start))
        .when(heartbeat(
            "old_connector",
            1,
            start - chrono::Duration::seconds(1),
            None,
        ))
        .then_error(HeartbeatError::ClockRegression);
    let now = start + chrono::Duration::seconds(2);
    TestCase::<Heartbeat>::new()
        .given_state(state(start))
        .when(heartbeat(
            "old_connector",
            1,
            now,
            Some(now + chrono::Duration::seconds(1)),
        ))
        .then_error(HeartbeatError::InvalidActivity);
    let mut before = state(start);
    before.active_at = MessageField::some(timestamp_from_datetime(&start));
    TestCase::<Heartbeat>::new()
        .given_state(before)
        .when(heartbeat(
            "old_connector",
            1,
            now,
            Some(start - chrono::Duration::seconds(1)),
        ))
        .then_error(HeartbeatError::InvalidActivity);
}

#[test]
fn heartbeat_rejects_a_different_authenticated_owner() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let command = Heartbeat::new(
        TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("other").unwrap()),
        reference("old_connector", 1),
        PeerObservation::new(proto::PeerAvailability::PEER_AVAILABILITY_READY, None, None).unwrap(),
        start + chrono::Duration::seconds(1),
    );
    TestCase::<Heartbeat>::new()
        .given_state(state(start))
        .when(command)
        .then_error(HeartbeatError::OwnerMismatch);
}

#[test]
fn peer_reference_and_observation_reject_invalid_boundary_values() {
    for raw in [
        "".to_owned(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        assert!(PeerId::parse(raw).is_err());
    }
    assert!(
        AttachmentRef::new(
            PeerId::parse(identity().peer_id().to_string()).unwrap(),
            ConnectorId::new("connector").unwrap(),
            0
        )
        .is_err()
    );
    for outside in [
        DateTime::<Utc>::from_timestamp(-62_135_596_801, 0).unwrap(),
        DateTime::<Utc>::from_timestamp(253_402_300_800, 0).unwrap(),
        DateTime::<Utc>::from_timestamp(59, 1_000_000_000).unwrap(),
    ] {
        assert!(matches!(
            PeerObservation::new(proto::PeerAvailability::PEER_AVAILABILITY_READY, None, Some(outside)),
            Err(PeerObservationError::InvalidActivity)
        ));
    }
    assert!(PeerObservation::new(proto::PeerAvailability::PEER_AVAILABILITY_UNSPECIFIED, None, None).is_err());
    assert!(
        PeerObservation::new(
            proto::PeerAvailability::PEER_AVAILABILITY_BLOCKED,
            Some(proto::PeerCondition::PEER_CONDITION_UNSPECIFIED),
            None
        )
        .is_err()
    );
}

#[test]
fn heartbeat_preserves_reported_activity_across_connectivity_only_renewals() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(2);
    let mut before = state(start);
    before.active_at = MessageField::some(timestamp_from_datetime(&start));
    let renewed = attachment(&identity(), "old_connector", 1, now);
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerHeartbeatRecorded {
                attachment: MessageField::some(renewed.clone()),
                availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                active_at: before.active_at.clone(),
                ..Default::default()
            }
            .into(),
        ),
    };
    let mut expected = before.clone();
    expected.attachment = MessageField::some(renewed);
    TestCase::<Heartbeat>::new()
        .given_state(before)
        .when(heartbeat("old_connector", 1, now, None))
        .then([event])
        .then_state(expected);
}

#[test]
fn heartbeat_replay_rejects_unknown_observations_or_loss_of_progress() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(2);
    let mut before = state(start);
    before.active_at = MessageField::some(timestamp_from_datetime(&start));
    for mutation in 0..3 {
        let mut recorded = proto::PeerHeartbeatRecorded {
            attachment: MessageField::some(attachment(&identity(), "old_connector", 1, now)),
            availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
            active_at: before.active_at.clone(),
            ..Default::default()
        };
        match mutation {
            0 => recorded.availability = Some(buffa::EnumValue::from(91)),
            1 => recorded.condition = Some(buffa::EnumValue::from(91)),
            _ => recorded.active_at = MessageField::default(),
        }
        assert!(
            trogon_peer::commands::state::evolve(
                before.clone(),
                &proto::PeerEvent {
                    event: Some(recorded.into())
                }
            )
            .is_err()
        );
    }
}

#[test]
fn a_heartbeat_requires_enrollment_history_before_it_can_renew_a_lease() {
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    TestCase::<Heartbeat>::new()
        .given_no_history()
        .when(heartbeat("old_connector", 1, now, None))
        .then_error(HeartbeatError::MissingEnrollment);
}

#[test]
fn heartbeat_replay_cannot_replace_a_fence_or_accept_a_renewal_after_expiry() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    for (connector, generation, seconds) in [
        ("old_connector", 2, 2),
        ("other_connector", 1, 2),
        ("old_connector", 1, 10),
    ] {
        let event = proto::PeerEvent {
            event: Some(
                proto::PeerHeartbeatRecorded {
                    attachment: MessageField::some(attachment(
                        &identity(),
                        connector,
                        generation,
                        start + chrono::Duration::seconds(seconds),
                    )),
                    availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_BUSY.into()),
                    ..Default::default()
                }
                .into(),
            ),
        };
        assert!(matches!(
            trogon_peer::commands::state::evolve(state(start), &event),
            Err(trogon_peer::EvolveError::InvalidAttachment)
        ));
    }
}

#[test]
fn heartbeat_cannot_renew_a_valid_detached_attachment_even_with_the_matching_fence() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(2);
    let mut before = state(start);
    let detached = before.attachment.as_option_mut().unwrap();
    detached.status = Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED.into());
    detached.observed_at = MessageField::some(timestamp_from_datetime(&now));
    detached.expires_at = detached.observed_at.clone();
    TestCase::<Heartbeat>::new()
        .given_state(before)
        .when(heartbeat("old_connector", 1, now, None))
        .then_error(HeartbeatError::Inactive);
}
