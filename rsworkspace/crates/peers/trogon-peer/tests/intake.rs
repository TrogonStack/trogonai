#![allow(clippy::unwrap_used)]
mod support;
use support::*;
use trogon_peer::*;
use trogonai_proto::peers::v1alpha1 as proto;

#[test]
fn intake_is_durable_ordered_and_independent_of_online_attachment() {
    let state = run(enrolled("recipient", 0), &intake(3, 2048));
    let state = run(state, &IntakeMessage::new(evidence(1, 1), time(11)));
    assert_eq!(state.inbox[0].message.message_id.as_deref(), Some("message_3"));
    assert_eq!(state.inbox[1].intake_ordinal, Some(2));
    assert_eq!(state.next_intake_ordinal, Some(3));
    assert_eq!(
        error(state.clone(), &intake(3, 2048)),
        MessageError::AlreadyIntaken(Box::new(state.inbox[0].clone()))
    );
    assert_eq!(error(state, &intake(3, 1)), MessageError::Conflict);
}
#[test]
fn pending_sixty_four_and_128_kib_defer_without_sender_settlement() {
    let mut state = enrolled("recipient", 0);
    for i in 0..64 {
        state = run(state, &intake(i, 2048));
    }
    assert_eq!(error(state.clone(), &intake(64, 1)), MessageError::DeferIntake);
    assert!(matches!(
        error(state.clone(), &intake(0, 2048)),
        MessageError::AlreadyIntaken(_)
    ));
    state = run(
        state,
        &DisposeMessage::new(evidence(0, 2048), TerminalDisposition::Unavailable, time(3)),
    );
    let state = run(state, &intake(64, 2048));
    assert_eq!(state.inbox.len(), 65);
    assert_eq!(state.dispositions.len(), 1);
}
#[test]
fn recipient_collaboration_and_tenant_must_match_accepted_evidence() {
    assert_eq!(
        error(enrolled("foreign", 0), &intake(0, 1)),
        MessageError::RecipientMismatch
    );
    let mut state = enrolled("recipient", 0);
    let mut enrollment = state.enrollment.as_option().unwrap().clone();
    enrollment.collaboration_ref = Some("foreign".into());
    state.enrollment = buffa::MessageField::some(enrollment);
    assert_eq!(error(state, &intake(0, 1)), MessageError::CollaborationMismatch);
    let mut sender = run(enrolled("sender", 0), &send(0, 1, 1));
    let mut entry = sender.outbox[0].clone();
    entry.semantic_digest = Some(vec![0; 32]);
    sender.outbox[0] = entry;
    assert!(AcceptedMessage::from_sender_history(&sender, &MessageId::new("message_0").unwrap()).is_err());
}
#[test]
fn expiry_cutoff_tombstones_before_intake_and_never_rewrites_outcome() {
    let cmd = DisposeMessage::new(evidence(0, 1), TerminalDisposition::Expired, time(3601));
    assert_eq!(
        error(
            enrolled("recipient", 0),
            &DisposeMessage::new(evidence(0, 1), TerminalDisposition::Expired, time(3600))
        ),
        MessageError::InvalidTransition
    );
    assert_eq!(
        error(
            enrolled("recipient", 0),
            &IntakeMessage::new(evidence(0, 1), time(3601))
        ),
        MessageError::RequiresDisposition
    );
    let state = run(enrolled("recipient", 0), &cmd);
    assert!(state.inbox.is_empty());
    assert_eq!(state.dispositions[0].recipient_revision, Some(1));
    for other in [
        cmd,
        DisposeMessage::new(evidence(0, 1), TerminalDisposition::Unavailable, time(4000)),
    ] {
        assert_eq!(
            error(state.clone(), &other),
            MessageError::AlreadyDisposed(Box::new(state.dispositions[0].clone()))
        );
    }
    assert!(matches!(error(state, &intake(0, 1)), MessageError::AlreadyDisposed(_)));
}
#[test]
fn malformed_intake_and_receipt_history_is_refused() {
    let state = run(enrolled("recipient", 0), &intake(0, 1));
    for ordinal in [None, Some(0), Some(2)] {
        let mut entry = state.inbox[0].clone();
        entry.intake_ordinal = ordinal;
        assert!(
            commands::state::evolve(
                enrolled("recipient", 0),
                &proto::PeerEvent {
                    event: Some(
                        proto::PeerMessageIntakeRecorded {
                            entry: buffa::MessageField::some(entry)
                        }
                        .into()
                    )
                }
            )
            .is_err()
        );
    }
    let disposed = run(
        state.clone(),
        &DisposeMessage::new(evidence(0, 1), TerminalDisposition::Unavailable, time(3)),
    );
    for mutation in 0..6 {
        let mut receipt = disposed.dispositions[0].clone();
        match mutation {
            0 => receipt.outcome = Some(buffa::EnumValue::from(99)),
            1 => receipt.outcome = Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNSPECIFIED.into()),
            2 => receipt.recipient_revision = Some(0),
            3 => receipt.recipient_revision = Some(2),
            4 => receipt.batch_id = Some("unexpected".into()),
            _ => receipt.recipient_peer_id = Some(identity("foreign").peer_id().to_string()),
        }
        assert!(
            commands::state::evolve(
                state.clone(),
                &proto::PeerEvent {
                    event: Some(
                        proto::PeerMessageDisposed {
                            receipt: buffa::MessageField::some(receipt)
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
fn intake_binding_uses_the_recipient_from_trusted_sender_history() {
    use trogon_decider::{Decider, testing::TestCase};
    let command = intake(0, 17);
    assert_eq!(command.evidence().tenant(), identity("sender").owner().tenant());
    assert_eq!(command.evidence().metadata().content_size_bytes, Some(17));
    assert_eq!(
        command.evidence().metadata().recipient_peer_id.as_deref(),
        Some(command.stream_id().as_str())
    );
    TestCase::<IntakeMessage>::new()
        .given_no_history()
        .when(command)
        .then_error(MessageError::InvalidState(EvolveError::MissingEnrollment));
}

#[test]
fn intake_rejects_foreign_tenant_evidence_even_when_it_targets_this_peer() {
    let foreign = EnrollmentIdentity::new(
        TrustedPeerOwner::new(
            TenantId::new("foreign_tenant").unwrap(),
            CallerId::new("caller").unwrap(),
        ),
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new("sender").unwrap(),
    );
    let sender = run(
        commands::state::initial_state(),
        &Enroll::new(
            foreign.clone(),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            PeerDisplayName::new("Peer").unwrap(),
            time(0),
        ),
    );
    let sender = run(
        sender,
        &SendMessage::new(
            foreign.owner().clone(),
            AttachmentRef::new(foreign.peer_id().clone(), ConnectorId::new("connector").unwrap(), 1).unwrap(),
            IdempotencyKey::new("key_0").unwrap(),
            MessageId::new("message_0").unwrap(),
            identity("recipient").peer_id().clone(),
            CollaborationRef::new("collaboration").unwrap(),
            content(1, 1),
            None,
            time(1),
        ),
    );
    let accepted = AcceptedMessage::from_sender_history(&sender, &MessageId::new("message_0").unwrap()).unwrap();
    assert_eq!(
        error(enrolled("recipient", 0), &IntakeMessage::new(accepted, time(2))),
        MessageError::TenantMismatch
    );
}

#[test]
fn intake_requires_valid_service_time_after_acceptance_and_consistent_sender_history() {
    for at in [time(0), chrono::DateTime::<chrono::Utc>::MAX_UTC] {
        assert_eq!(
            error(enrolled("recipient", 0), &IntakeMessage::new(evidence(0, 1), at)),
            MessageError::InvalidTransition
        );
    }
    let mut forged_history = enrolled("foreign_sender", 0);
    forged_history.outbox = run(enrolled("sender", 0), &send(0, 1, 1)).outbox;
    assert!(matches!(
        AcceptedMessage::from_sender_history(&forged_history, &MessageId::new("message_0").unwrap()),
        Err(MessageError::InvalidTransition)
    ));
}

#[test]
fn disposal_binding_cannot_override_different_metadata_already_in_the_inbox() {
    use trogon_decider::{Decider, testing::TestCase};
    let command = DisposeMessage::new(evidence(0, 2), TerminalDisposition::Unavailable, time(3));
    assert_eq!(command.evidence().metadata().content_size_bytes, Some(2));
    assert_eq!(command.disposition(), TerminalDisposition::Unavailable);
    assert_eq!(command.stream_id(), identity("recipient").peer_id());
    assert_eq!(error(inbox(&[0], 1), &command), MessageError::Conflict);
    TestCase::<DisposeMessage>::new()
        .given_no_history()
        .when(command)
        .then_error(MessageError::InvalidState(EvolveError::MissingEnrollment));
}

#[test]
fn receipt_replay_rejects_invalid_identifiers_and_reversing_terminal_outcomes() {
    let before = inbox(&[0], 1);
    let disposed = run(
        before.clone(),
        &DisposeMessage::new(evidence(0, 1), TerminalDisposition::Unavailable, time(3)),
    );
    let event = |receipt| proto::PeerEvent {
        event: Some(
            proto::PeerMessageDisposed {
                receipt: buffa::MessageField::some(receipt),
            }
            .into(),
        ),
    };
    for (field, mutation) in [("receipt.message_id", 0), ("receipt.recipient_peer_id", 1)] {
        let mut receipt = disposed.dispositions[0].clone();
        if mutation == 0 {
            receipt.message_id = Some("invalid id".to_owned());
        } else {
            receipt.recipient_peer_id = Some("invalid".to_owned());
        }
        assert!(
            matches!(commands::state::evolve(before.clone(), &event(receipt)), Err(EvolveError::InvalidValue {field: actual, ..}) if actual == field)
        );
    }
    assert_eq!(
        commands::state::evolve(disposed.clone(), &event(disposed.dispositions[0].clone())),
        Err(EvolveError::InvalidMessageTransition)
    );
    for (outcome, seconds) in [
        (proto::PreparationOutcome::PREPARATION_OUTCOME_UNAVAILABLE, 0),
        (proto::PreparationOutcome::PREPARATION_OUTCOME_EXPIRED, 3600),
    ] {
        let mut receipt = disposed.dispositions[0].clone();
        receipt.outcome = Some(outcome.into());
        receipt.decided_at =
            buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&time(seconds)));
        assert_eq!(
            commands::state::evolve(before.clone(), &event(receipt)),
            Err(EvolveError::InvalidMessageTransition)
        );
    }
}

#[test]
fn intake_replay_cannot_duplicate_or_import_a_foreign_entry() {
    let state = inbox(&[0], 1);
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerMessageIntakeRecorded {
                entry: buffa::MessageField::some(state.inbox[0].clone()),
            }
            .into(),
        ),
    };
    for target in [state, enrolled("foreign_recipient", 0)] {
        assert_eq!(
            commands::state::evolve(target, &event),
            Err(EvolveError::InvalidMessageTransition)
        );
    }
    let mut corrupt = inbox(&[0], 1);
    let mut metadata = corrupt.inbox[0].message.as_option().unwrap().clone();
    metadata.message_id = Some("invalid id".to_owned());
    corrupt.inbox[0].message = buffa::MessageField::some(metadata);
    assert!(matches!(
        error(corrupt, &intake(1, 1)),
        MessageError::InvalidState(EvolveError::InvalidValue {
            field: "message_id",
            ..
        })
    ));
}

#[test]
fn replay_defends_recipient_capacity_without_trusting_a_command_side_check() {
    let indices: Vec<_> = (0..64).collect();
    let before = inbox(&indices, 2048);
    let mut entry = run(enrolled("recipient", 0), &intake(64, 1)).inbox.remove(0);
    entry.intake_ordinal = Some(65);
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerMessageIntakeRecorded {
                entry: buffa::MessageField::some(entry),
            }
            .into(),
        ),
    };
    assert_eq!(
        commands::state::evolve(before, &event),
        Err(EvolveError::InvalidMessageTransition)
    );
}

#[test]
fn receipt_replay_detects_missing_recipient_and_an_unindexed_inbox_disposition() {
    let before = inbox(&[0], 1);
    let disposed = run(
        before.clone(),
        &DisposeMessage::new(evidence(0, 1), TerminalDisposition::Unavailable, time(3)),
    );
    let mut receipt = disposed.dispositions[0].clone();
    receipt.recipient_peer_id = None;
    let event = |receipt| proto::PeerEvent {
        event: Some(
            proto::PeerMessageDisposed {
                receipt: buffa::MessageField::some(receipt),
            }
            .into(),
        ),
    };
    assert_eq!(
        commands::state::evolve(before.clone(), &event(receipt)),
        Err(EvolveError::MissingField {
            field: "receipt.recipient_peer_id"
        })
    );
    let mut inconsistent = before;
    inconsistent.inbox[0].disposition = disposed.inbox[0].disposition.clone();
    assert_eq!(
        commands::state::evolve(inconsistent, &event(disposed.dispositions[0].clone())),
        Err(EvolveError::InvalidMessageTransition)
    );
}
