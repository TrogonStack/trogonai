#![allow(dead_code)]
use chrono::{DateTime, TimeZone, Utc};
use trogon_decider::{Decider, DecisionError, evaluate_decision};
use trogon_peer::*;
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};
pub fn time(seconds: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap() + chrono::Duration::seconds(seconds)
}
pub fn identity(session: &str) -> EnrollmentIdentity {
    EnrollmentIdentity::new(
        TrustedPeerOwner::new(TenantId::new("tenant").unwrap(), CallerId::new("caller").unwrap()),
        InstallationId::new("installation").unwrap(),
        NativeSessionId::new(session).unwrap(),
    )
}
pub fn reference(session: &str, generation: u64) -> AttachmentRef {
    AttachmentRef::new(
        identity(session).peer_id().clone(),
        ConnectorId::new("connector").unwrap(),
        generation,
    )
    .unwrap()
}
pub fn enrolled(session: &str, seconds: i64) -> PeerState {
    run(
        SendMessage::initial_state(),
        &Enroll::new(
            identity(session),
            ConnectorId::new("connector").unwrap(),
            CollaborationRef::new("collaboration").unwrap(),
            PeerDisplayName::new("Peer").unwrap(),
            time(seconds),
        ),
    )
}
pub fn run<C: Decider<State = PeerState, Event = proto::PeerEvent>>(state: PeerState, command: &C) -> PeerState
where
    C::DecideError: std::fmt::Debug,
    C::EvolveError: std::fmt::Debug,
{
    evaluate_decision(state, command).unwrap().0
}
pub fn error<C: Decider<State = PeerState, Event = proto::PeerEvent, DecideError = MessageError>>(
    state: PeerState,
    command: &C,
) -> MessageError
where
    C::EvolveError: std::fmt::Debug,
{
    match evaluate_decision(state, command).unwrap_err() {
        DecisionError::Decide(e) => e,
        other => panic!("unexpected failure: {other:?}"),
    }
}
pub fn content(size: u32, digest: u8) -> TrustedContent {
    TrustedContent::from_sealed_port(SealedContentRef::new("sealed_ref").unwrap(), size, vec![digest; 32]).unwrap()
}
pub fn send(index: usize, seconds: i64, size: u32) -> SendMessage {
    SendMessage::new(
        identity("sender").owner().clone(),
        reference("sender", 1),
        IdempotencyKey::new(format!("key_{index}")).unwrap(),
        MessageId::new(format!("message_{index}")).unwrap(),
        identity("recipient").peer_id().clone(),
        CollaborationRef::new("collaboration").unwrap(),
        content(size, 1),
        None,
        time(seconds),
    )
}

pub fn evidence(index: usize, size: u32) -> AcceptedMessage {
    AcceptedMessage::from_sender_history(
        &run(enrolled("sender", 0), &send(index, 1, size)),
        &MessageId::new(format!("message_{index}")).unwrap(),
    )
    .unwrap()
}
pub fn intake(index: usize, size: u32) -> IntakeMessage {
    IntakeMessage::new(evidence(index, size), time(2))
}
