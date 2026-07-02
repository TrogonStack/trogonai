use std::convert::Infallible;

use trogon_decider::{Decider, Decision};

use crate::state::{evolve, initial_state};
use crate::{AgentId, KillReason, KillSwitchEvent, KillSwitchState, OccurredAt};

/// Kill an agent for `reason`, effective `occurred_at`.
#[derive(Debug, Clone)]
pub struct Kill {
    pub agent_id: AgentId,
    pub reason: KillReason,
    pub occurred_at: OccurredAt,
}

impl Kill {
    pub fn new(agent_id: AgentId, reason: KillReason, occurred_at: OccurredAt) -> Self {
        Self {
            agent_id,
            reason,
            occurred_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KillError {
    /// Killing an already-killed agent is a no-op, not an error: the agent is
    /// already in the state the command asked for. Matches AGT's spec ("kill
    /// switch operation and failure modes" section 12.6) treating a repeat
    /// kill as recorded rather than as a failure that blocks the caller, while
    /// keeping our stream append-free (no redundant event) unlike AGT, which
    /// always appends a new `KillResult` to its in-memory history.
    #[error("agent is already killed for reason '{existing_reason}'; kill is idempotent, no event emitted")]
    AlreadyKilled { existing_reason: KillReason },
}

impl Decider for Kill {
    type StreamId = str;
    type State = KillSwitchState;
    type Event = KillSwitchEvent;
    type DecideError = KillError;
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
            KillSwitchState::Killed { reason, .. } => Err(KillError::AlreadyKilled {
                existing_reason: *reason,
            }),
            KillSwitchState::Alive => Ok(Decision::event(KillSwitchEvent::AgentKilled {
                agent_id: command.agent_id.clone(),
                reason: command.reason,
                occurred_at: command.occurred_at,
            })),
        }
    }

    fn decide_error_code(error: &Self::DecideError) -> &str {
        match error {
            KillError::AlreadyKilled { .. } => "already-killed",
        }
    }
}

#[cfg(test)]
mod tests;
