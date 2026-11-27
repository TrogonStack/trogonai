use trogon_sandbox_contract::{DriveMount, SandboxSpec, SandboxTags, ServiceAccountId, SnapshotTag, TenantId};

/// Input to `SandboxEntityStore::create`. The store assigns the id.
#[derive(Debug, Clone)]
pub struct NewSandboxEntity {
    pub tenant: TenantId,
    pub name: Option<String>,
    pub created_by: Option<ServiceAccountId>,
    pub spec: SandboxSpec,
    pub metadata: SandboxTags,
    pub drives: Vec<DriveMount>,
    /// `None` uses `e-{ulid}`.
    pub persistence_tag: Option<SnapshotTag>,
}
