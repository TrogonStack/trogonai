#![allow(clippy::unwrap_used)]
mod support;
use support::*;
use trogon_peer::*;
use trogonai_proto::peers::v1alpha1 as proto;

#[test]
fn intake_is_durable_ordered_and_independent_of_online_attachment() {
    let state=run(enrolled("recipient",0),&intake(3,2048));
    let state=run(state,&IntakeMessage::new(evidence(1,1),time(11)));
    assert_eq!(state.inbox[0].message.message_id.as_deref(),Some("message_3"));
    assert_eq!(state.inbox[1].intake_ordinal,Some(2));
    assert_eq!(state.next_intake_ordinal,Some(3));
    assert_eq!(error(state.clone(),&intake(3,2048)),MessageError::AlreadyIntaken(Box::new(state.inbox[0].clone())));
    assert_eq!(error(state,&intake(3,1)),MessageError::Conflict);
}
#[test]
fn pending_sixty_four_and_128_kib_defer_without_sender_settlement() {
    let mut state=enrolled("recipient",0);
    for i in 0..64 { state=run(state,&intake(i,2048)); }
    assert_eq!(error(state.clone(),&intake(64,1)),MessageError::DeferIntake);
    assert!(matches!(error(state.clone(),&intake(0,2048)),MessageError::AlreadyIntaken(_)));
    state=run(state,&DisposeMessage::new(evidence(0,2048),TerminalDisposition::Unavailable,time(3)));
    let state=run(state,&intake(64,2048));
    assert_eq!(state.inbox.len(),65);
    assert_eq!(state.dispositions.len(),1);
}
#[test]
fn recipient_collaboration_and_tenant_must_match_accepted_evidence() {
    assert_eq!(error(enrolled("foreign",0),&intake(0,1)),MessageError::RecipientMismatch);
    let mut state=enrolled("recipient",0);
    let mut enrollment=state.enrollment.as_option().unwrap().clone();
    enrollment.collaboration_ref=Some("foreign".into());state.enrollment=buffa::MessageField::some(enrollment);
    assert_eq!(error(state,&intake(0,1)),MessageError::CollaborationMismatch);
    let mut sender=run(enrolled("sender",0),&send(0,1,1));
    let mut entry=sender.outbox[0].clone();entry.semantic_digest=Some(vec![0;32]);sender.outbox[0]=entry;
    assert!(AcceptedMessage::from_sender_history(&sender,&MessageId::new("message_0").unwrap()).is_err());
}
#[test]
fn expiry_cutoff_tombstones_before_intake_and_never_rewrites_outcome() {
    let cmd=DisposeMessage::new(evidence(0,1),TerminalDisposition::Expired,time(3601));
    assert_eq!(error(enrolled("recipient",0),&DisposeMessage::new(evidence(0,1),TerminalDisposition::Expired,time(3600))),MessageError::InvalidTransition);
    assert_eq!(error(enrolled("recipient",0),&IntakeMessage::new(evidence(0,1),time(3601))),MessageError::RequiresDisposition);
    let state=run(enrolled("recipient",0),&cmd);
    assert!(state.inbox.is_empty());
    assert_eq!(state.dispositions[0].recipient_revision,Some(1));
    for other in [cmd,DisposeMessage::new(evidence(0,1),TerminalDisposition::Unavailable,time(4000))] {assert_eq!(error(state.clone(),&other),MessageError::AlreadyDisposed(Box::new(state.dispositions[0].clone())));}
    assert!(matches!(error(state,&intake(0,1)),MessageError::AlreadyDisposed(_)));
}
#[test]
fn malformed_intake_and_receipt_history_is_refused() {
    let state=run(enrolled("recipient",0),&intake(0,1));
    for ordinal in [None,Some(0),Some(2)] {
        let mut entry=state.inbox[0].clone();entry.intake_ordinal=ordinal;
        assert!(commands::state::evolve(enrolled("recipient",0),&proto::PeerEvent{event:Some(proto::PeerMessageIntakeRecorded{entry:buffa::MessageField::some(entry)}.into())}).is_err());
    }
    let disposed=run(state.clone(),&DisposeMessage::new(evidence(0,1),TerminalDisposition::Unavailable,time(3)));
    for mutation in 0..6 {
        let mut receipt=disposed.dispositions[0].clone();
        match mutation {0=>receipt.outcome=Some(buffa::EnumValue::from(99)),1=>receipt.outcome=Some(proto::PreparationOutcome::PREPARATION_OUTCOME_UNSPECIFIED.into()),2=>receipt.recipient_revision=Some(0),3=>receipt.recipient_revision=Some(2),4=>receipt.batch_id=Some("unexpected".into()),_=>receipt.recipient_peer_id=Some(identity("foreign").peer_id().to_string())}
        assert!(commands::state::evolve(state.clone(),&proto::PeerEvent{event:Some(proto::PeerMessageDisposed{receipt:buffa::MessageField::some(receipt)}.into())}).is_err());
    }
}
