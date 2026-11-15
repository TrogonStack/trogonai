#![allow(clippy::unwrap_used)]

mod support;
use support::*;
use trogon_decider::{Decider, evaluate_decision};
use trogon_peer::*;
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};

fn execute<C>(mut history: PeerHistory, command: &C) -> PeerHistory
where
    C: Decider<State = PeerState, Event = proto::PeerEvent>,
    C::DecideError: std::fmt::Debug,
    C::EvolveError: std::fmt::Debug,
{
    let (expected, events) = evaluate_decision(history.state().clone(), command).unwrap();
    for event in events.iter() {
        history = history.evolve(event).unwrap();
    }
    assert_eq!(history.state(), &expected);
    history
}

#[test]
fn admitted_content_retains_both_wire_purposes_and_drives_exact_retry_after_settlement() {
    let enroll = Enroll::new(
        identity("sender"),
        ConnectorId::new("connector").unwrap(),
        CollaborationRef::new("collaboration").unwrap(),
        PeerDisplayName::new("Peer").unwrap(),
        time(0),
    );
    let history = execute(execute(PeerHistory::initial(), &enroll), &send(0, 1, 7));
    let key = IdempotencyKey::new("key_0").unwrap();
    let before = history.accepted(&key).unwrap().content().clone();
    let recipient = run(inbox(&[0], 7), &prepare(&[0], 20, 3));
    let history = execute(history, &settle(&recipient, 0));
    let projected = history.accepted(&key).unwrap();
    let legacy = AcceptedMessage::from_sender_history(history.state(), &MessageId::new("message_0").unwrap()).unwrap();
    for evidence in [projected, &legacy] {
        assert_eq!(evidence.content(), &before);
        assert_eq!(
            Some(&evidence.content().reference().to_claim_check()),
            evidence.metadata().content_ref.as_option()
        );
        assert_eq!(Some(evidence.content().size()), evidence.metadata().content_size_bytes);
        assert_eq!(
            Some(evidence.content().semantic_fingerprint()),
            evidence.metadata().semantic_fingerprint.value.as_deref()
        );
        assert_ne!(
            evidence.content().reference().integrity_digest(),
            evidence.content().semantic_fingerprint()
        );
        let retry = SendMessage::new(
            identity("sender").owner().clone(),
            reference("sender", 1),
            key.clone(),
            MessageId::new("new_proposal").unwrap(),
            identity("recipient").peer_id().clone(),
            CollaborationRef::new("collaboration").unwrap(),
            evidence.content().clone(),
            None,
            time(4),
        );
        assert_eq!(
            error(history.state().clone(), &retry),
            MessageError::AlreadyAccepted(Box::new(history.state().outbox[0].clone()))
        );
    }
    assert!(history.state().outbox[0].settlement.is_set());
    assert_eq!(history.state().outbox.len(), 1);
}
