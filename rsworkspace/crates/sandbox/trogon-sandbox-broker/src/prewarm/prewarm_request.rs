use super::*;

#[derive(Debug, thiserror::Error)]
pub enum PrewarmRequestError {
    #[error("prewarm sandbox spec is invalid: {0}")]
    Spec(#[from] SandboxSpecError),
    #[error("prewarm idempotency key is invalid: {0}")]
    IdempotencyKey(#[from] LeaseRequestKeyError),
}

pub fn build_prewarm_request(
    tenant: &TenantId,
    digest: &Digest,
    artifact_type: ArtifactType,
    provider: ProviderKind,
    snapshot_registry: &str,
    repository: &str,
) -> Result<LeaseRequest, PrewarmRequestError> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Snapshot {
            reference: OciArtifactRef {
                registry: snapshot_registry.to_string(),
                repository: repository.to_string(),
                digest: digest.clone(),
                artifact_type,
            },
            parent: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: PREWARM_CPU_SECONDS,
        memory_mib: PREWARM_MEMORY_MIB,
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
        operation_id: None,
        tenant: tenant.clone(),
        provider_pref: vec![provider],
        spec,
        ttl_seconds: PREWARM_LEASE_TTL_SECONDS,
        idempotency_key: LeaseRequestKey::new(format!("prewarm-{}", generated_ulid()))?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: LeasePrewarm::Prewarm,
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Persistent,
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}
