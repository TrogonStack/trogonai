mod aggregates;
mod errors;
mod types;

pub use aggregates::{
    deepest_chains, longest_chain_by_tenant, most_active_agents, most_denied_agents,
};
pub use errors::TopNError;
pub use types::{DeepestChain, LongestChainByTenant, MostActiveAgent, MostDeniedAgent};

use crate::event::TrafficEvent;

pub struct TopNDashboards<'events> {
    events: &'events [TrafficEvent],
}

impl<'events> TopNDashboards<'events> {
    #[must_use]
    pub fn new(events: &'events [TrafficEvent]) -> Self {
        Self { events }
    }

    pub fn most_active_agents(&self, n: usize) -> Result<Vec<MostActiveAgent>, TopNError> {
        most_active_agents(self.events, n)
    }

    pub fn most_denied_agents(&self, n: usize) -> Result<Vec<MostDeniedAgent>, TopNError> {
        most_denied_agents(self.events, n)
    }

    pub fn deepest_chains(&self, n: usize) -> Result<Vec<DeepestChain>, TopNError> {
        deepest_chains(self.events, n)
    }

    pub fn longest_chain_by_tenant(&self) -> Result<Vec<LongestChainByTenant>, TopNError> {
        longest_chain_by_tenant(self.events)
    }
}
