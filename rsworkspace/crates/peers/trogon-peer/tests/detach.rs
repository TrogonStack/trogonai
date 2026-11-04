#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use chrono::{DateTime, TimeZone, Utc};
use trogon_decider::testing::TestCase;
use trogon_peer::{
    Attach, AttachmentRef, CallerId, CollaborationRef, ConnectorId, Detach, DetachError, EnrollmentIdentity, Heartbeat,
    HeartbeatError, InstallationId, NativeSessionId, PeerId, PeerObservation, TenantId, TrustedPeerOwner,
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
        attachment: MessageField::some(proto::AttachmentRef {
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

fn command(connector: &str, generation: u64, now: DateTime<Utc>) -> Detach {
    Detach::new(identity().owner().clone(), reference(connector, generation), now)
}
fn detached_state(mut before: PeerState, now: DateTime<Utc>) -> PeerState {
    let previous = before.attachment.as_option().unwrap();
    before.attachment = MessageField::some(proto::PeerAttachment {
        attachment: previous.attachment.clone(),
        status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED.into()),
        observed_at: MessageField::some(timestamp_from_datetime(&now)),
        expires_at: MessageField::some(timestamp_from_datetime(&now)),
    });
    before
}

#[test]
fn detach_retires_only_its_attachment_and_preserves_durable_peer_state() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(2);
    let before = state(start);
    TestCase::<Detach>::new()
        .given_state(before.clone())
        .when(command("old_connector", 1, now))
        .then([proto::PeerEvent {
            event: Some(
                proto::PeerDetached {
                    attachment: before.attachment.attachment.clone(),
                    detached_at: MessageField::some(timestamp_from_datetime(&now)),
                }
                .into(),
            ),
        }])
        .then_state(detached_state(before, now));
}

#[test]
fn detach_is_idempotent_and_old_connectors_cannot_detach_a_successor() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let now = start + chrono::Duration::seconds(2);
    TestCase::<Detach>::new()
        .given_state(detached_state(state(start), now))
        .when(command("old_connector", 1, now))
        .then_error(DetachError::AlreadyDetached);
    for (connector, generation) in [("old_connector", 2), ("other", 1)] {
        TestCase::<Detach>::new()
            .given_state(state(start))
            .when(command(connector, generation, now))
            .then_error(DetachError::Fenced);
    }
    let other = TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("other").unwrap());
    TestCase::<Detach>::new()
        .given_state(state(start))
        .when(Detach::new(other, reference("old_connector", 1), now))
        .then_error(DetachError::OwnerMismatch);
}

#[test]
fn detached_attachment_can_be_replaced_without_waiting_for_its_old_lease() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let at = start + chrono::Duration::seconds(2);
    let now = at + chrono::Duration::seconds(1);
    let before = state(start);
    let detached = TestCase::<Detach>::new()
        .given_state(before.clone())
        .when(command("old_connector", 1, at))
        .then([proto::PeerEvent {
            event: Some(
                proto::PeerDetached {
                    attachment: before.attachment.attachment.clone(),
                    detached_at: MessageField::some(timestamp_from_datetime(&at)),
                }
                .into(),
            ),
        }])
        .resulting_state()
        .clone();
    let next = attachment(&identity(), "new_connector", 2, now);
    let mut expected = detached.clone();
    expected.attachment = MessageField::some(next.clone());
    expected.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into());
    let reattached = TestCase::<Attach>::new()
        .given_state(detached.clone())
        .when(Attach::new(
            identity(),
            ConnectorId::new("new_connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            now,
        ))
        .then([proto::PeerEvent {
            event: Some(
                proto::PeerAttached {
                    attachment: MessageField::some(next),
                }
                .into(),
            ),
        }])
        .then_state(expected)
        .resulting_state()
        .clone();
    let heartbeat = Heartbeat::new(
        identity().owner().clone(),
        reference("old_connector", 1),
        PeerObservation::new(proto::PeerAvailability::PEER_AVAILABILITY_READY, None, None).unwrap(),
        now,
    );
    TestCase::<Heartbeat>::new()
        .given_state(detached)
        .when(heartbeat.clone())
        .then_error(HeartbeatError::Inactive);
    TestCase::<Detach>::new()
        .given_state(reattached.clone())
        .when(command("old_connector", 1, now))
        .then_error(DetachError::Fenced);
    TestCase::<Heartbeat>::new()
        .given_state(reattached)
        .when(heartbeat)
        .then_error(HeartbeatError::Fenced);
}

#[test]
fn detach_checks_service_time_but_can_retire_its_own_expired_lease() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    TestCase::<Detach>::new()
        .given_state(state(start))
        .when(command("old_connector", 1, start - chrono::Duration::seconds(1)))
        .then_error(DetachError::ClockRegression);
    let now = start + chrono::Duration::seconds(20);
    let before = state(start);
    TestCase::<Detach>::new()
        .given_state(before.clone())
        .when(command("old_connector", 1, now))
        .then([proto::PeerEvent {
            event: Some(
                proto::PeerDetached {
                    attachment: before.attachment.attachment.clone(),
                    detached_at: MessageField::some(timestamp_from_datetime(&now)),
                }
                .into(),
            ),
        }])
        .then_state(detached_state(before, now));
}

#[test]
fn detach_replay_rejects_another_generation_or_backwards_time() {
    let start = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let before = state(start);
    for mutation in 0..3 {
        let mut event = proto::PeerDetached {
            attachment: before.attachment.attachment.clone(),
            detached_at: MessageField::some(timestamp_from_datetime(&(start + chrono::Duration::seconds(1)))),
        };
        match mutation {
            0 => {
                event.attachment = MessageField::some(proto::AttachmentRef {
                    peer_id: Some(identity().peer_id().to_string()),
                    connector_id: Some("old_connector".to_owned()),
                    generation: Some(2),
                })
            }
            1 => {
                event.detached_at = MessageField::some(timestamp_from_datetime(&(start - chrono::Duration::seconds(1))))
            }
            _ => event.detached_at = MessageField::default(),
        }
        assert!(
            trogon_peer::commands::state::evolve(
                before.clone(),
                &proto::PeerEvent {
                    event: Some(event.into())
                }
            )
            .is_err()
        );
    }
}
