use super::*;

use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, DomainRule, DomainRuleAction, DormancyCapability, EgressCapability,
    EgressEnforcementLevel, EnvVarName, HostMountAccess, InjectHosts, IsolationCapability, IsolationTier,
    LifecycleCapability, LiteralPath, NativeSnapshotId, NativeSnapshotReceipt, NativeSnapshotRef, OciArtifactRef,
    PerformanceProfile, PlacementCapability, PullSecretId, RemoteMount, ResourceCapability, ResourceLimitMechanism,
    ResourceRange, SecretBinding, SecretBindingParts, SecretTarget, ShmSizeMib, SnapshotCapability, VolumeCapability,
};

pub(super) async fn make_core_with_allowlist(
    provider_response: Bytes,
    allowlist: Arc<InMemoryImageAllowList>,
    enforce: bool,
) -> anyhow::Result<BrokerCore> {
    CoreParts {
        image_allowlist: allowlist,
        config: BrokerConfig {
            enforce_image_allowlist: enforce,
            ..default_config()
        },
        ..CoreParts::new(Arc::new(MockBus { provider_response }))
    }
    .build()
}

// --- image readiness integration tests ---

pub(super) struct IncompatiblePlatformChecker;

#[async_trait::async_trait]
impl ImageReadinessChecker for IncompatiblePlatformChecker {
    async fn check(&self, _image: &trogon_sandbox_contract::OciImageRef) -> anyhow::Result<ImageReadiness> {
        Ok(ImageReadiness::Incompatible {
            reason: "no linux/amd64 manifest".to_string(),
        })
    }
}

pub(super) struct PreparingChecker;

#[async_trait::async_trait]
impl ImageReadinessChecker for PreparingChecker {
    async fn check(&self, _image: &trogon_sandbox_contract::OciImageRef) -> anyhow::Result<ImageReadiness> {
        Ok(ImageReadiness::Preparing {
            reason: "manifest not published yet".to_string(),
        })
    }
}

pub(super) struct FailingChecker;

#[async_trait::async_trait]
impl ImageReadinessChecker for FailingChecker {
    async fn check(&self, _image: &trogon_sandbox_contract::OciImageRef) -> anyhow::Result<ImageReadiness> {
        anyhow::bail!("registry unreachable")
    }
}

// --- tenant limits integration tests ---

pub(super) async fn make_core_with_limits(
    provider_response: Bytes,
    limits: Arc<InMemoryTenantLimits>,
    default_max_cpu: u32,
    default_max_mem: u32,
) -> anyhow::Result<BrokerCore> {
    CoreParts {
        tenant_limits: limits,
        config: BrokerConfig {
            default_max_cpu_per_request: default_max_cpu,
            default_max_memory_mib_per_request: default_max_mem,
            ..default_config()
        },
        ..CoreParts::new(Arc::new(MockBus { provider_response }))
    }
    .build()
}

pub(super) async fn per_tag_core(max_active_leases: u32) -> anyhow::Result<(BrokerCore, TenantId)> {
    let limits = Arc::new(InMemoryTenantLimits::new());
    let tenant = make_request(vec![ProviderKind::Mock])?.tenant;
    limits
        .insert(
            &tenant,
            TenantLimit {
                max_cpu_seconds_per_request: 3600,
                max_memory_mib_per_request: 8192,
                per_tag_quota: Some(crate::limits::PerTagQuota {
                    tag_key: "user".to_string(),
                    max_active_leases,
                }),
            },
        )
        .await;
    let core = make_core_with_limits(mock_provider_response()?, limits, 3600, 8192).await?;
    Ok((core, tenant))
}

pub(super) fn user_request(user: Option<&str>, key: &str) -> anyhow::Result<LeaseRequest> {
    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.idempotency_key = LeaseRequestKey::new(key)?;
    if let Some(user) = user {
        req.tags = SandboxTags::new(BTreeMap::from([("user".to_string(), user.to_string())]))?;
    }
    Ok(req)
}

// --- worker registry integration tests ---

pub(super) fn make_core_with_registry(
    provider_response: Bytes,
    registry: Arc<InMemoryWorkerRegistry>,
    enforce: bool,
) -> anyhow::Result<BrokerCore> {
    CoreParts {
        worker_registry: registry,
        config: BrokerConfig {
            enforce_worker_registry: enforce,
            ..default_config()
        },
        ..CoreParts::new(Arc::new(MockBus { provider_response }))
    }
    .build()
}

