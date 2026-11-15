use trogon_credential_domain::{CompleteReleaseRequest, CredentialKey, InjectionLocation, RuntimeServiceId};

use super::{AuditCorrelationId, ReleaseDestination, TenantAccount};

/// A runtime service asking for one credential, as its NATS caller parsed it.
///
/// The tenant and the runtime service come from the connection, never from a
/// field the requester wrote, so no part of the identity here is self-asserted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseRequest {
    tenant: TenantAccount,
    runtime_service: RuntimeServiceId,
    credential: CredentialKey,
    destination: ReleaseDestination,
    location: InjectionLocation,
    correlation_id: AuditCorrelationId,
}

impl ReleaseRequest {
    pub fn new(
        tenant: TenantAccount,
        runtime_service: RuntimeServiceId,
        credential: CredentialKey,
        destination: ReleaseDestination,
        location: InjectionLocation,
        correlation_id: AuditCorrelationId,
    ) -> Self {
        Self {
            tenant,
            runtime_service,
            credential,
            destination,
            location,
            correlation_id,
        }
    }

    pub fn tenant(&self) -> &TenantAccount {
        &self.tenant
    }

    pub fn runtime_service(&self) -> &RuntimeServiceId {
        &self.runtime_service
    }

    pub fn credential(&self) -> &CredentialKey {
        &self.credential
    }

    pub fn destination(&self) -> &ReleaseDestination {
        &self.destination
    }

    pub fn location(&self) -> &InjectionLocation {
        &self.location
    }

    pub fn correlation_id(&self) -> &AuditCorrelationId {
        &self.correlation_id
    }

    /// The part of the request a release policy judges.
    pub fn policy_request(&self) -> CompleteReleaseRequest {
        CompleteReleaseRequest::new(
            self.runtime_service.clone(),
            self.destination.host().clone(),
            self.location.clone(),
        )
    }
}

#[cfg(test)]
mod tests;
