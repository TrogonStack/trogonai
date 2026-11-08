#![allow(clippy::unwrap_used)]
mod support;
use support::*;
use trogon_decider::{Decider, WritePrecondition, evaluate_decision};
use trogon_peer::*;
use trogonai_proto::peers::{PeerEventCase, v1alpha1 as proto};

#[test]
fn acknowledgment_stops_replay_and_releases_recipient_quota_exactly_once() {
    let ids: Vec<_> = (0..64).collect();
    let state = run(inbox(&ids, 2048), &prepare(&[0, 1, 2, 3], 8192, 3));
    let state = run(state, &acknowledge("batch", 4));
    assert_eq!(AcknowledgeBatch::WRITE_PRECONDITION, WritePrecondition::StreamUnchanged);
    assert_eq!(SettleMessage::WRITE_PRECONDITION, WritePrecondition::StreamUnchanged);
    assert_eq!(state.batches[0].acknowledged_at.seconds, time(4).timestamp());
    assert_eq!(
        error(state.clone(), &acknowledge("batch", 5)),
        MessageError::AlreadyAcknowledged(Box::new(state.batches[0].clone()))
    );
    let mut state = state;
    for i in 64..68 {
        state = run(state, &intake(i, 2048));
    }
    assert_eq!(error(state.clone(), &intake(68, 1)), MessageError::DeferIntake);
    assert_eq!(error(state.clone(), &prepare(&[4], 20, 5)), MessageError::Conflict);
    let cmd = PrepareBatch::new(
        identity("recipient").owner().clone(),
        reference("recipient", 1),
        BatchId::new("next_batch").unwrap(),
        TrustedBatchPlan::from_renderer(vec![MessageId::new("message_4").unwrap()], 2048).unwrap(),
        time(5),
    );
    assert_eq!(run(state, &cmd).batches.len(), 2);
}
#[test]
fn acknowledgment_requires_assigned_batch_owner_live_current_attachment() {
    let state = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    assert_eq!(
        error(state.clone(), &acknowledge("foreign", 4)),
        MessageError::BatchNotFound
    );
    assert_eq!(
        error(state.clone(), &acknowledge("batch", 2)),
        MessageError::InvalidTransition
    );
    let foreign = AcknowledgeBatch::new(
        TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("foreign").unwrap()),
        reference("recipient", 1),
        BatchId::new("batch").unwrap(),
        time(4),
    );
    assert_eq!(
        error(state.clone(), &foreign),
        MessageError::Guard(HeartbeatError::OwnerMismatch)
    );
    assert_eq!(
        error(state, &acknowledge("batch", 10)),
        MessageError::Guard(HeartbeatError::Expired)
    );
}
#[test]
fn authoritative_settlement_retains_metadata_and_releases_sender_only_once() {
    for disposition in [
        None,
        Some(TerminalDisposition::Expired),
        Some(TerminalDisposition::Unavailable),
    ] {
        let recipient = match disposition {
            None => run(inbox(&[0], 2048), &prepare(&[0], 2048, 3)),
            Some(d) => run(
                enrolled("recipient", 0),
                &DisposeMessage::new(evidence(0, 2048), d, time(3601)),
            ),
        };
        let mut sender = enrolled("sender", 0);
        for i in 0..16 {
            let sec = if i < 10 { 1 } else { 61 };
            if i == 10 {
                sender.attachment = enrolled("sender", 61).attachment;
            }
            sender = run(sender, &send(i, sec, 2048));
        }
        let original = sender.outbox[0].clone();
        let sender = run(sender, &settle(&recipient, 0));
        assert_eq!(sender.outbox[0].message, original.message);
        assert_eq!(sender.outbox[0].semantic_digest, original.semantic_digest);
        assert_eq!(
            error(sender.clone(), &settle(&recipient, 0)),
            MessageError::AlreadySettled(Box::new(recipient.dispositions[0].clone()))
        );
        assert!(matches!(
            error(sender.clone(), &send(0, 62, 2048)),
            MessageError::AlreadyAccepted(_)
        ));
        assert_eq!(run(sender, &send(16, 62, 2048)).outbox.len(), 17);
    }
}
#[test]
fn settlement_does_not_erase_the_rolling_rate_window() {
    let recipient = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let mut sender = enrolled("sender", 0);
    for i in 0..10 {
        sender = run(sender, &send(i, 1, 1));
    }
    sender = run(sender, &settle(&recipient, 0));
    assert_eq!(error(sender, &send(10, 4, 1)), MessageError::RateLimited);
}
#[test]
fn conflicting_receipt_cannot_replace_original_outcome_or_release_again() {
    let prepared = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let expired = run(
        enrolled("recipient", 0),
        &DisposeMessage::new(evidence(0, 1), TerminalDisposition::Expired, time(3601)),
    );
    let sender = run(run(enrolled("sender", 0), &send(0, 1, 1)), &settle(&prepared, 0));
    assert_eq!(error(sender, &settle(&expired, 0)), MessageError::Conflict);
    let other = run(inbox(&[1], 1), &prepare(&[1], 20, 3));
    assert_eq!(
        error(run(enrolled("sender", 0), &send(0, 1, 1)), &settle(&other, 1)),
        MessageError::MessageNotFound
    );
}
#[test]
fn same_session_replacement_preserves_inbox_retry_and_fences_old_effects() {
    let sender = run(enrolled("sender", 0), &send(0, 1, 1));
    let sender = run(
        sender,
        &Detach::new(identity("sender").owner().clone(), reference("sender", 1), time(2)),
    );
    let sender = run(
        sender,
        &Attach::new(
            identity("sender").clone(),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            time(3),
        ),
    );
    assert_eq!(
        error(sender.clone(), &send(0, 4, 1)),
        MessageError::Guard(HeartbeatError::Fenced)
    );
    let retry = SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 2),
        IdempotencyKey::new("key_0").unwrap(),
        MessageId::new("new").unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(1, 1),
        None,
        time(4),
    );
    assert!(matches!(error(sender, &retry), MessageError::AlreadyAccepted(_)));
    let recipient = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let original = recipient.batches[0].clone();
    let recipient = run(
        recipient,
        &Detach::new(
            identity("recipient").owner().clone(),
            reference("recipient", 1),
            time(4),
        ),
    );
    let recipient = run(
        recipient,
        &Attach::new(
            identity("recipient").clone(),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            time(5),
        ),
    );
    assert_eq!(
        error(recipient.clone(), &acknowledge("batch", 6)),
        MessageError::Guard(HeartbeatError::Fenced)
    );
    assert_eq!(recipient.batches[0], original);
    let ack = AcknowledgeBatch::new(
        identity("recipient").owner().clone(),
        reference("recipient", 2),
        BatchId::new("batch").unwrap(),
        time(6),
    );
    assert!(run(recipient, &ack).batches[0].acknowledged_at.is_set());
}
#[test]
fn malformed_ack_and_settlement_events_or_internal_proofs_are_refused() {
    let before = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    for batch in ["foreign", ""] {
        assert!(
            commands::state::evolve(
                before.clone(),
                &proto::PeerEvent {
                    event: Some(
                        proto::PeerBatchAcknowledged {
                            batch_id: Some(batch.into()),
                            acknowledged_at: buffa::MessageField::some(
                                trogonai_proto::convert::timestamp_from_datetime(&time(4))
                            )
                        }
                        .into()
                    )
                }
            )
            .is_err()
        );
    }
    let sender = run(enrolled("sender", 0), &send(0, 1, 1));
    let (_, events) = evaluate_decision(sender.clone(), &settle(&before, 0)).unwrap();
    for mutation in 0..6 {
        let mut event = events.iter().next().unwrap().clone();
        if let Some(PeerEventCase::PeerMessageSettled(ref mut settled)) = event.event {
            let mut proof = settled.receipt.as_option().unwrap().clone();
            match mutation {
                0 => proof.recipient_revision = Some(0),
                1 => proof.outcome = Some(buffa::EnumValue::from(99)),
                2 => proof.recipient_peer_id = Some(identity("foreign").peer_id().to_string()),
                3 => proof.message_id = Some("foreign".into()),
                4 => proof.batch_id = None,
                _ => {
                    proof.decided_at =
                        buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&time(3601)))
                }
            }
            settled.receipt = buffa::MessageField::some(proof.clone());
            let mut invalid_history = before.clone();
            invalid_history.dispositions[0] = proof;
            assert!(
                AuthoritativeReceipt::from_recipient_history(&invalid_history, &MessageId::new("message_0").unwrap())
                    .is_err()
            );
        }
        assert!(commands::state::evolve(sender.clone(), &event).is_err());
    }
}

