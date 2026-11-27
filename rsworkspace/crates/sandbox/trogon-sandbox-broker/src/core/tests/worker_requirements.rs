use super::*;

use crate::core::capability_inference::infer_capability_requirements;
use crate::placement_gate::egress_requirements_only;
use crate::profile_satisfaction::ProfileSatisfies;
use trogon_sandbox_contract::{
    CapabilityNeed, CapabilityRequirements, EgressEnforcementLevel, ExtraCapability, FsPathEntry, FsPolicy,
    InjectHosts, IsolationTier, LeaseTtl, LiteralPath, NetworkPolicy, Port, PrivilegedAccess, SecretBinding,
    SecretBindingParts, SecretBindings, SecretMode,
};

#[tokio::test]
async fn enforce_true_zero_workers_returns_no_provider_available_and_refunds_quota() -> TestResult {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let quota = Arc::new(InMemoryQuotaStore::new());

    // Counting wrapper so we can assert refund happened
    struct TrackingQuota {
        inner: Arc<InMemoryQuotaStore>,
        released: Arc<AtomicU32>,
    }

    #[async_trait::async_trait]
    impl QuotaStore for TrackingQuota {
        async fn check_and_reserve(
            &self,
            tenant: &TenantId,
            cpu: u32,
            ttl: u32,
            cpu_cap: u32,
            wall_cap: u32,
        ) -> Result<(), LeaseError> {
            self.inner.check_and_reserve(tenant, cpu, ttl, cpu_cap, wall_cap).await
        }

        async fn release(&self, tenant: &TenantId, cpu: u32, ttl: u32) -> Result<(), crate::quota::QuotaStoreError> {
            self.released.fetch_add(1, Ordering::SeqCst);
            self.inner.release(tenant, cpu, ttl).await
        }

        async fn active_lease_count(&self, tenant: &TenantId) -> Result<u32, crate::quota::QuotaStoreError> {
            self.inner.active_lease_count(tenant).await
        }

        async fn check_active_lease_cap(&self, tenant: &TenantId) -> Result<(), LeaseError> {
            self.inner.check_active_lease_cap(tenant).await
        }

        async fn increment_active_leases(&self, tenant: &TenantId) -> Result<(), crate::quota::QuotaStoreError> {
            self.inner.increment_active_leases(tenant).await
        }

        async fn decrement_active_leases(&self, tenant: &TenantId) -> Result<(), crate::quota::QuotaStoreError> {
            self.inner.decrement_active_leases(tenant).await
        }

        async fn record_snapshot_published(
            &self,
            tenant: &TenantId,
            bytes: u64,
            default_snapshot: crate::quota::SnapshotQuota,
        ) -> Result<(), LeaseError> {
            self.inner
                .record_snapshot_published(tenant, bytes, default_snapshot)
                .await
        }

        async fn record_snapshot_deleted(
            &self,
            tenant: &TenantId,
            bytes: u64,
        ) -> Result<(), crate::quota::QuotaStoreError> {
            self.inner.record_snapshot_deleted(tenant, bytes).await
        }

        async fn check_prewarm_dispatch(&self, tenant: &TenantId, cpu_seconds: u32) -> Result<(), LeaseError> {
            self.inner.check_prewarm_dispatch(tenant, cpu_seconds).await
        }

        async fn record_prewarm_dispatch(
            &self,
            tenant: &TenantId,
            cpu_seconds: u32,
        ) -> Result<(), crate::quota::QuotaStoreError> {
            self.inner.record_prewarm_dispatch(tenant, cpu_seconds).await
        }

        async fn record_prewarm_release(
            &self,
            tenant: &TenantId,
            cpu_seconds: u32,
        ) -> Result<(), crate::quota::QuotaStoreError> {
            self.inner.record_prewarm_release(tenant, cpu_seconds).await
        }
    }

    let released = Arc::new(AtomicU32::new(0));
    let tracking_quota = Arc::new(TrackingQuota {
        inner: Arc::clone(&quota),
        released: Arc::clone(&released),
    });

    let core = BrokerCore::new(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        tracking_quota,
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        registry,
        default_pins(),
        default_tag_store(),
        default_resolved_digests(),
        default_drive_store(),
        default_named_leases(),
        BrokerConfig {
            default_cpu_cap: 3600,
            default_wall_cap: 7200,
            enforce_image_allowlist: false,
            enforce_worker_registry: true,
            default_max_cpu_per_request: 3600,
            default_max_memory_mib_per_request: 8192,
            default_active_lease_cap: 50,
            snapshot_registry: "snapshots.trogon.internal".to_string(),
            provider_dispatch_timeout: Duration::from_secs(600),
        },
        BrokerMetrics::new()?,
    );

    let req = make_request(vec![ProviderKind::K8sMicroVm])?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::NoProviderAvailable)),
        "expected NoProviderAvailable, got {response:?}"
    );
    assert_eq!(
        released.load(Ordering::SeqCst),
        1,
        "quota must be refunded when no workers available"
    );
    Ok(())
}

