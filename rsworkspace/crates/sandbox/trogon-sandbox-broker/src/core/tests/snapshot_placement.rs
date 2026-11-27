use super::*;
use crate::quota::{QuotaRecord, SnapshotQuota};
use std::time::SystemTime;
use trogon_sandbox_contract::{
    ArtifactType, EgressEnforcementLevel, IsolationCapability, IsolationTier, LifecycleCapability, OciImageRef,
    PerformanceProfile, ProviderCapabilityProfile, ProviderDispatchPayload, ServiceAccountId, SnapshotCapability,
    SnapshotTagRecord, VolumeCapability, WorkerCapacity, WorkerHeartbeat,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn native_snapshot_reaches_only_a_caller_that_accepts_it_and_is_recorded() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let lease_id = LeaseId::new("lease-source");
    let (core, bus) = native_snapshot_core(ProviderKind::Mock, true).await?;
    core.active_leases
        .put(&unretained_source_record(
            &tenant,
            &lease_id,
            trogon_sandbox_contract::RetentionToggles::default(),
        )?)
        .await?;

    let legacy = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            tenant: tenant.clone(),
            lease_id: lease_id.clone(),
            accept_native: false.into(),
        })
        .await;
    assert!(matches!(legacy, SnapshotLeaseResponse::Unsupported), "{legacy:?}");

    let response = core
        .handle_snapshot_lease(SnapshotLeaseRequest {
            tenant: tenant.clone(),
            lease_id,
            accept_native: true.into(),
        })
        .await;
    let SnapshotLeaseResponse::CreatedNative { receipt } = response else {
        return Err(format!("expected a native receipt, got {response:?}").into());
    };
    assert!(core.native_snapshots.owns(&tenant, &receipt.reference).await?);
    assert!(
        !core
            .native_snapshots
            .owns(&TenantId::new("other-tenant")?, &receipt.reference)
            .await?
    );
    let names = locked(&bus.requested_names)?;
    assert_eq!(names.len(), 2);
    assert!(
        names
            .iter()
            .all(|n| n.as_deref().is_some_and(|n| n.starts_with("trogon-")))
    );
    Ok(())
}

