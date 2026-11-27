use trogon_sandbox_contract::{LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderKind};

use crate::AuthorityResourceId;

/// A lease a worker holds, as the host reports it.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum HeldLease {
    /// A lease bound to an exact assignment.
    Targeted(LeaseAttemptIdentity),
    /// A provider resource that carries no assignment.
    Provider {
        /// The lease the resource belongs to.
        lease_id: LeaseId,
        /// The provider that holds it.
        provider: ProviderKind,
        /// The resource.
        resource_id: AuthorityResourceId,
    },
}

impl HeldLease {
    /// The lease id.
    pub fn lease_id(&self) -> &LeaseId {
        match self {
            Self::Targeted(identity) => &identity.lease_id,
            Self::Provider { lease_id, .. } => lease_id,
        }
    }

    /// The epoch of the held assignment, when it has one.
    pub fn epoch(&self) -> Option<LeaseEpoch> {
        match self {
            Self::Targeted(identity) => identity.target.epoch,
            Self::Provider { .. } => None,
        }
    }
}