/// Injects a worker of `provider` advertising `profile` into `registry`.
async fn inject_worker(
    registry: &InMemoryWorkerRegistry,
    worker_id: &str,
    provider: ProviderKind,
    profile: ProviderCapabilityProfile,
) -> anyhow::Result<()> {
    registry.inject(worker_heartbeat(worker_id, provider, profile)?).await;
    Ok(())
}

/// A Docker worker profile used by the secret-binding tests.
fn docker_profile() -> ProviderCapabilityProfile {
    let mut profile = default_capability_profile();
    profile.provider = ProviderKind::LocalDocker;
    profile
}

fn thirty_second_registry() -> Arc<InMemoryWorkerRegistry> {
    InMemoryWorkerRegistry::new(Duration::from_secs(30))
}

/// The denial requirements of a capability-unsatisfied response, or an error
/// naming what came back instead.
fn unsatisfied(response: LeaseResponse) -> Result<CapabilityRequirements, Box<dyn Error>> {
    match response {
        LeaseResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements }) => Ok(requirements),
        other => Err(format!("expected NoWorkerSatisfiesCapabilities, got {other:?}").into()),
    }
}

#[tokio::test]
async fn enforce_false_zero_workers_still_grants() -> TestResult {
    let registry = InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30));
    let core = make_core_with_registry(mock_provider_response()?, registry, false)?;
    let req = make_request(vec![ProviderKind::K8sMicroVm])?;
    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn privileged_spec_requires_a_worker_with_privileged_guest_support() -> TestResult {
    let registry = thirty_second_registry();
    let mut profile = default_capability_profile();
    profile.provider = ProviderKind::K8sMicroVm;
    profile.isolation.default_tier = IsolationTier::MicroVm;
    profile.isolation.supported_tiers = vec![IsolationTier::MicroVm];
    profile.isolation.supports_privileged = CapabilitySupport::Unsupported;
    inject_worker(&registry, "nonprivileged", ProviderKind::K8sMicroVm, profile).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;
    let mut request = edit_request_spec(make_request(vec![ProviderKind::K8sMicroVm])?, |parts| {
        parts.privileged = PrivilegedAccess::Granted;
    })?;
    request.isolation_tier = Some(IsolationTier::MicroVm);
    assert!(matches!(
        core.handle_lease_request(request).await,
        LeaseResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn required_capabilities_unsatisfied_returns_typed_error() -> TestResult {
    let mut k8s_profile = default_capability_profile();
    k8s_profile.provider = ProviderKind::K8sMicroVm;
    k8s_profile.snapshot.can_snapshot = CapabilitySupport::Unsupported;
    k8s_profile.snapshot.can_checkpoint_running = CapabilitySupport::Unsupported;
    k8s_profile.snapshot.can_restore = CapabilitySupport::Unsupported;
    k8s_profile.snapshot.supported_artifact_types = vec![];
    k8s_profile.isolation.default_tier = IsolationTier::MicroVm;
    k8s_profile.isolation.supported_tiers = vec![IsolationTier::MicroVm];
    k8s_profile.isolation.egress_enforcement = EgressEnforcementLevel::BestEffort;

    let registry = thirty_second_registry();
    inject_worker(&registry, "k8s-w1", ProviderKind::K8sMicroVm, k8s_profile).await?;

    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;
    let mut req = make_request(vec![ProviderKind::K8sMicroVm])?;
    req.required_capabilities = Some(CapabilityRequirements {
        needs_snapshot: CapabilityNeed::Required,
        ..Default::default()
    });
    req.idempotency_key = LeaseRequestKey::new("cap-req-test")?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(
            response,
            LeaseResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. })
        ),
        "expected NoWorkerSatisfiesCapabilities, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_drives_lease_for_a_worker_without_fuse_is_refused_before_any_drive_claim() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let drive_store = default_drive_store();
    let drive_id = make_drive_id(&tenant, "data")?;
    drive_store
        .create(&tenant, &drive_id, 10, "test-bucket".to_string())
        .await?;
    let registry = thirty_second_registry();
    let mut profile = default_capability_profile();
    profile.provider = ProviderKind::K8sMicroVm;
    profile.volume.supports_fuse_mounts = CapabilitySupport::Unsupported;
    inject_worker(&registry, "k8s-w1", ProviderKind::K8sMicroVm, profile).await?;
    let mut core = make_core_with_registry(mock_provider_response()?, registry.clone(), false)?;
    core.drive_store = drive_store.clone();

    let mut req = make_request(vec![ProviderKind::K8sMicroVm])?;
    req.tenant = tenant.clone();
    req.idempotency_key = LeaseRequestKey::new("drive-fuse-gate")?;
    let mount = DriveMount {
        drive_id: drive_id.clone(),
        mount_path: MountPath::new("/mnt/data")?,
        mode: DriveMountMode::default(),
    };
    let req = edit_request_spec(req, |parts| {
        parts.drives = vec![mount];
    })?;

    let requirements = unsatisfied(core.handle_lease_request(req).await)?;
    assert!(requirements.needs_fuse_mounts.is_required());
    let record = drive_store.get(&drive_id).await?.ok_or("drive")?;
    assert_eq!(record.attached_lease, None, "no drive claim may leak");
    Ok(())
}

#[test]
fn the_always_on_gate_keeps_fuse_and_privileged() -> TestResult {
    let gate = egress_requirements_only(&CapabilityRequirements {
        needs_fuse_mounts: CapabilityNeed::Required,
        needs_privileged: CapabilityNeed::Required,
        ..Default::default()
    })
    .ok_or("gated")?;
    assert!(gate.needs_fuse_mounts.is_required() && gate.needs_privileged.is_required());
    Ok(())
}

#[tokio::test]
async fn user_defined_policy_aimed_only_at_subprocess_is_rejected() -> TestResult {
    let registry = thirty_second_registry();
    inject_profile_worker(&registry, ProviderKind::Subprocess, false).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let response = core
        .handle_lease_request(user_defined_request(vec![ProviderKind::Subprocess], "egress-reject")?)
        .await;

    let requirements = unsatisfied(response)?;
    assert!(requirements.needs_domain_allowlist.is_required());
    Ok(())
}

#[tokio::test]
async fn deny_all_is_rejected_when_the_only_worker_does_not_enforce_egress() -> TestResult {
    for enforce in [true, false] {
        let registry = thirty_second_registry();
        inject_egress_level_worker(&registry, ProviderKind::Subprocess, EgressEnforcementLevel::None).await?;
        let core = make_core_with_registry(mock_provider_response()?, registry, enforce)?;

        let response = core
            .handle_lease_request(deny_all_request(ProviderKind::Subprocess, "deny-none")?)
            .await;
        let requirements = unsatisfied(response)?;
        assert_eq!(
            requirements.min_egress_enforcement,
            Some(EgressEnforcementLevel::BestEffort),
            "enforce={enforce}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn deny_all_is_admitted_when_a_worker_enforces_egress() -> TestResult {
    for enforce in [true, false] {
        let registry = thirty_second_registry();
        inject_egress_level_worker(&registry, ProviderKind::Subprocess, EgressEnforcementLevel::Mandatory).await?;
        let core = make_core_with_registry(mock_provider_response()?, registry, enforce)?;

        let response = core
            .handle_lease_request(deny_all_request(ProviderKind::Subprocess, "deny-ok")?)
            .await;
        assert!(
            matches!(response, LeaseResponse::Granted(_)),
            "enforce={enforce}: {response:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn allow_all_ignores_the_egress_level_when_the_registry_is_not_enforced() -> TestResult {
    let registry = thirty_second_registry();
    inject_egress_level_worker(&registry, ProviderKind::Subprocess, EgressEnforcementLevel::None).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, false)?;
    let req = edit_request_spec(deny_all_request(ProviderKind::Subprocess, "allow-all")?, |parts| {
        parts.network_policy = Some(NetworkPolicy::AllowAll);
    })?;

    let response = core.handle_lease_request(req).await;
    assert!(matches!(response, LeaseResponse::Granted(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn fs_policy_lease_is_rejected_when_no_worker_enforces_it() -> TestResult {
    let registry = thirty_second_registry();
    inject_profile_worker(&registry, ProviderKind::Subprocess, true).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let policy = FsPolicy::new(
        vec![FsPathEntry::Literal {
            path: LiteralPath::new("/home")?,
            directory: Default::default(),
        }],
        vec![],
        vec![],
        vec![],
        vec![],
        vec![],
    )?;
    let mut req = edit_request_spec(make_request(vec![ProviderKind::Subprocess])?, |parts| {
        parts.fs_policy = Some(policy);
    })?;
    req.idempotency_key = LeaseRequestKey::new("fs-policy-reject")?;

    let requirements = unsatisfied(core.handle_lease_request(req).await)?;
    assert!(requirements.needs_fs_policy.is_required());
    Ok(())
}

#[tokio::test]
async fn vcpu_request_inside_the_advertised_range_is_admitted() -> TestResult {
    for enforce in [true, false] {
        let response = sized_lease(sized_resources()?, enforce, |s| s.vcpu_millis = Some(2000)).await?;
        assert!(
            matches!(response, LeaseResponse::Granted(_)),
            "enforce={enforce}: {response:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn vcpu_request_outside_every_range_is_refused_with_a_typed_error() -> TestResult {
    for enforce in [true, false] {
        for millis in [100, 8000] {
            let response = sized_lease(sized_resources()?, enforce, |s| s.vcpu_millis = Some(millis)).await?;
            let requirements = unsatisfied(response)?;
            assert_eq!(requirements.vcpu_millis, Some(millis), "enforce={enforce}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn disk_request_is_refused_when_no_worker_supports_disk_quotas() -> TestResult {
    for enforce in [true, false] {
        let response = sized_lease(sized_resources()?, enforce, |s| s.disk_mib = Some(1024)).await?;
        let requirements = unsatisfied(response)?;
        assert_eq!(requirements.disk_mib, Some(1024), "enforce={enforce}");
    }
    Ok(())
}

#[tokio::test]
async fn zero_sized_resources_are_invalid_specs() -> TestResult {
    let response = sized_lease(sized_resources()?, true, |s| s.vcpu_millis = Some(0)).await?;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    let response = sized_lease(sized_resources()?, true, |s| s.disk_mib = Some(0)).await?;
    assert!(matches!(
        response,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn secret_binding_lease_is_rejected_when_worker_lacks_binding_support() -> TestResult {
    let registry = thirty_second_registry();
    let mut profile = docker_profile();
    profile.isolation.supports_secret_bindings = CapabilitySupport::Unsupported;
    inject_worker(&registry, "docker-w1", ProviderKind::LocalDocker, profile).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let req = secret_binding_request("bindings-reject", SecretMode::Mask)?;
    let requirements = unsatisfied(core.handle_lease_request(req).await)?;
    assert!(requirements.needs_secret_bindings.is_required());
    assert!(requirements.needs_tls_intercept.is_required());
    assert!(requirements.needs_secret_substitution.is_required());
    Ok(())
}

#[tokio::test]
async fn secret_binding_lease_is_granted_when_worker_supports_bindings() -> TestResult {
    let registry = thirty_second_registry();
    inject_worker(&registry, "docker-w1", ProviderKind::LocalDocker, docker_profile()).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let req = secret_binding_request("bindings-grant", SecretMode::Mask)?;
    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Granted(_)),
        "expected grant, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn secret_binding_inject_host_denied_by_policy_is_invalid_spec() -> TestResult {
    let registry = thirty_second_registry();
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let req = secret_binding_request("bindings-denied", SecretMode::Mask)?;
    let original = req
        .spec
        .secret_bindings()
        .bindings()
        .first()
        .ok_or("a secret binding")?
        .clone();
    let blocked = SecretBinding::new(SecretBindingParts {
        name: original.name().clone(),
        target: original.target().clone(),
        mode: original.mode(),
        extract: original.extract().cloned(),
        mask_duplicates: original.mask_duplicates(),
        on_extract_no_match: original.on_extract_no_match(),
        strip_response_headers: original.strip_response_headers().clone(),
        inject_hosts: InjectHosts::new(vec!["blocked.example.com".parse()?])?,
        inject_scope: original.inject_scope().clone(),
        source: original.source().clone(),
    })?;
    let bindings = SecretBindings::new(vec![blocked])?;
    // The spec refuses a binding whose inject host the network policy denies.
    assert!(edit_request_spec(req, |parts| parts.secret_bindings = bindings).is_err());
    let _ = &core;
    Ok(())
}

#[test]
fn mask_binding_infers_tls_and_substitution_but_deny_does_not() -> TestResult {
    let mask =
        infer_capability_requirements(&secret_binding_request("k", SecretMode::Mask)?).ok_or("mask requirements")?;
    assert!(
        mask.needs_secret_bindings.is_required()
            && mask.needs_tls_intercept.is_required()
            && mask.needs_secret_substitution.is_required()
    );

    let deny =
        infer_capability_requirements(&secret_binding_request("k", SecretMode::Deny)?).ok_or("deny requirements")?;
    assert!(deny.needs_secret_bindings.is_required());
    assert!(!deny.needs_tls_intercept.is_required() && !deny.needs_secret_substitution.is_required());
    Ok(())
}

#[tokio::test]
async fn masked_file_lease_is_rejected_when_worker_cannot_mask_files() -> TestResult {
    let registry = thirty_second_registry();
    let mut profile = docker_profile();
    profile.isolation.supports_masked_files = CapabilitySupport::Unsupported;
    inject_worker(&registry, "docker-w1", ProviderKind::LocalDocker, profile).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let response = core
        .handle_lease_request(masked_file_request("masked-file-reject")?)
        .await;
    let requirements = unsatisfied(response)?;
    assert!(requirements.needs_masked_files.is_required());

    let response = core
        .handle_lease_request(secret_binding_request("masked-env-grant", SecretMode::Mask)?)
        .await;
    assert!(
        matches!(response, LeaseResponse::Granted(_)),
        "expected grant, got {response:?}"
    );
    Ok(())
}

#[test]
fn empty_fs_policy_infers_no_requirement() -> TestResult {
    let req = edit_request_spec(make_request(vec![ProviderKind::Subprocess])?, |parts| {
        parts.fs_policy = Some(FsPolicy::default());
    })?;
    assert_eq!(infer_capability_requirements(&req), None);
    Ok(())
}

#[test]
fn exposed_ports_infer_the_port_preview_requirement() -> TestResult {
    let port = Port::new(3000)?;
    let req = edit_request_spec(make_request(vec![ProviderKind::Subprocess])?, |parts| {
        parts.exposed_ports = vec![port];
    })?;
    let inferred = infer_capability_requirements(&req).ok_or("requirement inferred")?;
    assert!(inferred.needs_port_preview.is_required());
    let cleared = edit_request_spec(req, |parts| parts.exposed_ports.clear())?;
    assert_eq!(infer_capability_requirements(&cleared), None);
    Ok(())
}

#[test]
fn the_declared_port_count_is_inferred_and_gated_only_beyond_one() -> TestResult {
    let one = vec![Port::new(3000)?];
    let req = edit_request_spec(make_request(vec![ProviderKind::Substrate])?, |parts| {
        parts.exposed_ports = one;
    })?;
    let inferred = infer_capability_requirements(&req).ok_or("requirement inferred")?;
    assert_eq!(inferred.exposed_port_count, Some(1));
    let gate = egress_requirements_only(&inferred);
    assert_eq!(gate.and_then(|g| g.exposed_port_count), None);

    let two = vec![Port::new(3000)?, Port::new(3001)?];
    let req = edit_request_spec(req, |parts| parts.exposed_ports = two)?;
    let inferred = infer_capability_requirements(&req).ok_or("requirement inferred")?;
    assert_eq!(inferred.exposed_port_count, Some(2));
    let gate = egress_requirements_only(&inferred).ok_or("several ports are gated")?;
    assert_eq!(gate.exposed_port_count, Some(2));
    Ok(())
}

#[test]
fn several_ports_skip_a_worker_whose_ingress_maximum_is_lower() -> TestResult {
    let two = vec![Port::new(3000)?, Port::new(3001)?];
    let req = edit_request_spec(make_request(vec![ProviderKind::Substrate])?, |parts| {
        parts.exposed_ports = two;
    })?;
    let inferred = infer_capability_requirements(&req).ok_or("requirement inferred")?;
    let gate = egress_requirements_only(&inferred).ok_or("gated")?;
    let mut profile = default_capability_profile();
    profile.isolation.supports_port_preview = CapabilitySupport::Supported;
    profile.ingress.max_exposed_ports = Some(1);
    assert!(!profile.satisfies(&gate));
    profile.ingress.max_exposed_ports = Some(2);
    assert!(profile.satisfies(&gate));
    profile.ingress.max_exposed_ports = None;
    assert!(profile.satisfies(&gate));
    Ok(())
}

#[test]
fn a_no_ttl_lease_needs_an_uncapped_worker_even_without_the_registry() -> TestResult {
    let req = edit_request_spec(make_request(vec![ProviderKind::Subprocess])?, |parts| {
        parts.no_ttl = LeaseTtl::Unbounded;
        parts.idle_timeout_seconds = NonZeroU32::new(60);
    })?;
    let inferred = infer_capability_requirements(&req).ok_or("requirement inferred")?;
    assert_eq!(
        inferred.lease_lifetime_seconds,
        Some(CapabilityRequirements::NO_TTL_LIFETIME)
    );
    let gate = egress_requirements_only(&inferred).ok_or("no_ttl is gated")?;
    assert_eq!(
        gate.lease_lifetime_seconds,
        Some(CapabilityRequirements::NO_TTL_LIFETIME)
    );

    let bounded = edit_request_spec(req, |parts| parts.no_ttl = LeaseTtl::Bounded)?;
    assert_eq!(infer_capability_requirements(&bounded), None);
    let port = Port::new(3000)?;
    let with_port = edit_request_spec(bounded, |parts| parts.exposed_ports = vec![port])?;
    let inferred = infer_capability_requirements(&with_port).ok_or("requirement inferred")?;
    assert_eq!(inferred.lease_lifetime_seconds, Some(with_port.ttl_seconds));
    assert_eq!(
        egress_requirements_only(&inferred).and_then(|g| g.lease_lifetime_seconds),
        None
    );
    Ok(())
}

#[test]
fn a_persistent_runtime_env_infers_the_persistent_runtime_requirement() -> TestResult {
    let req = edit_request_spec(make_request(vec![ProviderKind::V8Wasm])?, |parts| {
        parts
            .env
            .insert(crate::constants::PERSISTENT_RUNTIME_ENV.to_string(), "true".to_string());
    })?;
    let inferred = infer_capability_requirements(&req).ok_or("requirement inferred")?;
    assert!(inferred.needs_persistent_runtime.is_required());
    let cleared = edit_request_spec(req, |parts| parts.env.clear())?;
    assert_eq!(infer_capability_requirements(&cleared), None);
    Ok(())
}

#[test]
fn hardening_overrides_infer_the_matching_requirement() -> TestResult {
    let req = shm_override_request("infer")?;
    let inferred = infer_capability_requirements(&req).ok_or("requirement inferred")?;
    assert!(inferred.needs_hardening_overrides.is_required());
    let plain = edit_request_spec(req, |parts| {
        parts.platform.container = Default::default();
    })?;
    assert_eq!(infer_capability_requirements(&plain), None);
    Ok(())
}

#[tokio::test]
async fn hardening_overrides_are_denied_by_default() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    match core
        .handle_lease_request(shm_override_request("override-default-deny")?)
        .await
    {
        LeaseResponse::Denied(LeaseError::InvalidSpec { reason }) => {
            assert!(reason.contains("hardening overrides"), "{reason}");
        }
        other => return Err(format!("expected InvalidSpec, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn hardening_overrides_pass_for_an_allowed_tenant_only() -> TestResult {
    let core = make_core(mock_provider_response()?)?.with_hardening_override_policy(
        crate::hardening_override_policy::HardeningOverridePolicy::allowing([TenantId::new("test-tenant")?]),
    );
    let granted = core
        .handle_lease_request(shm_override_request("override-allowed")?)
        .await;
    assert!(
        matches!(granted, LeaseResponse::Granted(_)),
        "expected grant, got {granted:?}"
    );

    let mut other = shm_override_request("override-other-tenant")?;
    other.tenant = TenantId::new("someone-else")?;
    assert!(matches!(
        core.handle_lease_request(other).await,
        LeaseResponse::Denied(LeaseError::InvalidSpec { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn an_allowed_tenant_cannot_add_a_forbidden_capability() -> TestResult {
    // A forbidden capability is unrepresentable: it cannot be constructed.
    assert!(ExtraCapability::new("SYS_ADMIN").is_err());
    Ok(())
}

#[tokio::test]
async fn user_defined_policy_selects_docker_over_subprocess() -> TestResult {
    let registry = thirty_second_registry();
    inject_profile_worker(&registry, ProviderKind::Subprocess, false).await?;
    inject_profile_worker(&registry, ProviderKind::LocalDocker, true).await?;
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let response = core
        .handle_lease_request(user_defined_request(
            vec![ProviderKind::Subprocess, ProviderKind::LocalDocker],
            "egress-docker",
        )?)
        .await;

    assert!(
        matches!(response, LeaseResponse::Granted(ref g) if g.provider == ProviderKind::LocalDocker),
        "expected grant on Docker, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn lease_grant_carries_the_provider_capability_slice() -> TestResult {
    let registry = thirty_second_registry();
    inject_profile_worker(&registry, ProviderKind::LocalDocker, true).await?;
    let expected: Vec<String> = registry
        .profile_for_provider(ProviderKind::LocalDocker)
        .await
        .ok_or("a docker profile")?
        .sdk_features()
        .into_iter()
        .map(|feature| feature.as_str().to_owned())
        .collect();
    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    let response = core
        .handle_lease_request(make_request(vec![ProviderKind::LocalDocker])?)
        .await;

    let LeaseResponse::Granted(granted) = response else {
        return Err(format!("expected grant, got {response:?}").into());
    };
    assert_eq!(granted.capabilities, expected);
    Ok(())
}

#[tokio::test]
async fn provider_fallback_picks_subprocess_when_k8s_has_no_workers() -> TestResult {
    let registry = thirty_second_registry();

    // Only Subprocess has a worker available
    inject_worker(
        &registry,
        "subprocess-w1",
        ProviderKind::Subprocess,
        default_capability_profile(),
    )
    .await?;

    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;

    // Prefer K8sMicroVm first, then Subprocess
    let mut req = make_request(vec![ProviderKind::K8sMicroVm, ProviderKind::Subprocess])?;
    req.idempotency_key = LeaseRequestKey::new("fallback-test")?;
    let response = core.handle_lease_request(req).await;

    // Should fall back to Subprocess and grant
    assert!(
        matches!(response, LeaseResponse::Granted(ref g) if g.provider == ProviderKind::Subprocess),
        "expected grant on Subprocess, got {response:?}"
    );
    Ok(())
}
