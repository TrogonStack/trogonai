#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use chrono::{DateTime, TimeZone, Utc};
use trogon_decider::testing::TestCase;
use trogon_peer::{
    Attach, CallerId, CollaborationRef, ConnectorId, EnrollmentIdentity, InstallationId, NativeSessionId, TenantId,
    TrustedPeerOwner,
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
