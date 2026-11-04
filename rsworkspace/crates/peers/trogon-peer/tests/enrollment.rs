#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use chrono::{TimeZone, Utc};
use trogon_decider::{Decider, testing::TestCase};
use trogon_peer::{
    CallerId, CollaborationRef, ConnectorId, Enroll, EnrollmentIdentity, InstallationId, NativeSessionId,
    PeerDisplayName, TenantId, TrustedPeerOwner,
};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};

#[test]
fn fresh_enrollment_records_trusted_owner_and_a_ten_second_attachment() {
    let now = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
    let identity = EnrollmentIdentity::new(
        TrustedPeerOwner::new(
            TenantId::new("tenant_alpha").unwrap(),
            CallerId::new("caller_alpha").unwrap(),
        ),
        InstallationId::new("installation_alpha").unwrap(),
        NativeSessionId::new("session_alpha").unwrap(),
    );
    let command = Enroll::new(
        identity.clone(),
        ConnectorId::new("connector_alpha").unwrap(),
        CollaborationRef::new("collaboration_alpha").unwrap(),
        PeerDisplayName::new("Review").unwrap(),
        now,
    );
    assert_eq!(command.identity(), &identity);
    assert_eq!(command.stream_id(), identity.peer_id());
    assert_eq!(command.connector_id().as_str(), "connector_alpha");
    assert_eq!(command.collaboration().as_str(), "collaboration_alpha");
    assert_eq!(command.display_name().as_str(), "Review");
    assert_eq!(command.now(), now);
    let peer_id = "181a9e9b0706e955720fad504bf947d1752ae3ddae5406402d21146e69f55fac";
    let enrollment = proto::PeerEnrollment {
        peer_id: Some(peer_id.to_owned()),
        tenant_id: Some("tenant_alpha".to_owned()),
        caller_id: Some("caller_alpha".to_owned()),
        installation_id: Some("installation_alpha".to_owned()),
        native_session_id: Some("session_alpha".to_owned()),
        collaboration_ref: Some("collaboration_alpha".to_owned()),
        display_name: Some("Review".to_owned()),
        enrolled_at: MessageField::some(timestamp_from_datetime(&now)),
    };
    let attachment = proto::PeerAttachment {
        attachment: MessageField::some(proto::AttachmentRef {
            peer_id: Some(peer_id.to_owned()),
            connector_id: Some("connector_alpha".to_owned()),
            generation: Some(1),
        }),
        status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
        observed_at: MessageField::some(timestamp_from_datetime(&now)),
        expires_at: MessageField::some(timestamp_from_datetime(&(now + chrono::Duration::seconds(10)))),
    };

    let result = TestCase::<Enroll>::new()
        .given_no_history()
        .when(command)
        .then([
            proto::PeerEvent {
                event: Some(
                    proto::PeerEnrolled {
                        enrollment: MessageField::some(enrollment.clone()),
                    }
                    .into(),
                ),
            },
            proto::PeerEvent {
                event: Some(
                    proto::PeerAttached {
                        attachment: MessageField::some(attachment.clone()),
                    }
                    .into(),
                ),
            },
        ])
        .then_state(PeerState {
            enrollment: MessageField::some(enrollment),
            attachment: MessageField::some(attachment),
            availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
            next_intake_ordinal: Some(1),
            ..Default::default()
        });
    assert_eq!(result.stream_id(), peer_id);
    assert!(!result.resulting_state().active_at.is_set());
}
