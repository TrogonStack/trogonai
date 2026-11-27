//! Fixtures shared by the lease tests.

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use trogon_sandbox_contract::{
    BootMode, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, LeaseTtl, OciImageRef, PlatformOptions,
    PrivilegedAccess, ProviderKind, RetentionToggles, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags,
    SecretBindings, TenantId, WorkspaceSource,
};

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) fn registry_image(reference: &str) -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(reference),
        pull_secret: None,
    }
}

/// Spec parts for `image` with the given memory and CPU budget, which a test changes before building the spec.
pub(crate) fn spec_parts(image: &str, memory_mib: u32, cpu_seconds: u32) -> anyhow::Result<SandboxSpecParts> {
    Ok(SandboxSpecParts {
        image: registry_image(image),
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(cpu_seconds).ok_or_else(|| anyhow::anyhow!("cpu budget must be positive"))?,
        memory_mib: NonZeroU32::new(memory_mib).ok_or_else(|| anyhow::anyhow!("memory must be positive"))?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: BootMode::default(),
        working_dir: None,
        user: None,
        privileged: PrivilegedAccess::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: SecretBindings::default(),
        platform: PlatformOptions::default(),
        idle_timeout_seconds: None,
        no_ttl: LeaseTtl::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: RetentionToggles::default(),
    })
}

pub(crate) fn lease_request(parts: SandboxSpecParts, key: &str) -> anyhow::Result<LeaseRequest> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("t")?,
        provider_pref: vec![ProviderKind::Firecracker],
        spec: SandboxSpec::new(parts)?,
        ttl_seconds: 60,
        idempotency_key: LeaseRequestKey::new(key)?,
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
    })
}

/// A request for `image` with a small memory and CPU budget.
pub(crate) fn request(image: &str) -> anyhow::Result<LeaseRequest> {
    lease_request(spec_parts(image, 128, 1)?, "k")
}
