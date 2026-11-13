use trogon_credential_domain::RuntimeServiceId;
use trogon_identity_types::{PrincipalKind, SubjectPrincipal};

use super::{ReleaseCallerError, TenantAccount};

/// Who called the release endpoint, read from the subject the request arrived
/// on.
///
/// The tenant is the subject's first token, which the PLATFORM export pins to
/// the importing account (ADR#0081 Decision 3). The runtime service is the
/// `service.{id}` principal after the domain and version (ADR#0082 Decision
/// 2). Nothing the caller wrote in its payload or headers takes part, so no
/// part of the caller is self-asserted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseCaller {
    tenant: TenantAccount,
    runtime_service: RuntimeServiceId,
}

impl ReleaseCaller {
    /// Reads the caller from
    /// `{tenant}.secrets.v1.service.{id}.SecretExchangeService.ResolveForRelease`.
    pub fn from_subject(subject: &str) -> Result<Self, ReleaseCallerError> {
        let tokens: Vec<&str> = subject.split('.').collect();
        let [
            tenant,
            "secrets",
            "v1",
            kind,
            id,
            "SecretExchangeService",
            "ResolveForRelease",
        ] = tokens[..]
        else {
            return Err(ReleaseCallerError::OutsideEndpoint);
        };
        let tenant = TenantAccount::parse(tenant).map_err(ReleaseCallerError::Tenant)?;
        let principal = SubjectPrincipal::from_tokens(kind, id).map_err(ReleaseCallerError::Principal)?;
        if principal.kind() != PrincipalKind::Service {
            return Err(ReleaseCallerError::NotAService);
        }
        let runtime_service =
            RuntimeServiceId::new(principal.principal_id()).map_err(ReleaseCallerError::RuntimeService)?;
        Ok(Self {
            tenant,
            runtime_service,
        })
    }

    pub fn tenant(&self) -> &TenantAccount {
        &self.tenant
    }

    pub fn runtime_service(&self) -> &RuntimeServiceId {
        &self.runtime_service
    }
}

#[cfg(test)]
mod tests;
