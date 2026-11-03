use super::domain::*;
use super::{state,EvolveError,MessageError};
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::peers::{state_v1alpha1::PeerState,v1alpha1::PeerEvent};
#[allow(dead_code)]
#[derive(Debug,Clone)]
pub struct SettleMessage {sender: PeerId, evidence: AuthoritativeReceipt}
impl SettleMessage {pub fn new(sender: PeerId, evidence: AuthoritativeReceipt) -> Self {Self{sender, evidence}} }
impl Decider for SettleMessage {
    type StreamId=PeerId; type State=PeerState;type Event=PeerEvent;type DecideError=MessageError;type EvolveError=EvolveError;
    const WRITE_PRECONDITION: WritePrecondition=WritePrecondition::StreamUnchanged;
    fn stream_id(&self) -> &PeerId {&self.sender}
    fn initial_state() -> PeerState {state::initial_state()}
    fn evolve(state: PeerState,event: &PeerEvent) -> Result<PeerState,EvolveError> {state::evolve(state,event)}
    fn decide(_state: &PeerState,_command: &Self) -> Result<Decision<Self>,MessageError> {Err(MessageError::InvalidTransition)}
}
