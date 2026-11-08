#![allow(clippy::unwrap_used)]

mod support;
use buffa::Message;
use support::*;
use trogon_decider::{Decider, evaluate_decision};
use trogon_peer::*;
use trogonai_proto::peers::{PeerEventCase, state_v1alpha1::PeerState, v1alpha1 as proto};

fn assert_entries(history: &PeerHistory) {
    assert_eq!(history.inbox().len(), history.state().inbox.len());
    for (position, projected) in history.inbox().enumerate() {
        let entry = &history.state().inbox[position];
        let metadata = entry.message.as_option().unwrap();
        assert_eq!(projected.entry().encode_to_vec(), entry.encode_to_vec());
        assert_eq!(projected.metadata().encode_to_vec(), metadata.encode_to_vec());
        assert_eq!(Some(projected.message_id().as_str()), metadata.message_id.as_deref());
        assert_eq!(
            Some(&projected.content().reference().to_claim_check()),
            metadata.content_ref.as_option()
        );
        assert_eq!(Some(projected.content().size()), metadata.content_size_bytes);
        assert_eq!(
            Some(projected.content().semantic_fingerprint()),
            metadata.semantic_fingerprint.value.as_deref()
        );
        assert_eq!(
            projected.accepted_at(),
            trogonai_proto::convert::datetime_from_timestamp(metadata.accepted_at.as_option().unwrap()).unwrap()
        );
        assert_eq!(
            projected.expires_at(),
            trogonai_proto::convert::datetime_from_timestamp(metadata.expires_at.as_option().unwrap()).unwrap()
        );
    }
}

fn execute<C>(mut history: PeerHistory, command: &C) -> PeerHistory
where
    C: Decider<State = PeerState, Event = proto::PeerEvent>,
    C::DecideError: std::fmt::Debug,
    C::EvolveError: std::fmt::Debug,
{
    let (expected, events) = evaluate_decision(history.state().clone(), command).unwrap();
    for event in events.iter() {
        history = history.evolve(event).unwrap();
        assert_entries(&history);
    }
    assert_eq!(history.state().encode_to_vec(), expected.encode_to_vec());
    history
}

