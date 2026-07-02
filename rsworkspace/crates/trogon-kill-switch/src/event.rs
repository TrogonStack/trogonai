use crate::{AgentId, KillReason, OccurredAt};

/// Domain events for the kill switch aggregate.
///
/// One JetStream subject per agent (see `docs/proposals/kill-switch-enforcement.md`
/// for the subject layout), so every event in a stream already shares its
/// `agent_id`; the field is still carried on each event because `evolve` and
/// downstream projections operate on individual events, not the stream context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillSwitchEvent {
    /// An agent was killed for `reason`, effective `occurred_at`.
    AgentKilled {
        agent_id: AgentId,
        reason: KillReason,
        occurred_at: OccurredAt,
    },
    /// A previously killed agent was revived, effective `occurred_at`.
    AgentRevived { agent_id: AgentId, occurred_at: OccurredAt },
}

impl KillSwitchEvent {
    pub const fn agent_id(&self) -> &AgentId {
        match self {
            Self::AgentKilled { agent_id, .. } | Self::AgentRevived { agent_id, .. } => agent_id,
        }
    }

    pub const fn occurred_at(&self) -> OccurredAt {
        match self {
            Self::AgentKilled { occurred_at, .. } | Self::AgentRevived { occurred_at, .. } => *occurred_at,
        }
    }
}

#[cfg(test)]
mod tests;
