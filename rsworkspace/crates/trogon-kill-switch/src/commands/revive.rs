use std::convert::Infallible;

use trogon_decider::{Decider, Decision};

use crate::state::{evolve, initial_state};
use crate::{AgentId, KillSwitchEvent, KillSwitchState, OccurredAt};

/// Revive a previously killed agent, effective `occurred_at`.
#[derive(Debug, Clone)]
pub struct Revive {
    pub agent_id: AgentId,
    pub occurred_at: OccurredAt,
}

impl Revive {
    pub fn new(agent_id: AgentId, occurred_at: OccurredAt) -> Self {
        Self { agent_id, occurred_at }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ReviveError {
    /// Reviving an already-alive agent is a no-op, not an error, mirroring
    /// `KillError::AlreadyKilled`'s idempotency stance: the agent is already
    /// in the state the command asked for, so no event is emitted.
    #[error("agent is already alive; revive is idempotent, no event emitted")]
    AlreadyAlive,
}

impl Decider for Revive {
    type StreamId = str;
    type State = KillSwitchState;
    type Event = KillSwitchEvent;
    type DecideError = ReviveError;
    type EvolveError = Infallible;

    fn stream_id(&self) -> &Self::StreamId {
        self.agent_id.as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        Ok(evolve(state, event))
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        match state {
            KillSwitchState::Alive => Err(ReviveError::AlreadyAlive),
            KillSwitchState::Killed { .. } => Ok(Decision::event(KillSwitchEvent::AgentRevived {
                agent_id: command.agent_id.clone(),
                occurred_at: command.occurred_at,
            })),
        }
    }

    fn decide_error_code(error: &Self::DecideError) -> &str {
        match error {
            ReviveError::AlreadyAlive => "already-alive",
        }
    }
}

#[cfg(test)]
mod tests;
