#![allow(clippy::unwrap_used, clippy::panic)]
mod support;
use chrono::TimeZone;
use support::*;
use trogon_decider::{Decider, WritePrecondition};
use trogon_peer::*;
use trogonai_proto::peers::v1alpha1 as proto;

#[test]
fn acceptance_is_atomic_metadata_only_with_fixed_lifetime() {
    let state = run(enrolled("sender", 0), &send(0, 1, 2048));
    assert_eq!(SendMessage::WRITE_PRECONDITION, WritePrecondition::StreamUnchanged);
    let entry = &state.outbox[0];
    assert_eq!(entry.idempotency_key.as_deref(), Some("key_0"));
    assert_eq!(entry.semantic_digest.as_ref().unwrap().len(), 32);
    assert_eq!(entry.message.accepted_at.seconds, time(1).timestamp());
    assert_eq!(entry.message.expires_at.seconds, time(3601).timestamp());
    assert!(!entry.settlement.is_set());
}
#[test]
fn retry_ignores_proposed_identity_time_and_content_location() {
    let state = run(enrolled("sender", 0), &send(0, 1, 3));
    let retry = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        IdempotencyKey::new("key_0").unwrap(),
        MessageId::new("new_identity").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        TrustedContent::from_sealed_port(SealedContentRef::new("other_location").unwrap(), 3, vec![1; 32]).unwrap(),
        None,
        time(2),
    );
    assert_eq!(
        error(state.clone(), &retry),
        MessageError::AlreadyAccepted(Box::new(state.outbox[0].clone()))
    );
    for (recipient, collab, size, digest, reply) in [
        ("other", "collaboration", 3, 1, None),
        ("recipient", "other", 3, 1, None),
        ("recipient", "collaboration", 4, 1, None),
        ("recipient", "collaboration", 3, 2, None),
        (
            "recipient",
            "collaboration",
            3,
            1,
            Some(MessageId::new("reply").unwrap()),
        ),
    ] {
        let changed = SendMessage::new(
            identity("sender").owner().clone(),
            reference("sender", 1),
            IdempotencyKey::new("key_0").unwrap(),
            MessageId::new("new").unwrap(),
            identity(recipient).peer_id().clone(),
            CollaborationRef::new(collab).unwrap(),
            content(size, digest),
            reply,
            time(2),
        );
        assert_eq!(error(state.clone(), &changed), MessageError::Conflict);
    }
}
#[test]
fn ten_new_accepts_per_rolling_minute_with_exact_cutoff() {
    let mut state = enrolled("sender", 0);
    for i in 0..10 {
        state = run(state, &send(i, 1, 1));
    }
    assert_eq!(error(state.clone(), &send(10, 2, 1)), MessageError::RateLimited);
    assert!(matches!(
        error(state.clone(), &send(0, 2, 1)),
        MessageError::AlreadyAccepted(_)
    ));
    state.attachment = enrolled("sender", 61).attachment;
    assert_eq!(run(state, &send(10, 61, 1)).outbox.len(), 11);
}
#[test]
fn sixteen_unsettled_messages_and_thirty_two_kib_boundary() {
    let mut state = enrolled("sender", 0);
    for i in 0..16 {
        let seconds = if i < 10 { 1 } else { 61 };
        if i == 10 {
            state.attachment = enrolled("sender", 61).attachment;
        }
        state = run(state, &send(i, seconds, 2048));
    }
    assert_eq!(error(state.clone(), &send(16, 62, 1)), MessageError::SenderCapacity);
    assert!(matches!(
        error(state, &send(0, 62, 2048)),
        MessageError::AlreadyAccepted(_)
    ));
}
#[test]
fn owner_and_live_attachment_guard_even_exact_retries() {
    let state = run(enrolled("sender", 0), &send(0, 1, 1));
    assert_eq!(
        error(state.clone(), &send(0, 10, 1)),
        MessageError::Guard(HeartbeatError::Expired)
    );
    for (owner, attachment, expected) in [
        (
            identity("sender").owner().clone(),
            reference("sender", 2),
            HeartbeatError::Fenced,
        ),
        (
            TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("foreign").unwrap()),
            reference("sender", 1),
            HeartbeatError::OwnerMismatch,
        ),
    ] {
        let cmd = SendMessage::new(
            owner,
            attachment,
            IdempotencyKey::new("key_0").unwrap(),
            MessageId::new("unused").unwrap(),
            identity("recipient").peer_id().clone(),
            CollaborationRef::new("collaboration").unwrap(),
            content(1, 1),
            None,
            time(2),
        );
        assert_eq!(error(state.clone(), &cmd), MessageError::Guard(expected));
    }
}
#[test]
fn invalid_content_and_identifiers_are_refused_without_body_debug() {
    for size in [0, 2049, u32::MAX] {
        assert!(TrustedContent::from_sealed_port(SealedContentRef::new("sealed").unwrap(), size, vec![1; 32]).is_err());
    }
    assert!(TrustedContent::from_sealed_port(SealedContentRef::new("sealed").unwrap(), 1, vec![1; 31]).is_err());
    for value in ["", "contains space", "line\nbreak"] {
        assert!(MessageId::new(value).is_err());
        assert!(IdempotencyKey::new(value).is_err());
        assert!(SealedContentRef::new(value).is_err());
    }
}
#[test]
fn malformed_acceptance_events_are_rejected_on_replay() {
    let state = run(enrolled("sender", 0), &send(0, 1, 1));
    for mutation in 0..9 {
        let mut entry = state.outbox[0].clone();
        let mut metadata = entry.message.as_option().unwrap().clone();
        match mutation {
            0 => entry.idempotency_key = None,
            1 => entry.semantic_digest = Some(vec![0; 31]),
            2 => metadata.content_size_bytes = Some(2049),
            3 => metadata.content_digest = Some(vec![]),
            4 => metadata.expires_at = metadata.accepted_at.clone(),
            5 => metadata.sender_peer_id = Some(identity("foreign").peer_id().to_string()),
            6 => metadata.recipient_peer_id = Some("bad".into()),
            7 => metadata.reply_to_message_id = Some("bad reply".into()),
            _ => metadata.collaboration_ref = Some("other".into()),
        }
        entry.message = buffa::MessageField::some(metadata);
        assert!(
            commands::state::evolve(
                enrolled("sender", 0),
                &proto::PeerEvent {
                    event: Some(
                        proto::PeerMessageAccepted {
                            entry: buffa::MessageField::some(entry)
                        }
                        .into()
                    )
                }
            )
            .is_err()
        );
    }
}

#[test]
fn exact_retry_does_not_require_a_new_representable_lifetime() {
    let state = run(enrolled("sender", 0), &send(0, 1, 1));
    let original = state.outbox[0].clone();
    let now = chrono::Utc.with_ymd_and_hms(9999, 12, 31, 23, 59, 30).unwrap();
    let state = run(
        state,
        &Attach::new(
            identity("sender"),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            now,
        ),
    );
    let retry = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 2),
        IdempotencyKey::new("key_0").unwrap(),
        MessageId::new("replacement_proposal").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(1, 1),
        None,
        now + chrono::Duration::seconds(1),
    );
    trogon_decider::testing::TestCase::<SendMessage>::new()
        .given_state(state)
        .when(retry)
        .then_error(MessageError::AlreadyAccepted(Box::new(original)));
}
