use std::fmt;
use std::hash::{Hash, Hasher};

use super::errors::ContextThrottleError;

/// Rate-limit identity for delegated agents: one budget per `(tenant, agent, purpose)` tuple.
#[derive(Clone, Debug, Eq)]
pub struct ContextThrottleKey {
    pub tenant_id: String,
    pub agent_id: String,
    pub purpose: String,
}

impl PartialEq for ContextThrottleKey {
    fn eq(&self, other: &Self) -> bool {
        self.tenant_id == other.tenant_id
            && self.agent_id == other.agent_id
            && self.purpose == other.purpose
    }
}

impl Hash for ContextThrottleKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tenant_id.hash(state);
        self.agent_id.hash(state);
        self.purpose.hash(state);
    }
}

impl fmt::Display for ContextThrottleKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}:{}",
            self.tenant_id, self.agent_id, self.purpose
        )
    }
}

impl ContextThrottleKey {
    pub fn new(
        tenant_id: impl Into<String>,
        agent_id: impl Into<String>,
        purpose: impl Into<String>,
    ) -> Result<Self, ContextThrottleError> {
        let tenant_id = tenant_id.into();
        let agent_id = agent_id.into();
        let purpose = purpose.into();
        if tenant_id.is_empty() {
            return Err(ContextThrottleError::MalformedKey { field: "tenant_id" });
        }
        if agent_id.is_empty() {
            return Err(ContextThrottleError::MalformedKey { field: "agent_id" });
        }
        if purpose.is_empty() {
            return Err(ContextThrottleError::MalformedKey { field: "purpose" });
        }
        Ok(Self {
            tenant_id,
            agent_id,
            purpose,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tenant_is_malformed() {
        let err = ContextThrottleKey::new("", "agent/a", "deploy").unwrap_err();
        assert_eq!(
            err,
            ContextThrottleError::MalformedKey { field: "tenant_id" }
        );
    }

    #[test]
    fn empty_agent_is_malformed() {
        let err = ContextThrottleKey::new("acme", "", "deploy").unwrap_err();
        assert_eq!(
            err,
            ContextThrottleError::MalformedKey { field: "agent_id" }
        );
    }

    #[test]
    fn empty_purpose_is_malformed() {
        let err = ContextThrottleKey::new("acme", "agent/a", "").unwrap_err();
        assert_eq!(
            err,
            ContextThrottleError::MalformedKey { field: "purpose" }
        );
    }
}
