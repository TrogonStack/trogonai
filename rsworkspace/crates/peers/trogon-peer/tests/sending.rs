#![allow(clippy::unwrap_used, clippy::panic)]
mod support;
use buffa::Message;
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
        TrustedContent::from_sealed_port(
            SealedContentRef::new(claim_check("other_location", b"new stored representation")).unwrap(),
            3,
            vec![1; 32],
        )
        .unwrap(),
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
        assert!(
            TrustedContent::from_sealed_port(
                SealedContentRef::new(claim_check("sealed", b"stored fixture")).unwrap(),
                size,
                vec![1; 32]
            )
            .is_err()
        );
    }
    assert!(
        TrustedContent::from_sealed_port(
            SealedContentRef::new(claim_check("sealed", b"stored fixture")).unwrap(),
            1,
            vec![1; 31]
        )
        .is_err()
    );
    for value in ["", "contains space", "line\nbreak"] {
        assert!(MessageId::new(value).is_err());
        assert!(IdempotencyKey::new(value).is_err());
        assert!(SealedContentRef::new(claim_check(value, b"stored fixture")).is_err());
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
            3 => metadata.semantic_fingerprint = proto::ContentSemanticFingerprint { value: Some(vec![]) }.into(),
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

fn accepted_event(entry: proto::OutboxEntry) -> proto::PeerEvent {
    proto::PeerEvent {
        event: Some(proto::PeerMessageAccepted { entry: entry.into() }.into()),
    }
}

#[test]
fn acceptance_replay_reports_the_missing_or_invalid_required_identity_field() {
    let accepted = run(enrolled("sender", 0), &send(0, 1, 3)).outbox[0].clone();
    type MetadataMutation = (fn(&mut proto::MessageMetadata), &'static str);
    let missing: [MetadataMutation; 4] = [
        (|metadata| metadata.message_id = None, "message_id"),
        (|metadata| metadata.sender_peer_id = None, "sender_peer_id"),
        (|metadata| metadata.recipient_peer_id = None, "recipient_peer_id"),
        (|metadata| metadata.collaboration_ref = None, "collaboration_ref"),
    ];
    for (remove, field) in missing {
        let mut entry = accepted.clone();
        remove(entry.message.as_option_mut().unwrap());
        assert!(
            matches!(commands::state::evolve(enrolled("sender", 0), &accepted_event(entry)),
            Err(EvolveError::MissingField { field: actual }) if actual == field)
        );
    }
    let invalid: [MetadataMutation; 3] = [
        (
            |metadata| metadata.message_id = Some("contains space".into()),
            "message_id",
        ),
        (
            |metadata| metadata.sender_peer_id = Some("bad".into()),
            "sender_peer_id",
        ),
        (
            |metadata| metadata.collaboration_ref = Some("contains\ncontrol".into()),
            "collaboration_ref",
        ),
    ];
    for (mutate, field) in invalid {
        let mut entry = accepted.clone();
        mutate(entry.message.as_option_mut().unwrap());
        assert!(
            matches!(commands::state::evolve(enrolled("sender", 0), &accepted_event(entry)),
            Err(EvolveError::InvalidValue { field: actual, .. }) if actual == field)
        );
    }
    let mut no_message = accepted;
    no_message.message = Default::default();
    assert!(matches!(
        commands::state::evolve(enrolled("sender", 0), &accepted_event(no_message)),
        Err(EvolveError::MissingField { field: "message" })
    ));
}

#[test]
fn acceptance_replay_rejects_duplicate_and_foreign_sender_receipts_with_valid_semantic_hashes() {
    let state = run(enrolled("sender", 0), &send(0, 1, 3));
    let duplicate = accepted_event(state.outbox[0].clone());
    assert!(matches!(
        commands::state::evolve(state, &duplicate),
        Err(EvolveError::InvalidMessageTransition)
    ));
    let foreign_command = SendMessage::new(
        identity("foreign").owner().clone(),
        reference("foreign", 1),
        IdempotencyKey::new("foreign_key").unwrap(),
        MessageId::new("foreign_message").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(3, 1),
        None,
        time(1),
    );
    let foreign = run(enrolled("foreign", 0), &foreign_command).outbox[0].clone();
    assert!(matches!(
        commands::state::evolve(enrolled("sender", 0), &accepted_event(foreign)),
        Err(EvolveError::InvalidMessageTransition)
    ));
}

#[test]
fn acceptance_replay_requires_the_acceptance_time_to_belong_to_the_current_lease() {
    let expired = run(enrolled("sender", 10), &send(1, 11, 3)).outbox[0].clone();
    assert!(matches!(
        commands::state::evolve(enrolled("sender", 0), &accepted_event(expired)),
        Err(EvolveError::InvalidMessageTransition)
    ));
    let earlier = run(enrolled("sender", 0), &send(2, 1, 3)).outbox[0].clone();
    assert!(matches!(
        commands::state::evolve(enrolled("sender", 2), &accepted_event(earlier)),
        Err(EvolveError::InvalidMessageTransition)
    ));
}

#[test]
fn acceptance_replay_rejects_future_ordering_and_the_eleventh_recent_receipt() {
    let later = run(enrolled("sender", 0), &send(0, 2, 3));
    let earlier_entry = run(enrolled("sender", 0), &send(1, 1, 3)).outbox[0].clone();
    assert!(matches!(
        commands::state::evolve(later, &accepted_event(earlier_entry)),
        Err(EvolveError::InvalidMessageTransition)
    ));
    let mut recent = enrolled("sender", 0);
    for index in 0..10 {
        recent = run(recent, &send(index, 1, 3));
    }
    let eleventh = run(enrolled("sender", 0), &send(10, 2, 3)).outbox[0].clone();
    assert!(matches!(
        commands::state::evolve(recent, &accepted_event(eleventh)),
        Err(EvolveError::InvalidMessageTransition)
    ));
}

#[test]
fn new_send_rejects_foreign_collaboration_reused_message_identity_and_future_history() {
    let foreign_collaboration = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        IdempotencyKey::new("new_key").unwrap(),
        MessageId::new("new_message").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("other").unwrap(),
        content(3, 1),
        None,
        time(1),
    );
    assert_eq!(
        error(enrolled("sender", 0), &foreign_collaboration),
        MessageError::CollaborationMismatch
    );
    let state = run(enrolled("sender", 0), &send(0, 1, 3));
    let reused_id = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        IdempotencyKey::new("new_key").unwrap(),
        MessageId::new("message_0").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(3, 1),
        None,
        time(2),
    );
    assert_eq!(error(state, &reused_id), MessageError::Conflict);
    let future = run(enrolled("sender", 0), &send(0, 2, 3));
    assert_eq!(
        error(future, &send(1, 1, 3)),
        MessageError::Guard(HeartbeatError::ClockRegression)
    );
}

