//! Shared fixtures for the worker unit tests.

use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;

use trogon_sandbox_contract::{
    LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};

pub(crate) type TestResult = Result<(), Box<dyn Error>>;

/// The fields of a spec for a plain registry image, for a test to change before it builds the spec.
pub(crate) fn spec_parts() -> Result<SandboxSpecParts, Box<dyn Error>> {
    Ok(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("alpine:3.20"),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(60).ok_or("cpu seconds")?,
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
    })
}

pub(crate) fn spec() -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec::new(spec_parts()?)?)
}

/// A request for tenant `acme` with the given idempotency key.
pub(crate) fn lease_request(key: &str) -> Result<LeaseRequest, Box<dyn Error>> {
    lease_request_with(key, spec()?)
}

pub(crate) fn lease_request_with(key: &str, spec: SandboxSpec) -> Result<LeaseRequest, Box<dyn Error>> {
    Ok(LeaseRequest {
        tenant: TenantId::new("acme")?,
        provider_pref: vec![ProviderKind::LocalDocker],
        spec,
        ttl_seconds: 600,
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
        operation_id: None,
    })
}
