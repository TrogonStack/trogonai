#![allow(clippy::unwrap_used)]
mod support;
use support::*;
use trogon_decider::{Decider, WritePrecondition, evaluate_decision};
use trogon_peer::*;
use trogonai_proto::peers::{PeerEventCase,v1alpha1 as proto};

#[test]
fn prepare_atomically_records_ordered_batch_and_monotonic_dispositions() {
    let (state,events)=evaluate_decision(inbox(&[3,1,2,0,4],2048),&prepare(&[3,1,2,0],8192,3)).unwrap();
    assert_eq!(PrepareBatch::WRITE_PRECONDITION,WritePrecondition::StreamUnchanged);
    assert_eq!(events.len(),1);
    assert_eq!(state.batches[0].message_ids,vec!["message_3","message_1","message_2","message_0"]);
    assert_eq!(state.dispositions.len(),4);
    for (i,receipt) in state.dispositions.iter().enumerate() {
        assert_eq!(receipt.recipient_revision,Some(i as u64+1));
        assert_eq!(receipt.batch_id.as_deref(),Some("batch"));
        assert_eq!(receipt.outcome.and_then(|v|v.as_known()),Some(proto::PreparationOutcome::PREPARATION_OUTCOME_BATCH_PREPARED));
        assert_eq!(receipt.decided_at.seconds,time(3).timestamp());
    }
    assert!(!state.inbox[4].disposition.is_set());
}
#[test]
fn rendered_total_includes_provenance_and_warning_budget() {
    for (ids,bytes) in [(vec![0],0),(vec![0],8193),(vec![0,1,2,3,4],8192),(vec![0,0],10),(vec![],1)] {
        assert!(TrustedBatchPlan::from_renderer(ids.into_iter().map(|i|MessageId::new(format!("message_{i}")).unwrap()).collect(),bytes).is_err());
    }
    assert_eq!(error(inbox(&[0],2048),&prepare(&[0],2047,3)),MessageError::InvalidPlan);
    assert_eq!(run(inbox(&[0],2048),&prepare(&[0],8192,3)).batches.len(),1);
}
#[test]
fn rejects_skipped_foreign_duplicate_and_reordered_selection() {
    for ids in [&[1][..],&[1,0][..],&[0,9][..]] {assert_eq!(error(inbox(&[0,1],1),&prepare(ids,20,3)),MessageError::InvalidPlan);}
}
#[test]
fn oldest_unacknowledged_batch_replays_original_identity_and_time_after_expiry() {
    let mut state=run(inbox(&[0,1],1),&prepare(&[0],50,3));
    let original=state.batches[0].clone();
    state.attachment=enrolled("recipient",4000).attachment;
    assert_eq!(error(state.clone(),&prepare(&[1],60,4001)),MessageError::AlreadyPrepared(Box::new(original)));
    assert!(matches!(error(state,&DisposeMessage::new(evidence(0,1),TerminalDisposition::Expired,time(4002))),MessageError::AlreadyDisposed(_)));
}
#[test]
fn expiry_and_unavailable_first_outcomes_exclude_preparation() {
    for (disposition,seconds) in [(TerminalDisposition::Expired,3601),(TerminalDisposition::Unavailable,3)] {
        let mut state=run(inbox(&[0,1],1),&DisposeMessage::new(evidence(0,1),disposition,time(seconds)));
        state.attachment=enrolled("recipient",seconds).attachment;
        assert_eq!(error(state,&prepare(&[0],20,seconds)),MessageError::InvalidPlan);
    }
    let mut state=inbox(&[0],1);state.attachment=enrolled("recipient",3601).attachment;
    assert_eq!(error(state,&prepare(&[0],20,3601)),MessageError::InvalidPlan);
}
#[test]
fn recipient_owner_and_current_generation_guard_prepare() {
    let state=inbox(&[0],1);
    let cmd=PrepareBatch::new(TrustedPeerOwner::new(TenantId::new("tenant").unwrap(),CallerId::new("foreign").unwrap()),reference("recipient",1),BatchId::new("batch").unwrap(),TrustedBatchPlan::from_renderer(vec![MessageId::new("message_0").unwrap()],20).unwrap(),time(3));
    assert_eq!(error(state.clone(),&cmd),MessageError::Guard(HeartbeatError::OwnerMismatch));
    assert_eq!(error(state,&prepare(&[0],20,10)),MessageError::Guard(HeartbeatError::Expired));
}
#[test]
fn prepared_unacknowledged_work_keeps_recipient_quota_reserved() {
    let indices:Vec<_>=(0..64).collect();
    let state=run(inbox(&indices,2048),&prepare(&[0,1,2,3],8192,3));
    assert_eq!(error(state,&intake(64,1)),MessageError::DeferIntake);
}
#[test]
fn malformed_prepared_batch_cannot_rewrite_winners() {
    let before=inbox(&[0,1],1);
    let (_,events)=evaluate_decision(before.clone(),&prepare(&[0,1],20,3)).unwrap();
    let event=events.iter().next().unwrap();
    for mutation in 0..7 {
        let mut event=event.clone();
        if let Some(PeerEventCase::PeerBatchPrepared(ref mut prepared))=event.event {
            let mut batch=prepared.batch.as_option().unwrap().clone();
            match mutation {0=>batch.message_ids.reverse(),1=>batch.message_ids.push("message_0".into()),2=>batch.acknowledged_at=batch.prepared_at.clone(),3=>prepared.receipts[0].recipient_revision=Some(3),4=>prepared.receipts[0].batch_id=Some("foreign".into()),5=>prepared.receipts[0].outcome=Some(buffa::EnumValue::from(91)),_=>{prepared.receipts.pop();}}
            prepared.batch=buffa::MessageField::some(batch);
        }
        assert!(commands::state::evolve(before.clone(),&event).is_err());
    }
}
