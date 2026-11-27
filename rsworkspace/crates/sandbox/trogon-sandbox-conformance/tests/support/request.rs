//! A small lease request the hand-written tests send.

use trogon_sandbox_contract::{
    LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};

use super::Fallible;

/// An Alpine sandbox with one CPU minute and 512 MiB.
pub fn spec() -> Fallible<SandboxSpec> {
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("alpine:3.20"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: Default::default(),
        cpu_seconds: std::num::NonZeroU32::MIN.saturating_add(59),
        memory_mib: std::num::NonZeroU32::MIN.saturating_add(511),
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?)
}

/// An ephemeral, unnamed lease of [`spec`] for `tenant`.
pub fn lease_request(tenant: TenantId, key: &str) -> Fallible<LeaseRequest> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant,
        provider_pref: vec![ProviderKind::Mock],
        spec: spec()?,
        ttl_seconds: 120,
        idempotency_key: LeaseRequestKey::new(key)?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: LeasePrewarm::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::default(),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}