pub(super) async fn inject_profile_worker(
    registry: &InMemoryWorkerRegistry,
    provider: ProviderKind,
    supports_egress: bool,
) -> anyhow::Result<()> {
    let support = CapabilitySupport::from(supports_egress);
    let mut profile = default_capability_profile();
    profile.provider = provider;
    profile.isolation.supports_domain_allowlist = support;
    profile.isolation.supports_tls_intercept = support;
    profile.isolation.supports_secret_substitution = support;
    profile.isolation.supports_fs_policy = CapabilitySupport::Unsupported;
    registry
        .inject(worker_heartbeat(&format!("{provider:?}-w1"), provider, profile)?)
        .await;
    Ok(())
}

fn example_network_policy(
    domain: &str,
    denied: Vec<trogon_sandbox_contract::DeniedDomain>,
) -> anyhow::Result<trogon_sandbox_contract::NetworkPolicy> {
    Ok(trogon_sandbox_contract::NetworkPolicy::UserDefined {
        rules: vec![DomainRule {
            domain: domain.parse()?,
            action: DomainRuleAction::PassThrough,
        }],
        allow_ranges: vec![],
        deny_ranges: vec![],
        denied_domains: denied,
        opaque_tunnels: trogon_sandbox_contract::OpaqueTunnelPolicy::Allow,
        request_rules: vec![],
    })
}

pub(super) fn user_defined_request(provider_pref: Vec<ProviderKind>, key: &str) -> anyhow::Result<LeaseRequest> {
    let policy = example_network_policy("example.com", vec![])?;
    let mut req = edit_request_spec(make_request(provider_pref)?, |parts| {
        parts.network_policy = Some(policy);
    })?;
    req.idempotency_key = LeaseRequestKey::new(key)?;
    Ok(req)
}

pub(super) async fn inject_egress_level_worker(
    registry: &InMemoryWorkerRegistry,
    provider: ProviderKind,
    level: EgressEnforcementLevel,
) -> anyhow::Result<()> {
    let mut profile = default_capability_profile();
    profile.provider = provider;
    profile.isolation.egress_enforcement = level;
    registry
        .inject(worker_heartbeat(&format!("{provider:?}-egress"), provider, profile)?)
        .await;
    Ok(())
}

pub(super) fn deny_all_request(provider: ProviderKind, key: &str) -> anyhow::Result<LeaseRequest> {
    let mut req = edit_request_spec(make_request(vec![provider])?, |parts| {
        parts.network_policy = Some(trogon_sandbox_contract::NetworkPolicy::DenyAll);
    })?;
    req.idempotency_key = LeaseRequestKey::new(key)?;
    Ok(req)
}

pub(super) async fn sized_lease(
    resources: ResourceCapability,
    enforce: bool,
    edit: impl FnOnce(&mut SandboxSpecParts),
) -> anyhow::Result<LeaseResponse> {
    let registry = InMemoryWorkerRegistry::new(std::time::Duration::from_secs(30));
    let mut profile = default_capability_profile();
    profile.provider = ProviderKind::Subprocess;
    profile.resources = resources;
    registry
        .inject(worker_heartbeat("sized-w1", ProviderKind::Subprocess, profile)?)
        .await;
    let core = make_core_with_registry(mock_provider_response()?, registry, enforce)?;
    let mut req = edit_request_spec(make_request(vec![ProviderKind::Subprocess])?, edit)?;
    req.idempotency_key = LeaseRequestKey::new("sized")?;
    Ok(core.handle_lease_request(req).await)
}

pub(super) fn sized_resources() -> anyhow::Result<ResourceCapability> {
    Ok(ResourceCapability {
        vcpu_millis: Some(ResourceRange::new(250, 4000)?),
        disk_mib: None,
        reports_usage_gauges: CapabilitySupport::Unsupported,
    })
}

