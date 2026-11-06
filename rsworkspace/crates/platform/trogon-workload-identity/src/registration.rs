use std::num::NonZeroU64;
use std::time::SystemTime;

use spiffe::SpiffeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadRole {
    EgressProxy,
    Agent,
    Supervisor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplicationId(String);

impl ApplicationId {
    pub fn as_str(&self) -> &str { &self.0 }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationBinding {
    spiffe_id: SpiffeId,
    tenant: String,
    project: String,
    role: WorkloadRole,
    application_id: Option<ApplicationId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationSet {
    recovery_fence: NonZeroU64,
    registration_revision: NonZeroU64,
    trust_revision: NonZeroU64,
    expires_at: SystemTime,
    bindings: Vec<RegistrationBinding>,
    approved_roots: Vec<[u8;32]>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RegistrationError {
    #[error("registration contains an invalid identity or scope")]
    Binding,
    #[error("registration resource bounds are invalid")]
    Bounds,
    #[error("registration freshness is invalid")]
    Freshness,
    #[error("registration recovery fence or configured scope differs")]
    Scope,
    #[error("registration revision is stale or equivocal")]
    Revision,
    #[error("identity state is unavailable")]
    Unavailable,
}

impl RegistrationBinding {
    pub fn new(spiffe_id: &str, tenant: &str, project: &str, role: WorkloadRole, application_id: Option<&str>) -> Result<Self, RegistrationError> {
        let spiffe_id = SpiffeId::new(spiffe_id).map_err(|_| RegistrationError::Binding)?;
        let application_id = application_id.map(|value| ApplicationId(value.to_owned()));
        Ok(Self { spiffe_id, tenant: tenant.to_owned(), project: project.to_owned(), role, application_id })
    }
    pub fn spiffe_id(&self) -> &SpiffeId { &self.spiffe_id }
    pub fn tenant(&self) -> &str { &self.tenant }
    pub fn project(&self) -> &str { &self.project }
    pub fn role(&self) -> WorkloadRole { self.role }
    pub fn application_id(&self) -> Option<&ApplicationId> { self.application_id.as_ref() }
}

impl RegistrationSet {
    pub fn new(recovery_fence: NonZeroU64, registration_revision: NonZeroU64, trust_revision: NonZeroU64, expires_at: SystemTime, bindings: Vec<RegistrationBinding>, approved_roots: Vec<[u8;32]>) -> Result<Self, RegistrationError> {
        Ok(Self { recovery_fence, registration_revision, trust_revision, expires_at, bindings, approved_roots })
    }
    pub fn recovery_fence(&self) -> NonZeroU64 { self.recovery_fence }
    pub fn registration_revision(&self) -> NonZeroU64 { self.registration_revision }
    pub fn trust_revision(&self) -> NonZeroU64 { self.trust_revision }
    pub fn expires_at(&self) -> SystemTime { self.expires_at }
    pub fn bindings(&self) -> &[RegistrationBinding] { &self.bindings }
    pub fn approved_roots(&self) -> &[[u8;32]] { &self.approved_roots }
}
