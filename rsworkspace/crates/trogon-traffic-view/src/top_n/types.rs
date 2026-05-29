use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Root `agent_id` from `act_chain`, or `caller_sub` when the chain has no root agent.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MostActiveAgent {
    pub agent_id: String,
    pub event_count: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MostDeniedAgent {
    pub agent_id: String,
    pub deny_count: usize,
    pub top_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeepestChain {
    pub event_id: String,
    pub tenant: String,
    pub depth: usize,
    pub ts: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LongestChainByTenant {
    pub tenant: String,
    pub event_id: String,
    pub depth: usize,
    pub ts: DateTime<Utc>,
}
