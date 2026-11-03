#![allow(clippy::unwrap_used)]
mod support;
use support::*;
use trogon_decider::{Decider, WritePrecondition, evaluate_decision};
use trogon_peer::*;
use trogonai_proto::peers::{PeerEventCase, v1alpha1 as proto};

#[test]
fn prepare_atomically_records_ordered_batch_and_monotonic_dispositions() {
    let (state, events) = evaluate_decision(inbox(&[3, 1, 2, 0, 4], 2048), &prepare(&[3, 1, 2, 0], 8192, 3)).unwrap();
    assert_eq!(PrepareBatch::WRITE_PRECONDITION, WritePrecondition::StreamUnchanged);
    assert_eq!(events.len(), 1);
    assert_eq!(
        state.batches[0].message_ids,
        vec!["message_3", "message_1", "message_2", "message_0"]
    );
    assert_eq!(state.dispositions.len(), 4);
    for (i, receipt) in state.dispositions.iter().enumerate() {
        assert_eq!(receipt.recipient_revision, Some(i as u64 + 1));
        assert_eq!(receipt.batch_id.as_deref(), Some("batch"));
        assert_eq!(
            receipt.outcome.and_then(|v| v.as_known()),
            Some(proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED)
        );
        assert_eq!(receipt.decided_at.seconds, time(3).timestamp());
    }
    assert!(!state.inbox[4].disposition.is_set());
}
#[test]
fn rendered_total_includes_provenance_and_warning_budget() {
    for (ids, bytes) in [
        (vec![0], 0),
        (vec![0], 8193),
        (vec![0, 1, 2, 3, 4], 8192),
        (vec![0, 0], 10),
        (vec![], 1),
    ] {
        assert!(
            TrustedBatchPlan::from_renderer(
                ids.into_iter()
                    .map(|i| MessageId::new(format!("message_{i}")).unwrap())
                    .collect(),
                bytes
            )
            .is_err()
        );
    }
    assert_eq!(
        error(inbox(&[0], 2048), &prepare(&[0], 2047, 3)),
        MessageError::InvalidPlan
    );
    assert_eq!(run(inbox(&[0], 2048), &prepare(&[0], 8192, 3)).batches.len(), 1);
}
#[test]
fn rejects_skipped_foreign_duplicate_and_reordered_selection() {
    for ids in [&[1][..], &[1, 0][..], &[0, 9][..]] {
        assert_eq!(
            error(inbox(&[0, 1], 1), &prepare(ids, 20, 3)),
            MessageError::InvalidPlan
        );
    }
}
#[test]
fn oldest_unacknowledged_batch_replays_original_identity_and_time_after_expiry() {
    let mut state = run(inbox(&[0, 1], 1), &prepare(&[0], 50, 3));
    let original = state.batches[0].clone();
    state.attachment = enrolled("recipient", 4000).attachment;
    assert_eq!(
        error(state.clone(), &prepare(&[1], 60, 4001)),
        MessageError::AlreadyPrepared(Box::new(original))
    );
    assert!(matches!(
        error(
            state,
            &DisposeMessage::new(evidence(0, 1), TerminalDisposition::Expired, time(4002))
        ),
        MessageError::AlreadyDisposed(_)
    ));
}
#[test]
fn expiry_and_unavailable_first_outcomes_exclude_preparation() {
    for (disposition, seconds) in [
        (TerminalDisposition::Expired, 3601),
        (TerminalDisposition::Unavailable, 3),
    ] {
        let mut state = run(
            inbox(&[0, 1], 1),
            &DisposeMessage::new(evidence(0, 1), disposition, time(seconds)),
        );
        state.attachment = enrolled("recipient", seconds).attachment;
        assert_eq!(error(state, &prepare(&[0], 20, seconds)), MessageError::InvalidPlan);
    }
    let mut state = inbox(&[0], 1);
    state.attachment = enrolled("recipient", 3601).attachment;
    assert_eq!(error(state, &prepare(&[0], 20, 3601)), MessageError::InvalidPlan);
}
#[test]
fn recipient_owner_and_current_generation_guard_prepare() {
    let state = inbox(&[0], 1);
    let cmd = PrepareBatch::new(
        TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("foreign").unwrap()),
        reference("recipient", 1),
        BatchId::new("batch").unwrap(),
        TrustedBatchPlan::from_renderer(vec![MessageId::new("message_0").unwrap()], 20).unwrap(),
        time(3),
    );
    assert_eq!(
        error(state.clone(), &cmd),
        MessageError::Guard(HeartbeatError::OwnerMismatch)
    );
    assert_eq!(
        error(state, &prepare(&[0], 20, 10)),
        MessageError::Guard(HeartbeatError::Expired)
    );
}
#[test]
fn prepared_unacknowledged_work_keeps_recipient_quota_reserved() {
    let indices: Vec<_> = (0..64).collect();
    let state = run(inbox(&indices, 2048), &prepare(&[0, 1, 2, 3], 8192, 3));
    assert_eq!(error(state, &intake(64, 1)), MessageError::DeferIntake);
}
#[test]
fn malformed_prepared_batch_cannot_rewrite_winners() {
    let before = inbox(&[0, 1], 1);
    let (_, events) = evaluate_decision(before.clone(), &prepare(&[0, 1], 20, 3)).unwrap();
    let event = events.iter().next().unwrap();
    for mutation in 0..7 {
        let mut event = event.clone();
        if let Some(PeerEventCase::PeerBatchPrepared(ref mut prepared)) = event.event {
            let mut batch = prepared.batch.as_option().unwrap().clone();
            match mutation {
                0 => batch.message_ids.reverse(),
                1 => batch.message_ids.push("message_0".into()),
                2 => batch.acknowledged_at = batch.prepared_at.clone(),
                3 => prepared.receipts[0].recipient_revision = Some(3),
                4 => prepared.receipts[0].batch_id = Some("foreign".into()),
                5 => prepared.receipts[0].outcome = Some(buffa::EnumValue::from(91)),
                _ => {
                    prepared.receipts.pop();
                }
            }
            prepared.batch = buffa::MessageField::some(batch);
        }
        assert!(commands::state::evolve(before.clone(), &event).is_err());
    }
}

