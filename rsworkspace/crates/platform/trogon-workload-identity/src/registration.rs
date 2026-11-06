use std::num::NonZeroU64;
use std::time::SystemTime;

use crate::constants::{MAX_BINDINGS, MAX_ROOTS};
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
    pub fn as_str(&self) -> &str {
        &self.0
    }
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
    approved_roots: Vec<[u8; 32]>,
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
    pub fn new(
        spiffe_id: &str,
        tenant: &str,
        project: &str,
        role: WorkloadRole,
        application_id: Option<&str>,
    ) -> Result<Self, RegistrationError> {
        let parsed = SpiffeId::new(spiffe_id).map_err(|_| RegistrationError::Binding)?;
        if parsed.to_string() != spiffe_id
            || parsed.path().is_empty()
            || !valid_name(tenant, 128, false)
            || !valid_name(project, 128, false)
            || (role == WorkloadRole::Agent) != application_id.is_some()
            || application_id.is_some_and(|id| !valid_name(id, 256, true))
        {
            return Err(RegistrationError::Binding);
        }
        let spiffe_id = parsed;
        let application_id = application_id.map(|value| ApplicationId(value.to_owned()));
        Ok(Self {
            spiffe_id,
            tenant: tenant.to_owned(),
            project: project.to_owned(),
            role,
            application_id,
        })
    }
    pub fn spiffe_id(&self) -> &SpiffeId {
        &self.spiffe_id
    }
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
    pub fn project(&self) -> &str {
        &self.project
    }
    pub fn role(&self) -> WorkloadRole {
        self.role
    }
    pub fn application_id(&self) -> Option<&ApplicationId> {
        self.application_id.as_ref()
    }
}

impl RegistrationSet {
    pub fn new(
        recovery_fence: NonZeroU64,
        registration_revision: NonZeroU64,
        trust_revision: NonZeroU64,
        expires_at: SystemTime,
        mut bindings: Vec<RegistrationBinding>,
        mut approved_roots: Vec<[u8; 32]>,
    ) -> Result<Self, RegistrationError> {
        if bindings.is_empty()
            || bindings.len() > MAX_BINDINGS
            || approved_roots.is_empty()
            || approved_roots.len() > MAX_ROOTS
        {
            return Err(RegistrationError::Bounds);
        }
        bindings.sort_by(|a, b| a.spiffe_id().cmp(b.spiffe_id()));
        if bindings
            .windows(2)
            .any(|pair| pair[0].spiffe_id() == pair[1].spiffe_id())
        {
            return Err(RegistrationError::Binding);
        }
        approved_roots.sort_unstable();
        if approved_roots.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(RegistrationError::Bounds);
        }
        Ok(Self {
            recovery_fence,
            registration_revision,
            trust_revision,
            expires_at,
            bindings,
            approved_roots,
        })
    }
    pub fn recovery_fence(&self) -> NonZeroU64 {
        self.recovery_fence
    }
    pub fn registration_revision(&self) -> NonZeroU64 {
        self.registration_revision
    }
    pub fn trust_revision(&self) -> NonZeroU64 {
        self.trust_revision
    }
    pub fn expires_at(&self) -> SystemTime {
        self.expires_at
    }
    pub fn bindings(&self) -> &[RegistrationBinding] {
        &self.bindings
    }
    pub fn approved_roots(&self) -> &[[u8; 32]] {
        &self.approved_roots
    }
}

fn valid_name(value: &str, max: usize, colon: bool) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.') || (colon && byte == b':'))
}
