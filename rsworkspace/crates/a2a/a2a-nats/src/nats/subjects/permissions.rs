use trogon_identity_types::SubjectPrincipal;

use crate::a2a_prefix::A2aPrefix;

#[derive(Debug)]
pub struct CallerAgentRequestsSubject {
    prefix: A2aPrefix,
    caller: SubjectPrincipal,
}

impl CallerAgentRequestsSubject {
    pub fn new(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        Self {
            prefix: prefix.clone(),
            caller: caller.clone(),
        }
    }
}

impl std::fmt::Display for CallerAgentRequestsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.agents.*.{}.>", self.prefix.as_str(), self.caller)
    }
}

#[derive(Debug)]
pub struct CallerGatewayRequestsSubject {
    prefix: A2aPrefix,
    caller: SubjectPrincipal,
}

impl CallerGatewayRequestsSubject {
    pub fn new(prefix: &A2aPrefix, caller: &SubjectPrincipal) -> Self {
        Self {
            prefix: prefix.clone(),
            caller: caller.clone(),
        }
    }
}

impl std::fmt::Display for CallerGatewayRequestsSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.v1.gateway.*.{}.>", self.prefix.as_str(), self.caller)
    }
}

#[cfg(test)]
mod tests;
