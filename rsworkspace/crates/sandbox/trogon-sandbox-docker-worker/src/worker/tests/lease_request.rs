//! A lease request for the worker unit tests.

use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;

use trogon_sandbox_contract::{
    LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};

/// A request for a plain registry image under tenant `test-tenant`.
pub(super) fn sample_lease_request() -> Result<LeaseRequest, Box<dyn Error>> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("img:latest"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(2).ok_or("cpu seconds")?,
        memory_mib: NonZeroU32::new(512).ok_or("memory")?,
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
    })?;
    Ok(LeaseRequest {
        tenant: TenantId::new("test-tenant")?,
        provider_pref: vec![ProviderKind::LocalDocker],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new("idem-1")?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: LeasePrewarm::Regular,
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: Vec::new(),
        operation_id: None,
    })
}