#[test]
fn batch_correlation_names_roundtrip_the_wire_carrier_and_reject_invalid_tokens() {
    let accepted = run(enrolled("sender", 0), &send(0, 1, 3));
    for name in ["batch_live".to_owned(), "b".repeat(128)] {
        let id = BatchId::new(name).unwrap();
        let batch = proto::PeerBatch {
            batch_id: Some(id.as_str().to_owned()),
            message_ids: vec![accepted.outbox[0].message.message_id.clone().unwrap()],
            prepared_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&time(2))),
            ..Default::default()
        };
        let decoded = proto::PeerBatch::decode_from_slice(&batch.encode_to_vec()).unwrap();
        assert_eq!(decoded.message_ids, batch.message_ids);
        assert_eq!(BatchId::new(decoded.batch_id.unwrap()).unwrap(), id);
    }
    for (invalid, expected) in [
        (String::new(), PeerValueError::Empty),
        ("b".repeat(129), PeerValueError::TooLong { maximum: 128 }),
        ("batch.with.dot".into(), PeerValueError::InvalidCharacter),
        ("batch\nline".into(), PeerValueError::InvalidCharacter),
    ] {
        assert_eq!(BatchId::new(invalid), Err(expected));
    }
}

#[test]
fn replay_rejects_malformed_retry_keys_before_accepting_the_message() {
    let accepted = run(enrolled("sender", 0), &send(0, 1, 3)).outbox[0].clone();
    for key in [None, Some(String::new()), Some("invalid.retry".to_owned())] {
        let mut entry = accepted.clone();
        entry.idempotency_key = key.clone();
        let error = commands::state::evolve(enrolled("sender", 0), &accepted_event(entry)).unwrap_err();
        match key {
            None => assert!(matches!(
                error,
                EvolveError::MissingField {
                    field: "idempotency_key"
                }
            )),
            Some(_) => assert!(matches!(
                error,
                EvolveError::InvalidValue {
                    field: "idempotency_key",
                    ..
                }
            )),
        }
    }
}

#[test]
fn send_binding_matches_the_metadata_committed_by_the_command() {
    let command = send(0, 1, 17);
    assert_eq!(command.owner(), identity("sender").owner());
    assert_eq!(command.attachment(), &reference("sender", 1));
    assert_eq!(command.stream_id(), command.attachment().peer_id());
    assert_eq!(command.recipient(), identity("recipient").peer_id());
    assert_eq!(command.collaboration().as_str(), "collaboration");
    let state = run(enrolled("sender", 0), &command);
    let message = &state.outbox[0].message;
    assert_eq!(
        message.content_ref.as_option(),
        Some(&command.content().reference().to_claim_check())
    );
    assert_eq!(message.content_size_bytes, Some(command.content().size()));
    assert_eq!(
        message.semantic_fingerprint.value.as_deref(),
        Some(command.content().semantic_fingerprint())
    );
}