#[tokio::test]
async fn native_snapshot_restore_is_limited_to_the_owning_tenant_and_provider() -> TestResult {
    let owner = TenantId::new("test-tenant")?;
    let reference = trogon_sandbox_contract::NativeSnapshotRef {
        provider: ProviderKind::Mock,
        id: trogon_sandbox_contract::NativeSnapshotId::new("trogon-abc")?,
        expires_at: None,
    };
    let (core, _bus) = native_snapshot_core(ProviderKind::Mock, true).await?;
    core.native_snapshots.record(&owner, &reference).await?;

    let granted = core
        .handle_lease_request(native_restore_request(
            &owner,
            vec![ProviderKind::Hyperlight, ProviderKind::Mock],
            reference.clone(),
        )?)
        .await;
    assert!(
        matches!(granted, LeaseResponse::Granted(ref g) if g.provider == ProviderKind::Mock),
        "{granted:?}"
    );

    let foreign = core
        .handle_lease_request(native_restore_request(
            &TenantId::new("other-tenant")?,
            vec![ProviderKind::Mock],
            reference.clone(),
        )?)
        .await;
    assert!(
        matches!(foreign, LeaseResponse::Denied(LeaseError::InvalidSpec { ref reason }) if reason.contains("not found")),
        "{foreign:?}"
    );

    let wrong_provider = core
        .handle_lease_request(native_restore_request(
            &owner,
            vec![ProviderKind::Hyperlight],
            reference.clone(),
        )?)
        .await;
    assert!(
        matches!(wrong_provider, LeaseResponse::Denied(LeaseError::InvalidSpec { .. })),
        "{wrong_provider:?}"
    );

    let expired = trogon_sandbox_contract::NativeSnapshotRef {
        expires_at: Some(1),
        ..reference
    };
    core.native_snapshots.record(&owner, &expired).await?;
    let response = core
        .handle_lease_request(native_restore_request(&owner, vec![ProviderKind::Mock], expired)?)
        .await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::InvalidSpec { ref reason }) if reason.contains("expired")),
        "{response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn native_snapshot_restore_needs_a_worker_that_advertises_native_snapshots() -> TestResult {
    let owner = TenantId::new("test-tenant")?;
    let reference = trogon_sandbox_contract::NativeSnapshotRef {
        provider: ProviderKind::Mock,
        id: trogon_sandbox_contract::NativeSnapshotId::new("trogon-abc")?,
        expires_at: None,
    };
    let (core, _bus) = native_snapshot_core(ProviderKind::Mock, false).await?;
    core.native_snapshots.record(&owner, &reference).await?;

    let response = core
        .handle_lease_request(native_restore_request(&owner, vec![ProviderKind::Mock], reference)?)
        .await;
    assert!(matches!(response, LeaseResponse::Denied(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn fork_of_a_native_provider_restores_the_native_snapshot() -> TestResult {
    let tenant = TenantId::new("test-tenant")?;
    let source = LeaseId::new("lease-source");
    let (core, _bus) = native_snapshot_core(ProviderKind::Mock, true).await?;
    core.active_leases
        .put(&unretained_source_record(
            &tenant,
            &source,
            trogon_sandbox_contract::RetentionToggles::default(),
        )?)
        .await?;

    let response = core.handle_fork_request(make_fork_request(&tenant, &source)).await;
    assert!(matches!(response, LeaseResponse::Granted(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn snapshot_locality_prefers_warm_provider() -> TestResult {
    let ttl = Duration::from_secs(30);
    let registry = InMemoryWorkerRegistry::new(ttl);
    let digest = test_digest()?;

    let hl_profile = restore_capable_profile(ProviderKind::Hyperlight);
    registry
        .inject(WorkerHeartbeat {
            supports_lease_epoch: CapabilitySupport::Unsupported,
            worker_id: WorkerId::new("hl-cold")?,
            provider: ProviderKind::Hyperlight,
            capacity: WorkerCapacity {
                max_concurrent_leases: 4,
                current_leases: 0,
                capabilities: WorkerCapabilities::from(&hl_profile),
            },
            emitted_at: SystemTime::now(),
            cached_digests: vec![],
            profile: hl_profile.clone(),
            warm_pool_population: std::collections::HashMap::new(),
            dispatch_protocol: Default::default(),
            native_volume_bindings: Default::default(),
        })
        .await;

    let mock_profile = restore_capable_profile(ProviderKind::Mock);
    registry
        .inject(WorkerHeartbeat {
            supports_lease_epoch: CapabilitySupport::Unsupported,
            worker_id: WorkerId::new("mock-warm")?,
            provider: ProviderKind::Mock,
            capacity: WorkerCapacity {
                max_concurrent_leases: 4,
                current_leases: 0,
                capabilities: WorkerCapabilities::from(&mock_profile),
            },
            emitted_at: SystemTime::now(),
            cached_digests: vec![digest.clone()],
            profile: mock_profile,
            warm_pool_population: std::collections::HashMap::new(),
            dispatch_protocol: Default::default(),
            native_volume_bindings: Default::default(),
        })
        .await;

    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;
    let mut req = make_snapshot_request(vec![ProviderKind::Hyperlight, ProviderKind::Mock])?;
    req.idempotency_key = LeaseRequestKey::new("locality-test")?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Granted(ref g) if g.provider == ProviderKind::Mock),
        "expected warm Mock worker, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_without_restore_worker_returns_typed_error() -> TestResult {
    let no_restore = ProviderCapabilityProfile {
        provider: ProviderKind::K8sMicroVm,
        snapshot: SnapshotCapability {
            can_snapshot: false.into(),
            can_checkpoint_running: false.into(),
            can_restore: false.into(),
            supported_artifact_types: vec![],
            layered_deltas: false.into(),
            supports_native_snapshots: false.into(),
            supports_in_place_rollback: false.into(),
            max_snapshot_size_bytes: None,
        },
        volume: VolumeCapability {
            supported_modes: vec![],
            supports_s3_backend: false.into(),
            supports_fuse_mounts: false.into(),
        },
        isolation: IsolationCapability {
            default_tier: IsolationTier::MicroVm,
            supported_tiers: vec![IsolationTier::MicroVm],
            egress_enforcement: EgressEnforcementLevel::BestEffort,
            supports_privileged: false.into(),
            supports_sandbox_users: false.into(),
            supports_interactive_stdin: false.into(),
            supports_named_processes: false.into(),
            supports_process_readiness: false.into(),
            supports_graceful_stop: false.into(),
            supports_command_restart: false.into(),
            supports_command_cancellation: false.into(),
            supports_command_launch_cancellation: false.into(),
            supports_port_preview: false.into(),
            supports_expose_after_create: false.into(),
            supports_evidence_streaming: false.into(),
            supports_output_streaming: false.into(),
            supports_domain_allowlist: false.into(),
            supports_tls_intercept: false.into(),
            supports_secret_substitution: false.into(),
            supports_secret_bindings: false.into(),
            supports_masked_files: false.into(),
            supports_fs_policy: false.into(),
            supports_file_ops: false.into(),
            supports_file_search: false.into(),
            supports_file_archive: false.into(),
            supports_output_redaction: false.into(),
            supports_terminal: false.into(),
            resource_limits: trogon_sandbox_contract::ResourceLimitMechanism::None,
            egress: trogon_sandbox_contract::EgressCapability::NONE,
            supports_hardening_overrides: false.into(),
            supports_change_tracking: false.into(),
            supports_file_watch: false.into(),
            supports_persistent_runtime: false.into(),
            supports_dir_transfer: false.into(),
            supports_browser: false.into(),
            supports_network_policy_update: false.into(),
            supports_sandbox_groups: false.into(),
        },
        lifecycle: LifecycleCapability {
            supports_warm_pool: false.into(),
            max_warm_pool_size: None,
            supports_named_tags: false.into(),
            supports_prewarm_per_tenant: false.into(),
            supports_pause_resume: false.into(),
            supports_native_auto_stop: false.into(),
            max_lease_lifetime_seconds: None,
            dormancy: trogon_sandbox_contract::DormancyCapability::NONE,
        },
        performance: PerformanceProfile {
            cold_start_p50_ms: 5000,
            cold_restore_p50_ms: 10000,
            max_concurrent_sandboxes: Some(4),
        },
        resources: Default::default(),
        placement: Default::default(),
        ingress: Default::default(),
    };

    let ttl = Duration::from_secs(30);
    let registry = InMemoryWorkerRegistry::new(ttl);
    registry
        .inject(WorkerHeartbeat {
            supports_lease_epoch: CapabilitySupport::Unsupported,
            worker_id: WorkerId::new("k8s-w1")?,
            provider: ProviderKind::K8sMicroVm,
            capacity: WorkerCapacity {
                max_concurrent_leases: 4,
                current_leases: 0,
                capabilities: WorkerCapabilities::from(&no_restore),
            },
            emitted_at: SystemTime::now(),
            cached_digests: vec![],
            profile: no_restore,
            warm_pool_population: std::collections::HashMap::new(),
            dispatch_protocol: Default::default(),
            native_volume_bindings: Default::default(),
        })
        .await;

    let core = make_core_with_registry(mock_provider_response()?, registry, true)?;
    let mut req = make_snapshot_request(vec![ProviderKind::K8sMicroVm])?;
    req.idempotency_key = LeaseRequestKey::new("no-restore-test")?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(
            response,
            LeaseResponse::Denied(LeaseError::NoWorkerCanRestoreSnapshot { .. })
        ),
        "expected NoWorkerCanRestoreSnapshot, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn active_lease_cap_denies_new_leases() -> TestResult {
    let quota = Arc::new(InMemoryQuotaStore::new());
    let tenant = TenantId::new("cap-tenant")?;
    quota
        .seed_record(
            &tenant,
            QuotaRecord {
                active_leases: 1,
                ..QuotaRecord::with_snapshot_quota(3600, 7200, SnapshotQuota::new(100, 1_000_000_000), 1)
            },
        )
        .await;

    let core = BrokerCore::new(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        quota,
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(std::time::Duration::from_secs(30)),
        default_pins(),
        default_tag_store(),
        default_resolved_digests(),
        default_drive_store(),
        default_named_leases(),
        default_config(),
        BrokerMetrics::new()?,
    );

    let mut req = make_request(vec![ProviderKind::Mock])?;
    req.tenant = tenant;
    req.idempotency_key = LeaseRequestKey::new("cap-test")?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::TooManyActiveLeases { .. })),
        "expected TooManyActiveLeases, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn snapshot_tag_resolves_to_digest_at_dispatch() -> TestResult {
    let audit: Arc<dyn crate::snapshot_tag::SnapshotTagAuditor> =
        Arc::new(crate::snapshot_tag::InMemorySnapshotTagAuditor::new());
    let tag_store = Arc::new(crate::snapshot_tag::InMemorySnapshotTagStore::new(Arc::clone(&audit)));
    let tag = snapshot_tag_of("test-tenant", "prod")?;
    tag_store
        .set(
            &tag,
            SnapshotTagRecord {
                digest: test_digest()?,
                artifact_type: ArtifactType::HyperlightSnapshotV1,
                set_by: ServiceAccountId::new("test"),
                set_at: SystemTime::now(),
                retention: None,
            },
            None,
        )
        .await?;

    let core = BrokerCore::new(
        Arc::new(MockBus {
            provider_response: mock_provider_response()?,
        }),
        Arc::new(InMemoryQuotaStore::new()),
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(std::time::Duration::from_secs(30)),
        default_pins(),
        tag_store,
        default_resolved_digests(),
        default_drive_store(),
        default_named_leases(),
        default_config(),
        BrokerMetrics::new()?,
    );

    let req = make_request(vec![ProviderKind::Mock])?;
    let mut req = edit_request_spec(req, |parts| {
        parts.image = SandboxImage::SnapshotTag {
            tag: tag.clone(),
            artifact_type: ArtifactType::HyperlightSnapshotV1,
            create_from: None,
        };
    })?;
    req.idempotency_key = LeaseRequestKey::new("tag-resolve-test")?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Granted(_)),
        "expected Granted for resolved tag, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn missing_snapshot_tag_returns_typed_error() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let req = make_request(vec![ProviderKind::Mock])?;
    let mut req = {
        let mut req = req;
        let mut parts = req.spec.into_parts();
        parts.image = SandboxImage::SnapshotTag {
            tag: snapshot_tag_of("test-tenant", "missing")?,
            artifact_type: ArtifactType::HyperlightSnapshotV1,
            create_from: None,
        };
        req.spec = SandboxSpec::new(parts)?;
        req
    };
    req.idempotency_key = LeaseRequestKey::new("tag-missing-test")?;

    let response = core.handle_lease_request(req).await;
    assert!(
        matches!(response, LeaseResponse::Denied(LeaseError::SnapshotTagNotFound { .. })),
        "expected SnapshotTagNotFound, got {response:?}"
    );
    Ok(())
}

#[tokio::test]
async fn missing_snapshot_tag_with_create_from_falls_back_and_grants() -> TestResult {
    let bus = Arc::new(CapturingBus::new(mock_provider_response()?));
    let resolved_digests = default_resolved_digests();

    let core = BrokerCore::new(
        bus.clone(),
        Arc::new(InMemoryQuotaStore::new()),
        Arc::new(InMemoryIdempotencyStore::new()),
        Arc::new(InMemoryActiveLeaseStore::new()),
        Arc::new(InMemoryImageAllowList::new()),
        Arc::new(crate::host_mount_policy::InMemoryHostMountAllowList::new()),
        Arc::new(InMemoryHostImportPolicy::new()),
        Arc::new(InMemoryTenantLimits::new()),
        InMemoryWorkerRegistry::with_always_available(std::time::Duration::from_secs(30)),
        default_pins(),
        default_tag_store(),
        resolved_digests.clone(),
        default_drive_store(),
        default_named_leases(),
        default_config(),
        BrokerMetrics::new()?,
    );

    let fallback_image = SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
        pull_secret: None,
    };
    let req = make_request(vec![ProviderKind::Mock])?;
    let mut req = {
        let mut req = req;
        let mut parts = req.spec.into_parts();
        parts.image = SandboxImage::SnapshotTag {
            tag: snapshot_tag_of("test-tenant", "missing")?,
            artifact_type: ArtifactType::HyperlightSnapshotV1,
            create_from: Some(Box::new(fallback_image.clone())),
        };
        req.spec = SandboxSpec::new(parts)?;
        req
    };
    req.idempotency_key = LeaseRequestKey::new("tag-missing-create-from-test")?;

    let response = core.handle_lease_request(req).await;
    let granted = match response {
        LeaseResponse::Granted(g) => g,
        other => return Err(format!("expected Granted for create_from fallback, got {other:?}").into()),
    };

    let dispatched_payload = locked(&bus.last_payload)?.clone().ok_or("dispatched payload")?;
    let envelope: ProviderDispatchPayload = serde_json::from_slice(&dispatched_payload)?;
    assert_eq!(envelope.request.spec.image(), &fallback_image);

    let recorded = resolved_digests.get(&granted.lease_id).await?;
    assert!(
        recorded.is_none(),
        "create_from fallback must not record a resolved digest"
    );
    Ok(())
}
