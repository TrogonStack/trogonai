//! Agent-view permission outcomes and discovery filtering.
//!
//! Pure value objects + the typed outcome of a per-agent view check.
//! The wire-side integration with SpiceDB (cache, gate trait, gRPC
//! impl) lives in a follow-up `spicedb_permission` slice — keeping
//! the typed outcome surface here lets callers that don't need the
//! gRPC client (e.g. unit tests, in-process discovery filters) reach
//! the filter without depending on the `spicedb` feature.
//!
//! Caller flow:
//! 1. Run a view check per agent — implementation hands back an
//!    [`AgentViewCheckOutcome`] per agent in caller-supplied order.
//! 2. Pair the outcomes with the agent-id list via
//!    [`filter_agents_by_view`] to produce
//!    [`DiscoveryAgentFilterOutcome`] entries the discovery
//!    response shapes from directly.

use crate::agent_id::A2aAgentId;

/// Session key for per-(principal, account) outcome caching. The
/// pair is intentionally explicit — a SpiceDB principal can act
/// across multiple imported accounts in the same tenant, and each
/// `(sub, account)` lookup is independent of the others.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SpiceDbSessionKey {
    sub: String,
    account: String,
}

impl SpiceDbSessionKey {
    pub fn new(sub: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            sub: sub.into(),
            account: account.into(),
        }
    }

    pub fn sub(&self) -> &str {
        &self.sub
    }

    pub fn account(&self) -> &str {
        &self.account
    }
}

/// Typed agent-view tuple a gate consults SpiceDB for. Modelled as
/// a struct rather than a bare `A2aAgentId` so the resource shape
/// stays explicit at the callsite — and so future tuple additions
/// (e.g. tenant) don't reshape every caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentViewTuple {
    agent_id: A2aAgentId,
}

impl AgentViewTuple {
    pub fn new(agent_id: A2aAgentId) -> Self {
        Self { agent_id }
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }

    pub fn resource_id(&self) -> &str {
        self.agent_id.as_str()
    }
}

/// Outcome of an agent-view permission check.
///
/// `TransportError` is distinct from `Denied` so the caller can
/// fail closed on connectivity issues without conflating them with
/// an explicit policy denial. Tests and audit consumers need both
/// reasons to distinguish "we couldn't ask" from "policy said no".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentViewCheckOutcome {
    Allowed,
    Denied,
    TransportError,
}

/// Per-agent discovery result: either the agent is visible to the
/// caller, or it's hidden along with the reason it was hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryAgentFilterOutcome {
    Visible(A2aAgentId),
    Hidden {
        agent_id: A2aAgentId,
        reason: DiscoveryHiddenReason,
    },
}

/// Reason an agent was filtered out of a discovery response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryHiddenReason {
    Denied,
    TransportError,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentViewFilterError {
    #[error("agent_ids and outcomes must have the same length (agents={agents}, outcomes={outcomes})")]
    LengthMismatch { agents: usize, outcomes: usize },
}

/// Pair each agent id with its view-check outcome and collapse into
/// the discovery filter shape. Fails closed (`Err(LengthMismatch)`)
/// when the slices disagree on length — without this guard a
/// caller bug that drops an outcome would silently slip the
/// orphaned agents through as `Visible`.
pub fn filter_agents_by_view(
    agent_ids: &[A2aAgentId],
    outcomes: &[AgentViewCheckOutcome],
) -> Result<Vec<DiscoveryAgentFilterOutcome>, AgentViewFilterError> {
    if agent_ids.len() != outcomes.len() {
        return Err(AgentViewFilterError::LengthMismatch {
            agents: agent_ids.len(),
            outcomes: outcomes.len(),
        });
    }
    Ok(agent_ids
        .iter()
        .zip(outcomes.iter())
        .map(|(agent_id, outcome)| match outcome {
            AgentViewCheckOutcome::Allowed => DiscoveryAgentFilterOutcome::Visible(agent_id.clone()),
            AgentViewCheckOutcome::Denied => DiscoveryAgentFilterOutcome::Hidden {
                agent_id: agent_id.clone(),
                reason: DiscoveryHiddenReason::Denied,
            },
            AgentViewCheckOutcome::TransportError => DiscoveryAgentFilterOutcome::Hidden {
                agent_id: agent_id.clone(),
                reason: DiscoveryHiddenReason::TransportError,
            },
        })
        .collect())
}

#[cfg(test)]
mod tests;