#[test]
fn new_acceptance_rejects_wrong_scope_reused_message_id_and_backwards_outbox_time() {
    let state = run(enrolled("sender", 0), &send(0, 2, 1));
    let wrong_scope = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        IdempotencyKey::new("unused").unwrap(),
        MessageId::new("unused").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("foreign_scope").unwrap(),
        content(1, 1),
        None,
        time(3),
    );
    assert_eq!(error(state.clone(), &wrong_scope), MessageError::CollaborationMismatch);
    let reused_id = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        IdempotencyKey::new("unused").unwrap(),
        MessageId::new("message_0").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(1, 1),
        None,
        time(3),
    );
    assert_eq!(error(state.clone(), &reused_id), MessageError::Conflict);
    assert_eq!(
        error(state, &send(1, 1, 1)),
        MessageError::Guard(HeartbeatError::ClockRegression)
    );
}

fn acceptance(entry: proto::OutboxEntry) -> proto::PeerEvent {
    proto::PeerEvent {
        event: Some(
            proto::PeerMessageAccepted {
                entry: buffa::MessageField::some(entry),
            }
            .into(),
        ),
    }
}

#[test]
fn acceptance_replay_validates_each_identifier_before_semantic_digest() {
    let entry = run(enrolled("sender", 0), &send(0, 1, 1)).outbox.remove(0);
    for field in [
        "message_id",
        "sender_peer_id",
        "collaboration_ref",
        "content_ref",
        "idempotency_key",
    ] {
        let mut changed = entry.clone();
        let mut metadata = changed.message.as_option().unwrap().clone();
        match field {
            "message_id" => metadata.message_id = Some("invalid id".to_owned()),
            "sender_peer_id" => metadata.sender_peer_id = Some("invalid".to_owned()),
            "collaboration_ref" => metadata.collaboration_ref = Some("invalid\nreference".to_owned()),
            "content_ref" => {
                metadata
                    .content_ref
                    .as_option_mut()
                    .unwrap()
                    .storage_ref
                    .as_option_mut()
                    .unwrap()
                    .value = Some("invalid ref".to_owned())
            }
            _ => changed.idempotency_key = Some("invalid key".to_owned()),
        }
        changed.message = buffa::MessageField::some(metadata);
        assert!(
            matches!(trogon_peer::commands::state::evolve(enrolled("sender", 0), &acceptance(changed)), Err(EvolveError::InvalidValue {field: actual, ..}) if actual == field)
        );
    }
}

#[test]
fn intact_accepted_metadata_cannot_be_replayed_to_a_different_sender_or_lease() {
    let entry = run(enrolled("sender", 0), &send(0, 1, 1)).outbox.remove(0);
    assert_eq!(
        trogon_peer::commands::state::evolve(enrolled("other_sender", 0), &acceptance(entry.clone())),
        Err(EvolveError::InvalidMessageTransition)
    );
    let state = run(enrolled("sender", 0), &send(0, 1, 1));
    assert_eq!(
        trogon_peer::commands::state::evolve(state, &acceptance(entry.clone())),
        Err(EvolveError::InvalidMessageTransition)
    );
    assert_eq!(
        trogon_peer::commands::state::evolve(enrolled("sender", 2), &acceptance(entry.clone())),
        Err(EvolveError::InvalidMessageTransition)
    );
    let detached = run(
        enrolled("sender", 0),
        &Detach::new(identity("sender").owner().clone(), reference("sender", 1), time(0)),
    );
    assert_eq!(
        trogon_peer::commands::state::evolve(detached, &acceptance(entry)),
        Err(EvolveError::InvalidMessageTransition)
    );
}

#[test]
fn replay_enforces_acceptance_order_and_rate_even_for_otherwise_valid_facts() {
    let first = run(enrolled("sender", 0), &send(0, 2, 1));
    let earlier = run(enrolled("sender", 0), &send(1, 1, 1)).outbox.remove(0);
    assert_eq!(
        trogon_peer::commands::state::evolve(first, &acceptance(earlier)),
        Err(EvolveError::InvalidMessageTransition)
    );
    let state = (0..10).fold(enrolled("sender", 0), |state, i| run(state, &send(i, 1, 1)));
    let eleventh = run(enrolled("sender", 0), &send(10, 2, 1)).outbox.remove(0);
    assert_eq!(
        trogon_peer::commands::state::evolve(state, &acceptance(eleventh)),
        Err(EvolveError::InvalidMessageTransition)
    );
}

#[test]
fn acceptance_rejects_a_leap_clock_before_serializing_message_time() {
    let state = enrolled("sender", 58);
    let leap = chrono::Utc
        .timestamp_opt(time(59).timestamp(), 1_500_000_000)
        .single()
        .unwrap();
    let command = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        IdempotencyKey::new("leap_key").unwrap(),
        MessageId::new("leap_message").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(1, 1),
        None,
        leap,
    );
    assert_eq!(
        error(state.clone(), &command),
        MessageError::InvalidState(EvolveError::TimestampRange { field: "accepted_at" })
    );
    let ordinary = run(state, &send(0, 59, 1));
    assert_eq!(ordinary.outbox[0].message.accepted_at.seconds, time(59).timestamp());
    assert_eq!(ordinary.outbox[0].message.accepted_at.nanos, 0);
}
