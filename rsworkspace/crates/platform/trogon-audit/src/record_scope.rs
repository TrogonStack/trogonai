use trogon_nats::NatsToken;

use crate::actor::Actor;
use crate::audit_name::AuditName;
use crate::constants::MAX_ACT_CHAIN_ENTRIES;
use crate::trace_id::TraceId;

/// The identity every record carries: tenant, project, actor, delegation chain
/// and correlation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordScope {
    pub(crate) tenant: AuditName,
    pub(crate) project: NatsToken,
    pub(crate) actor: Actor,
    pub(crate) act_chain: Vec<AuditName>,
    pub(crate) trace_id: Option<TraceId>,
    pub(crate) request_id: Option<AuditName>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("act chain has {0} entries, over the {MAX_ACT_CHAIN_ENTRIES}-entry limit")]
pub struct ActChainTooLongError(usize);

impl RecordScope {
    pub fn new(tenant: AuditName, project: NatsToken, actor: Actor) -> Self {
        Self {
            tenant,
            project,
            actor,
            act_chain: Vec::new(),
            trace_id: None,
            request_id: None,
        }
    }

    /// The delegating agents, in order, as recorded by the verified token chain.
    pub fn with_act_chain(mut self, act_chain: Vec<AuditName>) -> Result<Self, ActChainTooLongError> {
        if act_chain.len() > MAX_ACT_CHAIN_ENTRIES {
            return Err(ActChainTooLongError(act_chain.len()));
        }
        self.act_chain = act_chain;
        Ok(self)
    }

    pub fn with_trace_id(mut self, trace_id: TraceId) -> Self {
        self.trace_id = Some(trace_id);
        self
    }

    pub fn with_request_id(mut self, request_id: AuditName) -> Self {
        self.request_id = Some(request_id);
        self
    }

    pub fn project(&self) -> &NatsToken {
        &self.project
    }

    pub fn actor(&self) -> &Actor {
        &self.actor
    }
}

#[cfg(test)]
mod tests;
