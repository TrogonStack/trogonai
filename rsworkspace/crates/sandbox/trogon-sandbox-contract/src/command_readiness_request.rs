use serde::{Deserialize, Deserializer, Serialize, de};

use crate::command_readiness_request_wire::CommandReadinessRequestWire;
use crate::{
    CommandReadinessRequestError, CommandReadinessTarget, LeaseId, ProviderKind, SandboxId, SandboxUserName, TenantId,
};

/// A request to check that a lease's sandbox can run commands through the sealed supervisor.
///
/// It binds the probe to one tenant, lease, provider, dispatch, and sandbox. The lease id must be 1 to
/// [`CommandReadinessRequest::MAX_LEASE_ID_LEN`] bytes of dot-separated tokens with no whitespace, control
/// characters, `*`, or `>`, so it can name a NATS subject. Construction and deserialization both enforce it, and
/// deserialization rejects unknown fields.
#[derive(Debug, Clone, Eq, PartialEq, Serialize)]
pub struct CommandReadinessRequest {
    tenant: TenantId,
    lease_id: LeaseId,
    provider: ProviderKind,
    target: CommandReadinessTarget,
    sandbox_id: SandboxId,
    run_as_user: Option<SandboxUserName>,
}

impl CommandReadinessRequest {
    /// Largest encoded request a worker accepts, in bytes.
    pub const MAX_BYTES: usize = 4096;
    /// Longest lease id a probe accepts, in bytes.
    pub const MAX_LEASE_ID_LEN: usize = 1024;

    pub fn new(
        tenant: TenantId,
        lease_id: LeaseId,
        provider: ProviderKind,
        target: CommandReadinessTarget,
        sandbox_id: SandboxId,
        run_as_user: Option<SandboxUserName>,
    ) -> Result<Self, CommandReadinessRequestError> {
        let lease = lease_id.as_str();
        if lease.is_empty() || lease.len() > Self::MAX_LEASE_ID_LEN {
            return Err(CommandReadinessRequestError::LeaseIdLength {
                max: Self::MAX_LEASE_ID_LEN,
            });
        }
        if lease.split('.').any(|token| {
            token.is_empty()
                || token.chars().any(|character| {
                    character.is_whitespace() || character.is_control() || matches!(character, '*' | '>')
                })
        }) {
            return Err(CommandReadinessRequestError::LeaseIdSubject);
        }
        Ok(Self {
            tenant,
            lease_id,
            provider,
            target,
            sandbox_id,
            run_as_user,
        })
    }

    pub fn tenant(&self) -> &TenantId {
        &self.tenant
    }

    pub fn lease_id(&self) -> &LeaseId {
        &self.lease_id
    }

    pub fn provider(&self) -> ProviderKind {
        self.provider
    }

    pub fn target(&self) -> &CommandReadinessTarget {
        &self.target
    }

    pub fn sandbox_id(&self) -> &SandboxId {
        &self.sandbox_id
    }

    pub fn run_as_user(&self) -> Option<&SandboxUserName> {
        self.run_as_user.as_ref()
    }
}

impl<'de> Deserialize<'de> for CommandReadinessRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CommandReadinessRequestWire::deserialize(deserializer)?;
        Self::new(
            wire.tenant,
            wire.lease_id,
            wire.provider,
            wire.target,
            wire.sandbox_id,
            wire.run_as_user,
        )
        .map_err(de::Error::custom)
    }
}
