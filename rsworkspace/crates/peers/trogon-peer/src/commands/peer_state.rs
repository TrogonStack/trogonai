use trogonai_proto::peers::state_v1alpha1;

#[repr(transparent)]
#[derive(Debug, Clone, PartialEq)]
pub struct PeerState(state_v1alpha1::PeerState);

impl Default for PeerState {
    fn default() -> Self {
        Self(state_v1alpha1::PeerState {
            next_intake_ordinal: Some(1),
            ..Default::default()
        })
    }
}

impl std::ops::Deref for PeerState {
    type Target = state_v1alpha1::PeerState;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for PeerState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<state_v1alpha1::PeerState> for PeerState {
    fn from(value: state_v1alpha1::PeerState) -> Self {
        Self(value)
    }
}

impl From<PeerState> for state_v1alpha1::PeerState {
    fn from(value: PeerState) -> Self {
        value.0
    }
}

impl AsRef<state_v1alpha1::PeerState> for PeerState {
    fn as_ref(&self) -> &state_v1alpha1::PeerState {
        &self.0
    }
}

impl trogon_decider::StateSnapshot for PeerState {
    type Payload = state_v1alpha1::PeerState;
}
