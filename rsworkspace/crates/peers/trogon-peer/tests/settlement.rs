#![allow(clippy::unwrap_used)]
mod support;
use support::*;
use trogon_decider::{Decider, WritePrecondition, evaluate_decision, testing::TestCase};
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
fn acknowledgment_binding_routes_only_to_its_attachment_history() {
    let command = acknowledge("batch", 4);
    assert_eq!(command.owner(), identity("recipient").owner());
    assert_eq!(command.attachment(), &reference("recipient", 1));
    assert_eq!(command.stream_id(), command.attachment().peer_id());
    assert_eq!(command.batch_id().as_str(), "batch");
    TestCase::<AcknowledgeBatch>::new()
        .given_no_history()
        .when(command)
        .then_error(MessageError::Guard(HeartbeatError::MissingEnrollment));
}

#[test]
fn settlement_binding_rejects_another_sender_or_foreign_tenant_receipt() {
    let recipient = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let command = settle(&recipient, 0);
    assert_eq!(command.sender(), identity("sender").peer_id());
    assert_eq!(command.stream_id(), command.sender());
    assert_eq!(command.evidence().tenant(), identity("recipient").owner().tenant());
    assert_eq!(command.evidence().receipt(), &recipient.dispositions[0]);
    TestCase::<SettleMessage>::new()
        .given_no_history()
        .when(command)
        .then_error(MessageError::InvalidState(EvolveError::MissingEnrollment));
    assert_eq!(
        error(enrolled("other_sender", 0), &settle(&recipient, 0)),
        MessageError::RecipientMismatch
    );
    let foreign = EnrollmentIdentity::new(
        TrustedPeerOwner::new(
            TenantId::new("foreign_tenant").unwrap(),
            CallerId::new("caller").unwrap(),
        ),
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new("recipient").unwrap(),
    );
    let mut foreign_history = run(
        commands::state::initial_state(),
        &Enroll::new(
            foreign.clone(),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            PeerDisplayName::new("Peer").unwrap(),
            time(0),
        ),
    );
    let mut receipt = recipient.dispositions[0].clone();
    receipt.recipient_peer_id = Some(foreign.peer_id().to_string());
    receipt.outcome = Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE.into());
    receipt.batch_id = None;
    foreign_history = trogon_peer::commands::state::evolve(
        foreign_history,
        &proto::PeerEvent {
            event: Some(
                proto::PeerMessageDisposed {
                    receipt: buffa::MessageField::some(receipt),
                }
                .into(),
            ),
        },
    )
    .unwrap();
    let foreign_evidence =
        AuthoritativeReceipt::from_recipient_history(&foreign_history, &MessageId::new("message_0").unwrap()).unwrap();
    assert_eq!(
        error(
            run(enrolled("sender", 0), &send(0, 1, 1)),
            &SettleMessage::new(identity("sender").peer_id().clone(), foreign_evidence)
        ),
        MessageError::TenantMismatch
    );
}

#[test]
fn prepared_receipt_evidence_requires_matching_batch_membership_time_and_inbox() {
    let state = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    for mutation in 0..3 {
        let mut corrupt = state.clone();
        match mutation {
            0 => corrupt.batches[0].message_ids.clear(),
            1 => {
                corrupt.batches[0].prepared_at =
                    buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&time(4)))
            }
            _ => corrupt.inbox.clear(),
        }
        assert!(matches!(
            AuthoritativeReceipt::from_recipient_history(&corrupt, &MessageId::new("message_0").unwrap()),
            Err(MessageError::InvalidTransition)
        ));
    }
}

#[test]
fn replay_rejects_duplicate_settlement_and_acknowledgment_before_preparation() {
    let before = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let ack_event = |seconds| proto::PeerEvent {
        event: Some(
            proto::PeerBatchAcknowledged {
                batch_id: Some("batch".to_owned()),
                acknowledged_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&time(
                    seconds,
                ))),
            }
            .into(),
        ),
    };
    assert_eq!(
        commands::state::evolve(before.clone(), &ack_event(2)),
        Err(EvolveError::InvalidMessageTransition)
    );
    let acknowledged = run(before.clone(), &acknowledge("batch", 4));
    assert_eq!(
        commands::state::evolve(acknowledged, &ack_event(5)),
        Err(EvolveError::InvalidMessageTransition)
    );
    let sender = run(enrolled("sender", 0), &send(0, 1, 1));
    let (settled, events) = evaluate_decision(sender, &settle(&before, 0)).unwrap();
    assert_eq!(
        commands::state::evolve(settled, events.iter().next().unwrap()),
        Err(EvolveError::InvalidMessageTransition)
    );
}

#[test]
fn acknowledgment_replay_cannot_use_an_expired_or_detached_attachment() {
    let before = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let event = |seconds| proto::PeerEvent {
        event: Some(
            proto::PeerBatchAcknowledged {
                batch_id: Some("batch".to_owned()),
                acknowledged_at: buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&time(
                    seconds,
                ))),
            }
            .into(),
        ),
    };
    assert_eq!(
        commands::state::evolve(before.clone(), &event(10)),
        Err(EvolveError::InvalidMessageTransition)
    );
    let detached = run(
        before,
        &Detach::new(
            identity("recipient").owner().clone(),
            reference("recipient", 1),
            time(4),
        ),
    );
    assert_eq!(
        commands::state::evolve(detached, &event(5)),
        Err(EvolveError::InvalidMessageTransition)
    );
}
