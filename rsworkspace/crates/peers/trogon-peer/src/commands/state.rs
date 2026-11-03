use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("peer event evolution is not implemented")]
pub struct EvolveError;

pub fn initial_state() -> PeerState {
    PeerState::default()
}

pub fn evolve(_state: PeerState, _event: &PeerEvent) -> Result<PeerState, EvolveError> {
    Err(EvolveError)
}