#[test]
fn core761_routed_ack_and_authoritative_settlement_replay_preserve_message() {
    let before = run(inbox(&[0], 7), &prepare(&[0], 20, 3));
    let ack = acknowledge("batch", 4);
    assert_eq!(ack.stream_id(), identity("recipient").peer_id());
    assert_eq!(AcknowledgeBatch::initial_state(), commands::state::initial_state());
    let (acknowledged, events) = evaluate_decision(before.clone(), &ack).unwrap();
    let mut replayed = before;
    for event in events.iter() {
        replayed = commands::state::evolve(replayed, event).unwrap();
    }
    assert_eq!(replayed, acknowledged);
    let proof = receipt(&acknowledged, 0);
    assert_eq!(proof.tenant(), identity("recipient").owner().tenant());
    assert_eq!(proof.receipt(), &acknowledged.dispositions[0]);
    let expected = proof.receipt().clone();
    let settle = SettleMessage::new(identity("sender").peer_id().clone(), proof);
    assert_eq!(settle.stream_id(), identity("sender").peer_id());
    let initial = SettleMessage::initial_state();
    assert_eq!(initial, commands::state::initial_state());
    let enroll = Enroll::new(
        identity("sender"),
        ConnectorId::new("connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        PeerDisplayName::new("Peer").unwrap(),
        time(0),
    );
    let (_, events) = evaluate_decision(initial.clone(), &enroll).unwrap();
    let mut sender = initial;
    for event in events.iter() {
        sender = commands::state::evolve(sender, event).unwrap();
    }
    let (_, events) = evaluate_decision(sender.clone(), &send(0, 1, 7)).unwrap();
    for event in events.iter() {
        sender = commands::state::evolve(sender, event).unwrap();
    }
    let original_message = sender.outbox[0].message.clone();
    let (settled, events) = evaluate_decision(sender.clone(), &settle).unwrap();
    for event in events.iter() {
        sender = commands::state::evolve(sender, event).unwrap();
    }
    assert_eq!(sender, settled);
    assert_eq!(sender.outbox[0].message, original_message);
    assert_eq!(sender.outbox[0].settlement.as_option(), Some(&expected));
    assert_eq!(error(sender, &settle), MessageError::AlreadySettled(Box::new(expected)));
}

#[test]
fn core761_authority_rejects_missing_batch_membership_or_changed_preparation_clock() {
    let prepared = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let id = MessageId::new("message_0").unwrap();
    let mut missing_member = prepared.clone();
    missing_member.batches[0].message_ids.clear();
    assert!(matches!(
        AuthoritativeReceipt::from_recipient_history(&missing_member, &id),
        Err(MessageError::InvalidTransition)
    ));
    let mut malformed_batch_id = prepared.clone();
    malformed_batch_id.dispositions[0].batch_id = Some("invalid.batch".into());
    assert!(matches!(
        AuthoritativeReceipt::from_recipient_history(&malformed_batch_id, &id),
        Err(MessageError::InvalidState(EvolveError::InvalidValue {
            field: "receipt.batch_id",
            source: PeerValueError::InvalidCharacter
        }))
    ));
    let mut changed_clock = prepared;
    changed_clock.batches[0].prepared_at = trogonai_proto::convert::timestamp_from_datetime(&time(4)).into();
    assert!(matches!(
        AuthoritativeReceipt::from_recipient_history(&changed_clock, &id),
        Err(MessageError::InvalidTransition)
    ));
}

#[test]
fn core761_settlement_cannot_route_to_other_sender_or_import_foreign_tenant_receipt() {
    let local_sender = run(enrolled("sender", 0), &send(0, 1, 1));
    let local_recipient = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let wrong_route = SettleMessage::new(identity("recipient").peer_id().clone(), receipt(&local_recipient, 0));
    assert_eq!(
        error(local_sender.clone(), &wrong_route),
        MessageError::RecipientMismatch
    );
    let foreign_owner = TrustedPeerOwner::new(TenantId::new("foreign").unwrap(), CallerId::new("caller").unwrap());
    let foreign_sender = EnrollmentIdentity::new(
        foreign_owner.clone(),
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new("sender").unwrap(),
    );
    let foreign_recipient = EnrollmentIdentity::new(
        foreign_owner,
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new("recipient").unwrap(),
    );
    let enroll = |identity: EnrollmentIdentity| {
        Enroll::new(
            identity,
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            PeerDisplayName::new("Peer").unwrap(),
            time(0),
        )
    };
    let command = SendMessage::new(
        foreign_sender.owner().clone(),
        AttachmentRef::new(
            foreign_sender.peer_id().clone(),
            ConnectorId::new("connector").unwrap(),
            1,
        )
        .unwrap(),
        IdempotencyKey::new("foreign_key").unwrap(),
        MessageId::new("message_0").unwrap(),
        foreign_recipient.peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(1, 1),
        None,
        time(1),
    );
    let sender = run(run(commands::state::initial_state(), &enroll(foreign_sender)), &command);
    let accepted = AcceptedMessage::from_sender_history(&sender, &MessageId::new("message_0").unwrap()).unwrap();
    let recipient = run(
        run(commands::state::initial_state(), &enroll(foreign_recipient)),
        &DisposeMessage::new(accepted, TerminalDisposition::Unavailable, time(3)),
    );
    let proof = receipt(&recipient, 0);
    assert_eq!(proof.tenant().as_str(), "foreign");
    assert_eq!(
        error(
            local_sender,
            &SettleMessage::new(identity("sender").peer_id().clone(), proof)
        ),
        MessageError::TenantMismatch
    );
}

#[test]
fn core761_ack_and_settlement_replay_reject_duplicate_or_fenced_effects() {
    let prepared = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let (acknowledged, events) = evaluate_decision(prepared.clone(), &acknowledge("batch", 4)).unwrap();
    let ack = events.iter().next().unwrap();
    assert_eq!(
        commands::state::evolve(acknowledged, ack),
        Err(EvolveError::InvalidMessageTransition)
    );
    let detached = run(
        prepared.clone(),
        &Detach::new(
            identity("recipient").owner().clone(),
            reference("recipient", 1),
            time(4),
        ),
    );
    assert_eq!(
        commands::state::evolve(detached, ack),
        Err(EvolveError::InvalidMessageTransition)
    );
    let mut early = ack.clone();
    if let Some(PeerEventCase::PeerBatchAcknowledged(ref mut event)) = early.event {
        event.acknowledged_at = trogonai_proto::convert::timestamp_from_datetime(&time(2)).into();
    }
    assert_eq!(
        commands::state::evolve(prepared.clone(), &early),
        Err(EvolveError::InvalidMessageTransition)
    );
    let sender = run(enrolled("sender", 0), &send(0, 1, 1));
    let (settled, events) = evaluate_decision(sender.clone(), &settle(&prepared, 0)).unwrap();
    let event = events.iter().next().unwrap();
    assert_eq!(
        commands::state::evolve(settled, event),
        Err(EvolveError::InvalidMessageTransition)
    );
    let mut different_owner = sender;
    different_owner.enrollment = enrolled("different_sender", 0).enrollment;
    assert_eq!(
        commands::state::evolve(different_owner, event),
        Err(EvolveError::InvalidMessageTransition)
    );
}

#[test]
fn acknowledgment_rejects_a_leap_clock_before_serializing_receipt_time() {
    use chrono::TimeZone;
    let sender = run(enrolled("sender", -3), &send(0, -2, 1));
    let evidence = AcceptedMessage::from_sender_history(&sender, &MessageId::new("message_0").unwrap()).unwrap();
    let recipient = run(enrolled("recipient", -3), &IntakeMessage::new(evidence, time(-1)));
    let prepared = run(recipient, &prepare(&[0], 20, -1));
    let leap = chrono::Utc.timestamp_opt(time(-1).timestamp(), 1_500_000_000).single().unwrap();
    let command = AcknowledgeBatch::new(identity("recipient").owner().clone(), reference("recipient", 1), BatchId::new("batch").unwrap(), leap);
    assert_eq!(error(prepared.clone(), &command), MessageError::InvalidState(EvolveError::TimestampRange { field: "acknowledged_at" }));
    let ordinary = run(prepared, &acknowledge("batch", 0));
    assert_eq!(ordinary.batches[0].acknowledged_at.seconds, time(0).timestamp());
    assert_eq!(ordinary.batches[0].acknowledged_at.nanos, 0);
}