fn secret_binding_with_target(
    key: &str,
    mode: trogon_sandbox_contract::SecretMode,
    target: SecretTarget,
) -> anyhow::Result<LeaseRequest> {
    let policy = example_network_policy("*.example.com", vec!["blocked.example.com".parse()?])?;
    let binding = SecretBinding::new(SecretBindingParts {
        name: "api".parse()?,
        target,
        mode,
        extract: None,
        mask_duplicates: Default::default(),
        on_extract_no_match: Default::default(),
        strip_response_headers: Default::default(),
        inject_hosts: InjectHosts::new(vec!["api.example.com".parse()?])?,
        inject_scope: Default::default(),
        source: trogon_sandbox_contract::CredentialRequest {
            provider: trogon_sandbox_contract::CredentialProvider::Registry,
            pull_secret_id: PullSecretId::new("s"),
            scope: trogon_sandbox_contract::CredentialScope::RegistryPull {
                repository_glob: "*".to_string(),
            },
            ttl_seconds: 60,
            requester: None,
        },
    })?;
    let bindings = trogon_sandbox_contract::SecretBindings::new(vec![binding])?;
    let mut req = edit_request_spec(make_request(vec![ProviderKind::LocalDocker])?, |parts| {
        parts.network_policy = Some(policy);
        parts.secret_bindings = bindings;
    })?;
    req.idempotency_key = LeaseRequestKey::new(key)?;
    Ok(req)
}

pub(super) fn secret_binding_request(
    key: &str,
    mode: trogon_sandbox_contract::SecretMode,
) -> anyhow::Result<LeaseRequest> {
    secret_binding_with_target(key, mode, SecretTarget::Env(EnvVarName::new("API_TOKEN")?))
}

pub(super) fn masked_file_request(key: &str) -> anyhow::Result<LeaseRequest> {
    secret_binding_with_target(
        key,
        trogon_sandbox_contract::SecretMode::Mask,
        SecretTarget::File(LiteralPath::new("/etc/creds")?),
    )
}

pub(super) fn shm_override_request(key: &str) -> anyhow::Result<LeaseRequest> {
    let shm = ShmSizeMib::new(1024)?;
    let mut req = edit_request_spec(make_request(vec![ProviderKind::Subprocess])?, |parts| {
        parts.platform.container.shm_size_mib = Some(shm);
    })?;
    req.idempotency_key = LeaseRequestKey::new(key)?;
    Ok(req)
}

// --- host-mount allow-list integration tests ---

pub(super) fn make_host_mount_request(provider_pref: Vec<ProviderKind>, path: &str) -> anyhow::Result<LeaseRequest> {
    let mut req = edit_request_spec(make_request(provider_pref)?, |parts| {
        parts.workspace = WorkspaceSource::HostMount {
            path: path.to_string(),
            access: HostMountAccess::from(false),
        };
    })?;
    req.idempotency_key = LeaseRequestKey::new("hm-test-key")?;
    Ok(req)
}

pub(super) fn test_digest() -> anyhow::Result<trogon_sandbox_contract::Digest> {
    Ok(trogon_sandbox_contract::Digest::new(format!(
        "sha256:{}",
        "a".repeat(64)
    ))?)
}

pub(super) fn restore_capable_profile(provider: ProviderKind) -> ProviderCapabilityProfile {
    let yes = CapabilitySupport::Supported;
    let no = CapabilitySupport::Unsupported;
    ProviderCapabilityProfile {
        provider,
        snapshot: SnapshotCapability {
            can_snapshot: yes,
            can_checkpoint_running: no,
            can_restore: yes,
            supported_artifact_types: vec![ArtifactType::HyperlightSnapshotV1],
            layered_deltas: no,
            supports_native_snapshots: no,
            supports_in_place_rollback: no,
            max_snapshot_size_bytes: None,
        },
        volume: VolumeCapability {
            supported_modes: vec![],
            supports_s3_backend: no,
            supports_fuse_mounts: no,
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::WasmMicroVm,
            supported_tiers: vec![IsolationTier::WasmMicroVm],
            egress_enforcement: EgressEnforcementLevel::BestEffort,
            supports_privileged: no,
            supports_sandbox_users: no,
            supports_interactive_stdin: no,
            supports_named_processes: no,
            supports_process_readiness: no,
            supports_graceful_stop: no,
            supports_command_restart: no,
            supports_command_cancellation: no,
            supports_command_launch_cancellation: no,
            supports_port_preview: no,
            supports_expose_after_create: no,
            supports_evidence_streaming: no,
            supports_output_streaming: no,
            supports_domain_allowlist: no,
            supports_tls_intercept: no,
            supports_secret_substitution: no,
            supports_secret_bindings: no,
            supports_masked_files: no,
            supports_fs_policy: no,
            supports_file_ops: no,
            supports_file_search: no,
            supports_file_archive: no,
            supports_output_redaction: no,
            supports_terminal: no,
            resource_limits: ResourceLimitMechanism::None,
            egress: EgressCapability::NONE,
            supports_hardening_overrides: no,
            supports_change_tracking: no,
            supports_file_watch: no,
            supports_persistent_runtime: no,
            supports_dir_transfer: no,
            supports_browser: no,
            supports_network_policy_update: no,
            supports_sandbox_groups: no,
        },
        lifecycle: LifecycleCapability {
            supports_warm_pool: no,
            max_warm_pool_size: None,
            supports_named_tags: no,
            supports_prewarm_per_tenant: no,
            supports_pause_resume: no,
            supports_native_auto_stop: no,
            max_lease_lifetime_seconds: None,
            dormancy: DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 50,
            cold_restore_p50_ms: 100,
            max_concurrent_sandboxes: Some(8),
        },
        resources: Default::default(),
        placement: Default::default(),
        ingress: Default::default(),
    }
}

