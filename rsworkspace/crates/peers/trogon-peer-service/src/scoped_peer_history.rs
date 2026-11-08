use std::ops::Deref;
use trogon_peer::{
    AcceptedMessage, CollaborationRef, EnrollmentFacts, IdempotencyKey, IntakenMessage, PeerHistory, PeerId,
    TrustedPeerOwner,
};
use trogonai_proto::peers::state_v1alpha1::PeerState;

pub(crate) struct ScopedPeerHistory {
    history: PeerHistory,
    enrollment: EnrollmentFacts,
}

impl ScopedPeerHistory {
    pub(crate) fn from_history(history: PeerHistory, owner: &TrustedPeerOwner, peer: &PeerId) -> Option<Self> {
        let enrollment = history.enrollment()?.clone();
        if enrollment.identity().peer_id() != peer || enrollment.identity().owner().tenant() != owner.tenant() {
            return None;
        }
        Some(Self { history, enrollment })
    }

    pub(crate) fn collaboration(&self) -> &CollaborationRef {
        self.enrollment.collaboration()
    }

    pub(crate) fn inbox_entries(&self) -> std::slice::Iter<'_, IntakenMessage> {
        self.history.inbox()
    }

    pub(crate) fn accepted(&self, key: &IdempotencyKey) -> Option<&AcceptedMessage> {
        self.history.accepted(key)
    }
}

impl Deref for ScopedPeerHistory {
    type Target = PeerState;

    fn deref(&self) -> &Self::Target {
        self.history.state()
    }
}
