#![allow(clippy::unwrap_used)]

mod support;
use support::*;
use trogon_decider::{Decider, evaluate_decision};
use trogon_peer::*;
use trogonai_proto::peers::{PeerEventCase, state_v1alpha1::PeerState, v1alpha1 as proto};

fn enrollment(session: &str) -> Enroll {
    Enroll::new(
        identity(session),
        ConnectorId::new("connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        PeerDisplayName::new("Peer").unwrap(),
        time(0),
    )
}

fn execute<C>(mut history: PeerHistory, command: &C) -> PeerHistory
where
    C: Decider<State = PeerState, Event = proto::PeerEvent>,
    C::DecideError: std::fmt::Debug,
    C::EvolveError: std::fmt::Debug,
{
    let (expected, events) = evaluate_decision(history.state().clone(), command).unwrap();
    let mut raw = history.state().clone();
    for event in events.iter() {
        raw = commands::state::evolve(raw, event).unwrap();
        history = history.evolve(event).unwrap();
        assert_eq!(history.state(), &raw);
    }
    assert_eq!(history.state(), &expected);
    history
}

fn enrolled_history(session: &str) -> PeerHistory {
    execute(PeerHistory::initial(), &enrollment(session))
}

#[test]
fn accepted_projection_retains_exact_routing_and_enrollment_through_settlement() {
    let initial = PeerHistory::initial();
    assert_eq!(initial.state(), &SendMessage::initial_state());
    assert!(initial.enrollment().is_none());
    assert!(initial.accepted(&IdempotencyKey::new("key_0").unwrap()).is_none());
    let history = execute(enrolled_history("sender"), &send(0, 1, 7));
    let recipient = run(inbox(&[0], 7), &prepare(&[0], 20, 3));
    let history = execute(history, &settle(&recipient, 0));
    let facts = history.enrollment().unwrap();
    assert_eq!(facts.identity(), &identity("sender"));
    assert_eq!(facts.collaboration(), &CollaborationRef::new("collaboration").unwrap());
    let accepted = history.accepted(&IdempotencyKey::new("key_0").unwrap()).unwrap();
    assert_eq!(accepted.tenant(), identity("sender").owner().tenant());
    assert_eq!(accepted.recipient(), identity("recipient").peer_id());
    assert_eq!(Some(accepted.metadata()), history.state().outbox[0].message.as_option());
    assert!(history.state().outbox[0].settlement.is_set());
    let legacy = AcceptedMessage::from_sender_history(history.state(), &MessageId::new("message_0").unwrap()).unwrap();
    assert_eq!(legacy.recipient(), accepted.recipient());
    assert_eq!(legacy.metadata(), accepted.metadata());
    assert!(history.accepted(&IdempotencyKey::new("missing").unwrap()).is_none());
    assert_eq!(history.clone().into_state(), *history.state());
}

#[test]
fn recipient_history_preserves_facts_across_intake_disposition_batch_and_attachment_events() {
    let history = execute(enrolled_history("recipient"), &intake(0, 1));
    let history = execute(history, &intake(1, 1));
    let history = execute(
        history,
        &DisposeMessage::new(evidence(1, 1), TerminalDisposition::Unavailable, time(3)),
    );
    let history = execute(history, &prepare(&[0], 20, 3));
    let history = execute(history, &acknowledge("batch", 4));
    let history = execute(
        history,
        &Detach::new(
            identity("recipient").owner().clone(),
            reference("recipient", 1),
            time(5),
        ),
    );
    let history = execute(
        history,
        &Attach::new(
            identity("recipient"),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            time(6),
        ),
    );
    assert_eq!(history.enrollment().unwrap().identity(), &identity("recipient"));
    assert_eq!(history.enrollment().unwrap().collaboration().as_str(), "collaboration");
    assert_eq!(history.state().inbox.len(), 2);
    assert_eq!(history.state().dispositions.len(), 2);
    assert!(history.state().batches[0].acknowledged_at.is_set());
    assert!(history.accepted(&IdempotencyKey::new("key_0").unwrap()).is_none());
}

#[test]
fn owned_state_copies_and_consumed_clones_cannot_change_retained_facts() {
    let history = execute(enrolled_history("sender"), &send(0, 1, 1));
    let original = history.state().clone();
    let mut borrowed_copy = history.state().clone();
    borrowed_copy.enrollment.as_option_mut().unwrap().collaboration_ref = Some("changed_copy".into());
    let mut consumed_copy = history.clone().into_state();
    consumed_copy.outbox[0]
        .message
        .as_option_mut()
        .unwrap()
        .recipient_peer_id = Some(identity("foreign").peer_id().to_string());
    assert_ne!(borrowed_copy, original);
    assert_ne!(consumed_copy, original);
    assert_eq!(history.state(), &original);
    assert_eq!(history.enrollment().unwrap().collaboration().as_str(), "collaboration");
    assert_eq!(
        history
            .accepted(&IdempotencyKey::new("key_0").unwrap())
            .unwrap()
            .recipient(),
        identity("recipient").peer_id()
    );
}

#[test]
fn malformed_enrollment_fails_at_the_same_field_and_source_before_any_projection() {
    let (_, events) = evaluate_decision(SendMessage::initial_state(), &enrollment("sender")).unwrap();
    let original = events.iter().next().unwrap();
    for (value, expected) in [
        (
            None,
            EvolveError::MissingField {
                field: "enrollment.collaboration_ref",
            },
        ),
        (
            Some(String::new()),
            EvolveError::InvalidValue {
                field: "enrollment.collaboration_ref",
                source: PeerValueError::Empty,
            },
        ),
        (
            Some("control\nvalue".into()),
            EvolveError::InvalidValue {
                field: "enrollment.collaboration_ref",
                source: PeerValueError::InvalidCharacter,
            },
        ),
        (
            Some("x".repeat(257)),
            EvolveError::InvalidValue {
                field: "enrollment.collaboration_ref",
                source: PeerValueError::TooLong { maximum: 256 },
            },
        ),
    ] {
        let mut event = original.clone();
        if let Some(PeerEventCase::PeerEnrolled(ref mut enrolled)) = event.event {
            enrolled.enrollment.as_option_mut().unwrap().collaboration_ref = value;
        }
        assert_eq!(PeerHistory::initial().evolve(&event).unwrap_err(), expected);
        assert_eq!(
            commands::state::evolve(SendMessage::initial_state(), &event),
            Err(expected)
        );
    }
}

#[test]
fn duplicate_enrollment_and_missing_event_keep_the_original_error_precedence() {
    let history = enrolled_history("sender");
    let (_, events) = evaluate_decision(SendMessage::initial_state(), &enrollment("sender")).unwrap();
    let mut duplicate = events.iter().next().unwrap().clone();
    if let Some(PeerEventCase::PeerEnrolled(ref mut enrolled)) = duplicate.event {
        enrolled.enrollment.as_option_mut().unwrap().collaboration_ref = None;
    }
    assert_eq!(
        history.clone().evolve(&duplicate).unwrap_err(),
        EvolveError::AlreadyEnrolled
    );
    assert_eq!(
        commands::state::evolve(history.state().clone(), &duplicate),
        Err(EvolveError::AlreadyEnrolled)
    );
    assert_eq!(
        PeerHistory::initial().evolve(&proto::PeerEvent::default()).unwrap_err(),
        EvolveError::MissingEvent
    );
    assert_eq!(history.enrollment().unwrap().collaboration().as_str(), "collaboration");
}

#[test]
fn accepted_recipient_validation_still_precedes_semantic_digest_validation() {
    let history = enrolled_history("sender");
    let (_, events) = evaluate_decision(history.state().clone(), &send(0, 1, 1)).unwrap();
    let original = events.iter().next().unwrap();
    for (recipient, expected) in [
        (
            None,
            EvolveError::MissingField {
                field: "recipient_peer_id",
            },
        ),
        (
            Some("invalid".into()),
            EvolveError::InvalidValue {
                field: "recipient_peer_id",
                source: PeerValueError::InvalidPeerId,
            },
        ),
    ] {
        let mut event = original.clone();
        if let Some(PeerEventCase::PeerMessageAccepted(ref mut accepted)) = event.event {
            accepted
                .entry
                .as_option_mut()
                .unwrap()
                .message
                .as_option_mut()
                .unwrap()
                .recipient_peer_id = recipient;
        }
        assert_eq!(history.clone().evolve(&event).unwrap_err(), expected);
        assert_eq!(commands::state::evolve(history.state().clone(), &event), Err(expected));
    }
    let mut bad_key_and_recipient = original.clone();
    if let Some(PeerEventCase::PeerMessageAccepted(ref mut accepted)) = bad_key_and_recipient.event {
        let entry = accepted.entry.as_option_mut().unwrap();
        entry.idempotency_key = Some("invalid.key".into());
        entry.message.as_option_mut().unwrap().recipient_peer_id = None;
    }
    assert_eq!(
        history.evolve(&bad_key_and_recipient).unwrap_err(),
        EvolveError::InvalidValue {
            field: "idempotency_key",
            source: PeerValueError::InvalidCharacter
        }
    );
}

#[test]
fn accepted_projection_never_records_invalid_digest_or_duplicate_acceptance() {
    let history = enrolled_history("sender");
    let (_, events) = evaluate_decision(history.state().clone(), &send(0, 1, 1)).unwrap();
    let original = events.iter().next().unwrap();
    let mut bad_digest = original.clone();
    if let Some(PeerEventCase::PeerMessageAccepted(ref mut accepted)) = bad_digest.event {
        accepted.entry.as_option_mut().unwrap().semantic_digest = Some(vec![0; 32]);
    }
    assert_eq!(
        history.clone().evolve(&bad_digest).unwrap_err(),
        EvolveError::InvalidMessageTransition
    );
    assert!(history.accepted(&IdempotencyKey::new("key_0").unwrap()).is_none());
    let accepted = history.evolve(original).unwrap();
    assert_eq!(
        accepted.clone().evolve(original).unwrap_err(),
        EvolveError::InvalidMessageTransition
    );
    assert_eq!(accepted.state().outbox.len(), 1);
    assert_eq!(
        accepted
            .accepted(&IdempotencyKey::new("key_0").unwrap())
            .unwrap()
            .metadata()
            .message_id
            .as_deref(),
        Some("message_0")
    );
}
