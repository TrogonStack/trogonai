use super::domain::*;
use super::{state, EvolveError, MessageError};
use chrono::{DateTime, Utc};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState,v1alpha1::PeerEvent};
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DisposeMessage { pub(crate) evidence: AcceptedMessage, pub(crate) disposition: TerminalDisposition, pub(crate) now: DateTime<Utc> }
impl DisposeMessage { pub fn new(evidence: AcceptedMessage, disposition: TerminalDisposition, now: DateTime<Utc>) -> Self { Self { evidence, disposition, now } } }
impl Decider for DisposeMessage {
    type StreamId=PeerId; type State=PeerState; type Event=PeerEvent; type DecideError=MessageError; type EvolveError=EvolveError;
    const WRITE_PRECONDITION: WritePrecondition=WritePrecondition::StreamUnchanged;
    fn stream_id(&self) -> &PeerId { &self.evidence.recipient }
    fn initial_state() -> PeerState {state::initial_state()}
    fn evolve(state: PeerState,event: &PeerEvent) -> Result<PeerState,EvolveError> {state::evolve(state,event)}
    fn decide(_state: &PeerState,_command: &Self) -> Result<Decision<Self>,MessageError> {Err(MessageError::InvalidTransition)}
}