fn recipient() -> PeerHistory {
    let command = Enroll::new(
        identity("recipient"),
        ConnectorId::new("connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        PeerDisplayName::new("Peer").unwrap(),
        time(0),
    );
    execute(PeerHistory::initial(), &command)
}

fn with_inbox(count: usize) -> PeerHistory {
    (0..count).fold(recipient(), |history, index| execute(history, &intake(index, 7)))
}

#[test]
fn terminal_dispositions_replace_exact_first_middle_and_last_admitted_slots() {
    let mut history = with_inbox(3);
    for (index, seconds) in [(0, 3), (1, 4), (2, 5)] {
        history = execute(
            history,
            &DisposeMessage::new(evidence(index, 7), TerminalDisposition::Unavailable, time(seconds)),
        );
        assert_eq!(
            history
                .inbox()
                .map(|record| record.message_id().as_str())
                .collect::<Vec<_>>(),
            vec!["message_0", "message_1", "message_2"]
        );
        assert_eq!(
            history
                .inbox()
                .filter(|record| record.entry().disposition.is_set())
                .count(),
            index + 1
        );
    }
    assert_eq!(history.state().dispositions.len(), 3);
    assert_entries(&history);
}

#[test]
fn prepared_multiple_receipts_and_acknowledgement_keep_entire_inbox_records_synchronized() {
    let history = with_inbox(4);
    let history = execute(history, &prepare(&[0, 1, 2, 3], 40, 3));
    assert!(history.inbox().all(|record| record.entry().disposition.is_set()));
    let before: Vec<_> = history.inbox().map(|record| record.entry().encode_to_vec()).collect();
    let history = execute(history, &acknowledge("batch", 4));
    assert!(history.state().batches[0].acknowledged_at.is_set());
    assert_eq!(
        history
            .inbox()
            .map(|record| record.entry().encode_to_vec())
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(history.inbox().len(), 4);
    assert_entries(&history);
}

#[test]
fn terminal_before_intake_is_a_legitimate_zero_replacement_and_later_intake_is_rejected() {
    let history = execute(
        recipient(),
        &DisposeMessage::new(evidence(0, 7), TerminalDisposition::Unavailable, time(2)),
    );
    assert_eq!(history.inbox().len(), 0);
    assert!(history.state().inbox.is_empty());
    assert_eq!(history.state().dispositions.len(), 1);
    let (_, events) = evaluate_decision(enrolled("recipient", 0), &intake(0, 7)).unwrap();
    assert_eq!(
        history.clone().evolve(events.iter().next().unwrap()).unwrap_err(),
        EvolveError::InvalidMessageTransition
    );
    assert!(matches!(
        error(history.state().clone(), &intake(0, 7)),
        MessageError::AlreadyDisposed(_)
    ));
    assert_entries(&history);
}

#[test]
fn malformed_and_duplicate_events_never_publish_partial_inbox_updates() {
    let original = with_inbox(4);
    let (_, events) = evaluate_decision(original.state().clone(), &prepare(&[0, 1, 2, 3], 40, 3)).unwrap();
    let prepared = events.iter().next().unwrap();
    let empty = recipient();
    assert_eq!(
        empty.clone().evolve(prepared).unwrap_err(),
        EvolveError::InvalidMessageTransition
    );
    assert_entries(&empty);
    let mut invalid_later_receipt = prepared.clone();
    if let Some(PeerEventCase::PeerBatchPrepared(ref mut event)) = invalid_later_receipt.event {
        event.receipts[2].recipient_revision = Some(99);
    }
    assert_eq!(
        original.clone().evolve(&invalid_later_receipt).unwrap_err(),
        EvolveError::InvalidMessageTransition
    );
    assert_eq!(
        original
            .inbox()
            .filter(|record| record.entry().disposition.is_set())
            .count(),
        0
    );
    assert_entries(&original);
    let prepared_history = original.clone().evolve(prepared).unwrap();
    assert_eq!(
        prepared_history.clone().evolve(prepared).unwrap_err(),
        EvolveError::InvalidMessageTransition
    );
    assert_entries(&prepared_history);
    let (_, events) = evaluate_decision(enrolled("recipient", 0), &intake(0, 7)).unwrap();
    let intake_event = events.iter().next().unwrap();
    assert_eq!(
        original.clone().evolve(intake_event).unwrap_err(),
        EvolveError::InvalidMessageTransition
    );
    let mut malformed = intake_event.clone();
    if let Some(PeerEventCase::PeerMessageIntakeRecorded(ref mut event)) = malformed.event {
        event
            .entry
            .as_option_mut()
            .unwrap()
            .message
            .as_option_mut()
            .unwrap()
            .content_size_bytes = Some(0);
    }
    assert_eq!(
        recipient().evolve(&malformed).unwrap_err(),
        EvolveError::InvalidValue {
            field: "content",
            source: PeerValueError::InvalidContent
        }
    );
    assert_entries(&original);
}

#[test]
fn projected_record_and_raw_state_copies_cannot_mutate_the_closed_cursor() {
    let history = with_inbox(3);
    let expected: Vec<_> = history.inbox().map(|record| record.entry().encode_to_vec()).collect();
    let copied = history.inbox().next().unwrap().clone();
    let mut copied_entry = copied.entry().clone();
    copied_entry.message.as_option_mut().unwrap().content_size_bytes = Some(2049);
    let mut copied_metadata = copied.metadata().clone();
    copied_metadata.message_id = Some("different".into());
    let mut copied_state = history.clone().into_state();
    copied_state.inbox.reverse();
    assert_ne!(copied_entry.encode_to_vec(), copied.entry().encode_to_vec());
    assert_ne!(copied_metadata.encode_to_vec(), copied.metadata().encode_to_vec());
    assert_ne!(copied_state.encode_to_vec(), history.state().encode_to_vec());
    assert_eq!(
        history
            .inbox()
            .map(|record| record.entry().encode_to_vec())
            .collect::<Vec<_>>(),
        expected
    );
    assert_entries(&history);
}
