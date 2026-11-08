use super::domain::IdempotencyKey;
use super::state::{self, ProjectionUpdate};
use super::{AcceptedMessage, EnrollmentFacts, EvolveError, IntakenMessage};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1::PeerEvent};

#[derive(Debug, Clone)]
pub(super) struct AcceptedOutbox {
    pub(super) key: IdempotencyKey,
    pub(super) evidence: AcceptedMessage,
}

#[derive(Debug, Clone)]
pub struct PeerHistory {
    state: PeerState,
    enrollment: Option<EnrollmentFacts>,
    accepted: Vec<AcceptedOutbox>,
    inbox: Vec<IntakenMessage>,
}

impl PeerHistory {
    pub fn initial() -> Self {
        Self {
            state: state::initial_state(),
            enrollment: None,
            accepted: Vec::new(),
            inbox: Vec::new(),
        }
    }

    pub fn evolve(self, event: &PeerEvent) -> Result<Self, EvolveError> {
        let transition = state::transition(self.state, event)?;
        let mut enrollment = self.enrollment;
        let mut accepted = self.accepted;
        let mut inbox = self.inbox;
        match transition.update {
            Some(ProjectionUpdate::Enrolled(facts)) => enrollment = Some(facts),
            Some(ProjectionUpdate::Accepted(message)) => accepted.push(*message),
            Some(ProjectionUpdate::Intaken(message)) => inbox.push(*message),
            Some(ProjectionUpdate::InboxReplacements(replacements)) => {
                for replacement in replacements {
                    // Receipt slots belong to this cursor's push-only inbox; no raw-state input can change its alignment.
                    inbox[replacement.slot].entry = replacement.entry;
                }
            }
            None => {}
        }
        Ok(Self {
            state: transition.state,
            enrollment,
            accepted,
            inbox,
        })
    }

    pub fn state(&self) -> &PeerState {
        &self.state
    }

    pub fn enrollment(&self) -> Option<&EnrollmentFacts> {
        self.enrollment.as_ref()
    }

    pub fn accepted(&self, key: &IdempotencyKey) -> Option<&AcceptedMessage> {
        self.accepted
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| &entry.evidence)
    }

    pub fn inbox(&self) -> std::slice::Iter<'_, IntakenMessage> {
        self.inbox.iter()
    }

    pub fn into_state(self) -> PeerState {
        self.state
    }
}
