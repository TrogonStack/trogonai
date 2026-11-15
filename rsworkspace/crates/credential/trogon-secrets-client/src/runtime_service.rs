use std::fmt;

use trogon_credential_domain::RuntimeServiceId;
use trogon_identity_types::{PrincipalId, PrincipalKind, SubjectPrincipal};

use super::RuntimeServiceError;

/// The runtime service a client asks on behalf of, as the `service.{id}`
/// principal of the release subject (ADR#0082 Decision 2).
///
/// The secrets service reads the runtime service back from that principal, so
/// only an id the subject carries verbatim is accepted: an id the subject
/// mapping would hash would reach the service as its digest and never match
/// the policy that names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeService(SubjectPrincipal);

impl RuntimeService {
    pub fn new(id: &RuntimeServiceId) -> Result<Self, RuntimeServiceError> {
        PrincipalId::new(id.as_str())
            .ok()
            .map(|principal| SubjectPrincipal::from_principal(PrincipalKind::Service, &principal))
            .filter(|principal| !principal.id().is_hashed())
            .map(Self)
            .ok_or(RuntimeServiceError::NotVerbatim)
    }
}

impl fmt::Display for RuntimeService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests;