#[test]
fn renderer_plan_and_command_binding_match_the_persisted_batch() {
    use trogon_decider::testing::TestCase;
    let command = prepare(&[3, 1], 73, 3);
    assert_eq!(command.owner(), identity("recipient").owner());
    assert_eq!(command.attachment(), &reference("recipient", 1));
    assert_eq!(command.stream_id(), command.attachment().peer_id());
    assert_eq!(command.batch_id().as_str(), "batch");
    assert_eq!(command.plan().rendered_utf8_bytes(), 73);
    let state = run(inbox(&[3, 1], 2), &command);
    assert_eq!(
        state.batches[0].message_ids,
        command
            .plan()
            .message_ids()
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect::<Vec<_>>()
    );
    TestCase::<PrepareBatch>::new()
        .given_no_history()
        .when(command)
        .then_error(MessageError::Guard(HeartbeatError::MissingEnrollment));
}

#[test]
fn preparation_replay_rejects_missing_selection_bad_batch_identity_and_expired_lease() {
    let before = inbox(&[0], 1);
    let (_, events) = evaluate_decision(before.clone(), &prepare(&[0], 20, 3)).unwrap();
    for mutation in 0..4 {
        let mut event = events.iter().next().unwrap().clone();
        if let Some(PeerEventCase::PeerBatchPrepared(prepared)) = event.event.as_mut() {
            let mut batch = prepared.batch.as_option().unwrap().clone();
            match mutation {
                0 => batch.batch_id = Some("invalid batch".to_owned()),
                1 => {
                    batch.message_ids.clear();
                    prepared.receipts.clear();
                }
                2 => {
                    batch.prepared_at =
                        buffa::MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&time(10)));
                    prepared.receipts[0].decided_at = batch.prepared_at.clone();
                }
                _ => {
                    prepared.receipts[0].recipient_peer_id = Some("not_a_peer".to_owned());
                }
            }
            prepared.batch = buffa::MessageField::some(batch);
        }
        assert!(commands::state::evolve(before.clone(), &event).is_err());
    }
    let prepared = run(before.clone(), &prepare(&[0], 20, 3));
    assert_eq!(
        commands::state::evolve(prepared, events.iter().next().unwrap()),
        Err(EvolveError::InvalidMessageTransition)
    );
    let mut inconsistent = before;
    inconsistent.inbox[0].disposition = buffa::MessageField::some(proto::RecipientReceipt::default());
    assert_eq!(
        commands::state::evolve(inconsistent, events.iter().next().unwrap()),
        Err(EvolveError::InvalidMessageTransition)
    );
}

#[test]
fn preparation_refuses_a_receipt_with_invalid_batch_id_before_committing_it() {
    let state = run(inbox(&[0], 1), &prepare(&[0], 20, 3));
    let mut receipt = state.dispositions[0].clone();
    receipt.batch_id = Some("invalid batch".to_owned());
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerMessageDisposed {
                receipt: buffa::MessageField::some(receipt),
            }
            .into(),
        ),
    };
    assert!(matches!(
        commands::state::evolve(inbox(&[0], 1), &event),
        Err(EvolveError::InvalidValue {
            field: "receipt.batch_id",
            ..
        })
    ));
}

#[test]
fn prepared_outcome_cannot_arrive_through_a_terminal_disposal_event() {
    let before = inbox(&[0], 1);
    let prepared = run(before.clone(), &prepare(&[0], 20, 3));
    let event = proto::PeerEvent {
        event: Some(
            proto::PeerMessageDisposed {
                receipt: buffa::MessageField::some(prepared.dispositions[0].clone()),
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
fn preparation_rejects_corrupted_inbox_metadata_and_ambiguous_order() {
    for mutation in 0..3 {
        let mut state = inbox(&[0, 1], 1);
        match mutation {
            0 => {
                let mut metadata = state.inbox[0].message.as_option().unwrap().clone();
                metadata.content_size_bytes = Some(0);
                state.inbox[0].message = buffa::MessageField::some(metadata);
            }
            1 => state.inbox[0].intake_ordinal = Some(0),
            _ => state.inbox[1].intake_ordinal = state.inbox[0].intake_ordinal,
        }
        assert!(matches!(
            error(state, &prepare(&[0, 1], 20, 3)),
            MessageError::InvalidState(_)
        ));
    }
}
