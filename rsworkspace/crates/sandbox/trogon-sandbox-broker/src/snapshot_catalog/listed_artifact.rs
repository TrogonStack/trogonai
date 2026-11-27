use trogon_sandbox_contract::{OciArtifactRef, TenantId};

/// An artifact found in the registry, with the tenant that owns it.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ListedArtifact {
    pub tenant: TenantId,
    pub reference: OciArtifactRef,
}
