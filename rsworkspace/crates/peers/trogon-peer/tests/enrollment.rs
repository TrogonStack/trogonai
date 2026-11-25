#![allow(clippy::unwrap_used)]

use buffa::MessageField;
use chrono::{TimeZone, Utc};
use trogon_decider::{Decider, evaluate_decision, testing::TestCase};
use trogon_peer::{
    CallerId, CollaborationRef, ConnectorId, Enroll, EnrollError, EnrollmentIdentity, EvolveError, InstallationId,
    NativeSessionId, PeerDisplayName, TenantId, TrustedPeerOwner,
};
use trogonai_proto::convert::timestamp_from_datetime;
use trogonai_proto::peers::{state_v1alpha1 as state, v1alpha1 as proto};

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
        attachment_ref: MessageField::some(proto::AttachmentRef {
            peer_id: Some(peer_id.to_owned()),
            connector_id: Some("connector_alpha".to_owned()),
            generation: Some(1),
        }),
        status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED.into()),
        observed_at: MessageField::some(timestamp_from_datetime(&now)),
        expires_at: MessageField::some(timestamp_from_datetime(&(now + chrono::Duration::seconds(10)))),
    };

    let expected_payload = state::PeerState {
        enrollment: MessageField::some(enrollment.clone()),
        attachment: MessageField::some(attachment.clone()),
        availability: Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into()),
        next_intake_ordinal: Some(1),
        ..Default::default()
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
        .then_state(expected_payload.clone().into());
    assert_eq!(result.stream_id(), peer_id);
    assert!(!result.resulting_state().active_at.is_set());
    type DomainState = <Enroll as Decider>::State;
    type Payload = <DomainState as trogon_decider::StateSnapshot>::Payload;
    let initial = DomainState::default();
    let expected_initial = Payload {
        next_intake_ordinal: Some(1),
        ..Default::default()
    };
    assert_eq!(initial.as_ref(), &expected_initial);
    assert_eq!(
        buffa::Message::encode_to_vec(initial.as_ref()),
        buffa::Message::encode_to_vec(&expected_initial)
    );
    assert_ne!(
        buffa::Message::encode_to_vec(initial.as_ref()),
        buffa::Message::encode_to_vec(&Payload::default())
    );
    let mut replayed = initial;
    for event in result.then_events() {
        replayed = trogon_decider::Evolve::evolve(replayed, event).unwrap();
    }
    assert_eq!(&replayed, result.resulting_state());
    let payload: Payload = replayed.clone().into();
    assert_eq!(&payload, result.resulting_state().as_ref());
    let bytes = buffa::Message::encode_to_vec(&payload);
    assert_eq!(bytes, buffa::Message::encode_to_vec(&expected_payload));
    let decoded = <Payload as buffa::Message>::decode_from_slice(&bytes).unwrap();
    let restored = DomainState::from(decoded);
    assert_eq!(restored, replayed);
    assert_eq!(buffa::Message::encode_to_vec(restored.as_ref()), bytes);
    assert!(matches!(
        trogon_decider::Evolve::evolve(restored.clone(), &result.then_events()[0]),
        Err(trogon_peer::EvolveError::AlreadyEnrolled)
    ));
    assert!(matches!(
        trogon_decider::Evolve::evolve(restored, &proto::PeerEvent::default()),
        Err(trogon_peer::EvolveError::MissingEvent)
    ));
}

#[test]
fn an_enrolled_identity_cannot_be_created_twice_by_command_or_replayed_fact() {
    let identity = EnrollmentIdentity::new(
        TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("caller").unwrap()),
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new("session").unwrap(),
    );
    let command = Enroll::new(
        identity,
        ConnectorId::new("connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        PeerDisplayName::new("Peer").unwrap(),
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    );
    let (state, _) = evaluate_decision(trogon_peer::commands::state::initial_state(), &command).unwrap();
    TestCase::<Enroll>::new()
        .given_state(state.clone())
        .when(command)
        .then_error(EnrollError::AlreadyEnrolled);
    let duplicate = proto::PeerEvent {
        event: Some(
            proto::PeerEnrolled {
                enrollment: state.enrollment.clone(),
            }
            .into(),
        ),
    };
    assert_eq!(
        trogon_peer::commands::state::evolve(state, &duplicate),
        Err(EvolveError::AlreadyEnrolled)
    );
}