pub(super) fn make_snapshot_request(provider_pref: Vec<ProviderKind>) -> anyhow::Result<LeaseRequest> {
    let image = SandboxImage::Snapshot {
        reference: OciArtifactRef {
            registry: "snapshots.trogon.internal".to_string(),
            repository: "snapshots/acme/test".to_string(),
            digest: test_digest()?,
            artifact_type: ArtifactType::HyperlightSnapshotV1,
        },
        parent: None,
    };
    let mut req = edit_request_spec(make_request(provider_pref)?, |parts| {
        parts.image = image;
    })?;
    req.idempotency_key = LeaseRequestKey::new("snapshot-test")?;
    Ok(req)
}

pub(super) struct NativeSnapshotBus {
    pub(super) requested_names: std::sync::Mutex<Vec<Option<String>>>,
    pub(super) dispatch_response: Bytes,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for NativeSnapshotBus {
    async fn request(&self, subject: &str, payload: Bytes) -> anyhow::Result<Bytes> {
        if subject.contains("checkpoint_running") {
            let request: CheckpointRunningRequest = serde_json::from_slice(&payload)?;
            locked(&self.requested_names)?.push(request.native_name.clone());
            let name = request.native_name.unwrap_or_else(|| "unnamed".to_string());
            let outcome = SnapshotExportOutcome::Native(NativeSnapshotReceipt {
                reference: NativeSnapshotRef {
                    provider: ProviderKind::Mock,
                    id: NativeSnapshotId::new(name)?,
                    expires_at: None,
                },
                bytes: None,
            });
            Ok(Bytes::from(serde_json::to_vec(&outcome)?))
        } else {
            Ok(self.dispatch_response.clone())
        }
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn native_snapshot_profile(provider: ProviderKind, native: bool) -> ProviderCapabilityProfile {
    let mut profile = restore_capable_profile(provider);
    profile.snapshot.can_checkpoint_running = CapabilitySupport::Supported;
    profile.snapshot.supported_artifact_types = vec![];
    profile.snapshot.supports_native_snapshots = CapabilitySupport::from(native);
    profile
}

pub(super) async fn native_snapshot_core(
    provider: ProviderKind,
    native: bool,
) -> anyhow::Result<(BrokerCore, Arc<NativeSnapshotBus>)> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let profile = native_snapshot_profile(provider, native);
    registry
        .inject(worker_heartbeat("native-worker", provider, profile)?)
        .await;
    let bus = Arc::new(NativeSnapshotBus {
        requested_names: std::sync::Mutex::new(Vec::new()),
        dispatch_response: mock_provider_response()?,
    });
    let core = CoreParts {
        worker_registry: registry,
        config: BrokerConfig {
            enforce_image_allowlist: true,
            enforce_worker_registry: true,
            ..default_config()
        },
        ..CoreParts::new(bus.clone())
    }
    .build()?;
    Ok((core, bus))
}

pub(super) fn native_restore_request(
    tenant: &TenantId,
    provider_pref: Vec<ProviderKind>,
    reference: NativeSnapshotRef,
) -> anyhow::Result<LeaseRequest> {
    let mut req = make_snapshot_request(provider_pref)?;
    req.tenant = tenant.clone();
    req.spec = with_spec_parts(req.spec, |parts| {
        parts.image = SandboxImage::NativeSnapshot { reference };
    })?;
    req.idempotency_key = LeaseRequestKey::new(format!(
        "native-restore-{}-{}",
        req.provider_pref.len(),
        reference_key(req.spec.image())
    ))?;
    Ok(req)
}

pub(super) fn reference_key(image: &SandboxImage) -> String {
    match image {
        SandboxImage::NativeSnapshot { reference } => {
            format!(
                "{:?}-{}-{}",
                reference.provider,
                reference.id,
                reference.expires_at.unwrap_or(0)
            )
        }
        other => format!("{other:?}"),
    }
}

// --- FUSE remote mount validation tests ---

pub(super) fn make_mount_request(mounts: Vec<RemoteMount>) -> anyhow::Result<LeaseRequest> {
    let mut req = edit_request_spec(make_request(vec![ProviderKind::Mock])?, |parts| {
        parts.mounts = mounts;
    })?;
    req.idempotency_key = LeaseRequestKey::new("fuse-test")?;
    Ok(req)
}

// --- network policy validation integration tests ---

pub(super) fn make_network_policy_request(
    network_policy: trogon_sandbox_contract::NetworkPolicy,
) -> anyhow::Result<LeaseRequest> {
    let mut req = edit_request_spec(make_request(vec![ProviderKind::Mock])?, |parts| {
        parts.network_policy = Some(network_policy);
    })?;
    req.idempotency_key = LeaseRequestKey::new("network-policy-test")?;
    Ok(req)
}

/// A bus that replies to `provider_dispatch` subjects with a lease grant
/// and to `provider_purge_named` subjects with a configurable
/// [`PurgeNamedResponse`], so delete_named_sandbox tests can grant a named
/// lease without that reply leaking into the purge_named round trip.
pub(super) struct PurgeNamedBus {
    pub(super) lease_response: Bytes,
    pub(super) purge_response: Bytes,
}

#[async_trait::async_trait]
impl crate::bus::BrokerBus for PurgeNamedBus {
    async fn request(&self, subject: &str, _payload: Bytes) -> anyhow::Result<Bytes> {
        if subject.contains("purge_named") {
            Ok(self.purge_response.clone())
        } else {
            Ok(self.lease_response.clone())
        }
    }

    async fn publish(&self, _subject: &str, _payload: Bytes) -> anyhow::Result<()> {
        Ok(())
    }
}

pub(super) fn make_core_with_purge_named_response(purge_response: PurgeNamedResponse) -> anyhow::Result<BrokerCore> {
    make_core_with_bus(Arc::new(PurgeNamedBus {
        lease_response: mock_provider_response()?,
        purge_response: Bytes::from(serde_json::to_vec(&purge_response)?),
    }))
}

// --- vendor passthrough and region placement ---

pub(super) fn placement_core(
    bus: Arc<dyn crate::bus::BrokerBus>,
    registry: Arc<InMemoryWorkerRegistry>,
    enforce: bool,
    allow_native: bool,
) -> anyhow::Result<BrokerCore> {
    let core = CoreParts {
        worker_registry: registry,
        config: BrokerConfig {
            enforce_worker_registry: enforce,
            ..default_config()
        },
        ..CoreParts::new(bus)
    }
    .build()?;
    Ok(if allow_native {
        core.with_native_policy(Arc::new(crate::native_policy::TenantAllowListNativePassthrough::new([
            TenantId::new("test-tenant")?,
        ])))
    } else {
        core
    })
}

pub(super) async fn inject_placement_worker(
    registry: &InMemoryWorkerRegistry,
    provider: ProviderKind,
    worker_id: &str,
    regions: &[&str],
    supports_native_passthrough: bool,
) -> anyhow::Result<()> {
    let mut profile = default_capability_profile();
    profile.provider = provider;
    profile.placement = PlacementCapability {
        regions: regions.iter().map(|r| r.to_string()).collect(),
        supports_native_passthrough: CapabilitySupport::from(supports_native_passthrough),
        ..Default::default()
    };
    registry.inject(worker_heartbeat(worker_id, provider, profile)?).await;
    Ok(())
}
