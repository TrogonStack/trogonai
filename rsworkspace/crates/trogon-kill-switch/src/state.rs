use crate::{KillReason, KillSwitchEvent, OccurredAt};

/// Kill switch state for one agent, rebuilt by folding [`KillSwitchEvent`]s.
///
/// There is no explicit "unknown agent" variant distinct from [`Alive`](Self::Alive):
/// a stream with no history and a stream whose most recent event was
/// `AgentRevived` are behaviorally identical (the agent may run, may be killed
/// again, may be revived again as a no-op-rejected idempotent case). Collapsing
/// them keeps `decide` from needing to special-case "never seen" versus
/// "seen and currently alive."
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillSwitchState {
    /// The agent is not currently killed.
    Alive,
    /// The agent is killed for `reason`, effective `since`.
    Killed { reason: KillReason, since: OccurredAt },
}

pub const fn initial_state() -> KillSwitchState {
    KillSwitchState::Alive
}

pub fn evolve(_state: KillSwitchState, event: &KillSwitchEvent) -> KillSwitchState {
    match event {
        KillSwitchEvent::AgentKilled {
            reason, occurred_at, ..
        } => KillSwitchState::Killed {
            reason: *reason,
            since: *occurred_at,
        },
        KillSwitchEvent::AgentRevived { .. } => KillSwitchState::Alive,
    }
}

#[cfg(test)]
mod tests;
