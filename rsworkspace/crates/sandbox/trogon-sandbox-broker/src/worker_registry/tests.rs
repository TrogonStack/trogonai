use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;

use trogon_sandbox_contract::{
    CapabilityRequirements, CapabilitySupport, Digest, ProviderCapabilityProfile, ProviderKind, WorkerCapabilities,
    WorkerHeartbeat, WorkerId,
};

use crate::default_capability_profile::default_capability_profile;
use trogon_sandbox_contract::{
    ArtifactType, CapabilityNeed, DormancyCapability, DriveId, DriveMount, DriveMountMode, DriveName, EgressCapability,
    EgressEnforcementLevel, IsolationCapability, IsolationTier, LifecycleCapability, MountPath, NativeVolumeBinding,
    PerformanceProfile, ProviderVolumeId, ProviderVolumeRef, ResourceLimitMechanism, SnapshotCapability, TenantId,
    VolumeCapability, WorkerCapacity, WorkerDispatchProtocol,
};
use trogon_sandbox_protocol::subjects;

use super::*;
use crate::worker_fence::{WorkerFence, WorkerPlacementState};

fn hb(
    worker_id: &str,
    provider: ProviderKind,
    current_leases: u32,
    max_concurrent_leases: u32,
    age: Duration,
) -> Result<WorkerHeartbeat, Box<dyn std::error::Error>> {
    let profile = default_capability_profile();
    Ok(WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: WorkerId::new(worker_id)?,
        provider,
        capacity: WorkerCapacity {
            max_concurrent_leases,
            current_leases,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now()
            .checked_sub(age)
            .ok_or("heartbeat age reaches before the clock epoch")?,
        cached_digests: vec![],
        profile,
        warm_pool_population: HashMap::new(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    })
}

fn hb_in(worker_id: &str, regions: &[&str]) -> Result<WorkerHeartbeat, Box<dyn std::error::Error>> {
    let mut beat = hb(worker_id, ProviderKind::K8sMicroVm, 0, 4, Duration::from_secs(1))?;
    beat.profile.placement.regions = regions.iter().map(|r| r.to_string()).collect();
    Ok(beat)
}

fn fence(worker: &str, state: WorkerPlacementState) -> Result<WorkerFence, Box<dyn std::error::Error>> {
    Ok(WorkerFence {
        provider: ProviderKind::K8sMicroVm,
        worker_id: WorkerId::new(worker)?,
        state,
        reason: None,
        updated_at: SystemTime::now(),
    })
}

#[tokio::test]
async fn a_fenced_worker_takes_no_new_placements() -> Result<(), Box<dyn std::error::Error>> {
    let reg = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    reg.inject(hb_in("w-a", &["us"])?).await;
    reg.inject(hb_in("w-b", &["us"])?).await;

    reg.fences().replace(&[fence("w-a", WorkerPlacementState::Cordoned)?]);
    assert_eq!(
        reg.available_for(ProviderKind::K8sMicroVm).await,
        vec![WorkerId::new("w-b")?]
    );

    reg.fences().replace(&[
        fence("w-a", WorkerPlacementState::Cordoned)?,
        fence("w-b", WorkerPlacementState::Draining)?,
    ]);
    assert!(!reg.any_available(ProviderKind::K8sMicroVm).await);
    assert!(!reg.any_can_restore(ProviderKind::K8sMicroVm, None).await);
    assert!(
        select_provider(&*reg, &[ProviderKind::K8sMicroVm], None, None)
            .await
            .is_err()
    );

    reg.fences().replace(&[]);
    assert_eq!(reg.available_for(ProviderKind::K8sMicroVm).await.len(), 2);
    Ok(())
}

fn regions(names: &[&str]) -> Vec<String> {
    names.iter().map(|r| r.to_string()).collect()
}

#[tokio::test]
async fn region_selection_prefers_the_first_region_with_a_worker() -> Result<(), Box<dyn std::error::Error>> {
    let reg = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    reg.inject(hb_in("w-us", &["us"])?).await;
    reg.inject(hb_in("w-eu", &["eu"])?).await;
    let (selection, region) = select_provider_in_regions(
        &*reg,
        &[ProviderKind::K8sMicroVm],
        None,
        None,
        WorkerSelectionConstraints::default(),
        &regions(&["eu", "us"]),
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(region.as_deref(), Some("eu"));
    assert_eq!(selection.provider, ProviderKind::K8sMicroVm);
    Ok(())
}

#[tokio::test]
async fn region_selection_falls_back_in_order() -> Result<(), Box<dyn std::error::Error>> {
    let reg = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    reg.inject(hb_in("w-us", &["us"])?).await;
    let (_, region) = select_provider_in_regions(
        &*reg,
        &[ProviderKind::K8sMicroVm],
        None,
        None,
        WorkerSelectionConstraints::default(),
        &regions(&["eu", "ap", "us"]),
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(region.as_deref(), Some("us"));
    Ok(())
}

#[tokio::test]
async fn region_selection_names_the_regions_when_none_match() -> Result<(), Box<dyn std::error::Error>> {
    let reg = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    reg.inject(hb_in("w-us", &["us"])?).await;
    let result = select_provider_in_regions(
        &*reg,
        &[ProviderKind::K8sMicroVm],
        None,
        None,
        WorkerSelectionConstraints::default(),
        &regions(&["eu", "ap"]),
    )
    .await;
    assert!(matches!(
        result,
        Err(ProviderSelectionError::NoWorkerInRegion(ref r)) if r == &regions(&["eu", "ap"])
    ));
    Ok(())
}

#[tokio::test]
async fn region_selection_reports_the_real_blocker_when_no_worker_exists() -> Result<(), Box<dyn std::error::Error>> {
    let reg = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let result = select_provider_in_regions(
        &*reg,
        &[ProviderKind::K8sMicroVm],
        None,
        None,
        WorkerSelectionConstraints::default(),
        &regions(&["eu"]),
    )
    .await;
    assert!(matches!(result, Err(ProviderSelectionError::NoProviderAvailable)));
    Ok(())
}

#[tokio::test]
async fn no_regions_means_no_placement_filter() -> Result<(), Box<dyn std::error::Error>> {
    let reg = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    reg.inject(hb_in("w-any", &[])?).await;
    let (_, region) = select_provider_in_regions(
        &*reg,
        &[ProviderKind::K8sMicroVm],
        None,
        None,
        WorkerSelectionConstraints::default(),
        &[],
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    assert!(region.is_none());
    Ok(())
}

#[tokio::test]
async fn fresh_worker_with_capacity_is_available() -> Result<(), Box<dyn std::error::Error>> {
    let ttl = Duration::from_secs(30);
    let reg = InMemoryWorkerRegistry::new(ttl);
    reg.inject(hb("w1", ProviderKind::K8sMicroVm, 0, 4, Duration::from_secs(5))?)
        .await;
    assert!(reg.any_available(ProviderKind::K8sMicroVm).await);
    assert!(
        reg.available_for(ProviderKind::K8sMicroVm)
            .await
            .contains(&WorkerId::new("w1")?)
    );
    Ok(())
}

#[tokio::test]
async fn stale_worker_is_not_available() -> Result<(), Box<dyn std::error::Error>> {
    let ttl = Duration::from_secs(30);
    let reg = InMemoryWorkerRegistry::new(ttl);
    reg.inject(hb("w-stale", ProviderKind::K8sMicroVm, 0, 4, Duration::from_secs(60))?)
        .await;
    assert!(!reg.any_available(ProviderKind::K8sMicroVm).await);
    Ok(())
}

#[tokio::test]
async fn capacity_zero_worker_is_not_available() -> Result<(), Box<dyn std::error::Error>> {
    let ttl = Duration::from_secs(30);
    let reg = InMemoryWorkerRegistry::new(ttl);
    // current_leases == max_concurrent_leases, so no capacity is left.
    reg.inject(hb("w-full", ProviderKind::K8sMicroVm, 4, 4, Duration::from_secs(5))?)
        .await;
    assert!(!reg.any_available(ProviderKind::K8sMicroVm).await);
    Ok(())
}

#[tokio::test]
async fn no_heartbeats_means_not_available() -> Result<(), Box<dyn std::error::Error>> {
    let ttl = Duration::from_secs(30);
    let reg = InMemoryWorkerRegistry::new(ttl);
    assert!(!reg.any_available(ProviderKind::K8sMicroVm).await);
    Ok(())
}

#[tokio::test]
async fn fresh_and_stale_only_returns_fresh() -> Result<(), Box<dyn std::error::Error>> {
    let ttl = Duration::from_secs(30);
    let reg = InMemoryWorkerRegistry::new(ttl);
    reg.inject(hb("fresh", ProviderKind::K8sMicroVm, 1, 4, Duration::from_secs(5))?)
        .await;
    reg.inject(hb("stale", ProviderKind::K8sMicroVm, 0, 4, Duration::from_secs(60))?)
        .await;
    let available = reg.available_for(ProviderKind::K8sMicroVm).await;
    assert_eq!(available.len(), 1);
    assert!(available.contains(&WorkerId::new("fresh")?));
    Ok(())
}

#[tokio::test]
async fn always_available_registry_returns_true_with_no_heartbeats() -> Result<(), Box<dyn std::error::Error>> {
    let reg = InMemoryWorkerRegistry::with_always_available(Duration::from_secs(30));
    assert!(reg.any_available(ProviderKind::K8sMicroVm).await);
    assert!(reg.any_available(ProviderKind::Subprocess).await);
    Ok(())
}

#[tokio::test]
async fn nats_registry_detects_heartbeat_and_evicts_on_ttl() -> Result<(), Box<dyn std::error::Error>> {
    let ttl = Duration::from_secs(2);
    let server = crate::test_support::LocalNats::start().await;
    let client = server.client.clone();

    let cancel = CancellationToken::new();
    let registry = NatsWorkerRegistry::new(client.clone(), ttl, cancel.clone()).await?;

    // Not yet available
    assert!(!registry.any_available(ProviderKind::K8sMicroVm).await);

    // Publish a fresh heartbeat
    let worker_id = WorkerId::new(format!("test-worker-{}", crate::generated_ulid::generated_ulid()))?;
    let profile = default_capability_profile();
    let hb_msg = WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: worker_id.clone(),
        provider: ProviderKind::K8sMicroVm,
        capacity: WorkerCapacity {
            max_concurrent_leases: 4,
            current_leases: 0,
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: vec![],
        profile,
        warm_pool_population: HashMap::new(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    };
    let subject = subjects::worker_heartbeat(ProviderKind::K8sMicroVm, &worker_id);
    client
        .publish(subject, bytes::Bytes::from(serde_json::to_vec(&hb_msg)?))
        .await?;

    // Allow time for the subscription task to process
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(registry.any_available(ProviderKind::K8sMicroVm).await);

    // Wait for the TTL to expire (the sweeper runs every ttl/3, about 667ms)
    tokio::time::sleep(ttl + Duration::from_secs(1)).await;
    assert!(!registry.any_available(ProviderKind::K8sMicroVm).await);

    cancel.cancel();
    Ok(())
}

#[tokio::test]
async fn select_provider_prefers_warm_worker_provider() -> Result<(), Box<dyn std::error::Error>> {
    let ttl = Duration::from_secs(30);
    let reg = InMemoryWorkerRegistry::new(ttl);
    let digest = Digest::new(format!("sha256:{}", "b".repeat(64)))?;

    let hl = restore_capable_profile(ProviderKind::Hyperlight);
    reg.inject(WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: WorkerId::new("hl")?,
        provider: ProviderKind::Hyperlight,
        capacity: WorkerCapacity {
            max_concurrent_leases: 4,
            current_leases: 0,
            capabilities: WorkerCapabilities::from(&hl),
        },
        emitted_at: SystemTime::now(),
        cached_digests: vec![],
        profile: hl,
        warm_pool_population: HashMap::new(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    })
    .await;

    let mock = restore_capable_profile(ProviderKind::Mock);
    reg.inject(WorkerHeartbeat {
        supports_lease_epoch: CapabilitySupport::Unsupported,
        worker_id: WorkerId::new("mock")?,
        provider: ProviderKind::Mock,
        capacity: WorkerCapacity {
            max_concurrent_leases: 4,
            current_leases: 0,
            capabilities: WorkerCapabilities::from(&mock),
        },
        emitted_at: SystemTime::now(),
        cached_digests: vec![digest.clone()],
        profile: mock,
        warm_pool_population: HashMap::new(),
        dispatch_protocol: Default::default(),
        native_volume_bindings: Default::default(),
    })
    .await;

    let chosen = select_provider(
        reg.as_ref(),
        &[ProviderKind::Hyperlight, ProviderKind::Mock],
        None,
        Some(&digest),
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(chosen.provider, ProviderKind::Mock);
    assert!(chosen.locality_hit);
    Ok(())
}

#[tokio::test]
async fn snapshot_fallback_refuses_a_provider_without_restore() -> Result<(), Box<dyn std::error::Error>> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    for (provider, can_restore) in [(ProviderKind::Mock, false), (ProviderKind::Hyperlight, true)] {
        let mut profile = restore_capable_profile(provider);
        profile.snapshot.can_restore = CapabilitySupport::from(can_restore);
        registry
            .inject(WorkerHeartbeat {
                supports_lease_epoch: CapabilitySupport::Unsupported,
                worker_id: WorkerId::new(format!("{provider:?}"))?,
                provider,
                dispatch_protocol: Default::default(),
                native_volume_bindings: Default::default(),
                capacity: WorkerCapacity {
                    max_concurrent_leases: 4,
                    current_leases: 0,
                    capabilities: WorkerCapabilities::from(&profile),
                },
                emitted_at: SystemTime::now(),
                cached_digests: vec![],
                profile,
                warm_pool_population: HashMap::new(),
            })
            .await;
    }
    let digest = Digest::new(format!("sha256:{}", "1".repeat(64)))?;
    let selection = select_provider(
        registry.as_ref(),
        &[ProviderKind::Mock, ProviderKind::Hyperlight],
        None,
        Some(&digest),
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(selection.provider, ProviderKind::Hyperlight);
    Ok(())
}

#[tokio::test]
async fn native_volume_selection_requires_every_exact_registered_binding_field()
-> Result<(), Box<dyn std::error::Error>> {
    let binding = NativeVolumeBinding {
        mount: DriveMount {
            drive_id: DriveId {
                tenant: TenantId::new("routing-tenant")?,
                name: DriveName::new("data")?,
            },
            mount_path: MountPath::new("/data")?,
            mode: DriveMountMode::ReadWrite,
        },
        storage: ProviderVolumeRef {
            provider: ProviderKind::Fly,
            volume_id: ProviderVolumeId::new("vol_registered")?,
        },
    };
    for mismatch in 0..7 {
        let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
        let mut wrong = binding.clone();
        match mismatch {
            0 => wrong.mount.drive_id.tenant = TenantId::new("another-tenant")?,
            1 => {
                wrong.mount.drive_id = DriveId {
                    tenant: binding.mount.drive_id.tenant.clone(),
                    name: DriveName::new("another-drive")?,
                }
            }
            2 => wrong.mount.mount_path = MountPath::new("/another-path")?,
            3 => wrong.mount.mode = DriveMountMode::ReadOnly,
            4 => wrong.storage.provider = ProviderKind::Proxmox,
            _ => wrong.storage.volume_id = ProviderVolumeId::new("vol_unregistered")?,
        }
        for (worker_id, bindings) in [
            ("a-incompatible", if mismatch == 6 { vec![] } else { vec![wrong] }),
            ("z-compatible", vec![binding.clone()]),
        ] {
            let profile = restore_capable_profile(ProviderKind::Fly);
            registry
                .inject(WorkerHeartbeat {
                    supports_lease_epoch: CapabilitySupport::Unsupported,
                    worker_id: WorkerId::new(worker_id)?,
                    provider: ProviderKind::Fly,
                    dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
                    native_volume_bindings: bindings,
                    capacity: WorkerCapacity {
                        max_concurrent_leases: 4,
                        current_leases: 0,
                        capabilities: WorkerCapabilities::from(&profile),
                    },
                    emitted_at: SystemTime::now(),
                    cached_digests: vec![],
                    profile,
                    warm_pool_population: HashMap::new(),
                })
                .await;
        }
        let selected = select_provider_for_native_volumes(
            registry.as_ref(),
            &[ProviderKind::Fly],
            None,
            None,
            std::slice::from_ref(&binding),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        let worker = selected
            .worker_id
            .ok_or("native volume selection must name the targeted worker")?;
        assert_eq!(
            worker.as_str(),
            "z-compatible",
            "binding mismatch {mismatch} must be filtered before load sorting"
        );
    }
    Ok(())
}

#[tokio::test]
async fn modern_worker_selection_requires_targeted_cleanup_protocol() -> Result<(), Box<dyn std::error::Error>> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    for (provider, worker_id, protocol) in [
        (ProviderKind::Firecracker, "legacy-fc", WorkerDispatchProtocol::Legacy),
        (ProviderKind::Fly, "targeted-fly", WorkerDispatchProtocol::TargetedV1),
    ] {
        let profile = restore_capable_profile(provider);
        registry
            .inject(WorkerHeartbeat {
                supports_lease_epoch: CapabilitySupport::Unsupported,
                worker_id: WorkerId::new(worker_id)?,
                provider,
                dispatch_protocol: protocol,
                native_volume_bindings: Default::default(),
                capacity: WorkerCapacity {
                    max_concurrent_leases: 4,
                    current_leases: 0,
                    capabilities: WorkerCapabilities::from(&profile),
                },
                emitted_at: SystemTime::now(),
                cached_digests: vec![],
                profile,
                warm_pool_population: HashMap::new(),
            })
            .await;
    }
    let selected = select_provider(
        registry.as_ref(),
        &[ProviderKind::Firecracker, ProviderKind::Fly],
        None,
        None,
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(selected.provider, ProviderKind::Fly);
    Ok(())
}

#[tokio::test]
async fn e2b_selection_prefers_a_live_epoch_bound_target_over_legacy_dispatch() -> Result<(), Box<dyn std::error::Error>>
{
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let mut legacy = hb("legacy-e2b", ProviderKind::E2b, 0, 4, Duration::ZERO)?;
    legacy.profile.provider = ProviderKind::E2b;
    registry.inject(legacy).await;
    let mut targeted = hb("targeted-e2b", ProviderKind::E2b, 1, 4, Duration::ZERO)?;
    targeted.profile.provider = ProviderKind::E2b;
    targeted.supports_lease_epoch = CapabilitySupport::Supported;
    targeted.dispatch_protocol = WorkerDispatchProtocol::TargetedV1;
    registry.inject(targeted).await;
    let selected = select_provider(registry.as_ref(), &[ProviderKind::E2b], None, None)
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        selected.worker_id.as_ref().map(|worker| worker.as_str()),
        Some("targeted-e2b"),
        "a targeted E2B lease must retain its exact assignment for later launch cancellation"
    );
    Ok(())
}

#[tokio::test]
async fn e2b_selection_keeps_explicit_legacy_workers_available() -> Result<(), Box<dyn std::error::Error>> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    registry
        .inject(hb("legacy-e2b", ProviderKind::E2b, 0, 4, Duration::ZERO)?)
        .await;
    let selected = select_provider(registry.as_ref(), &[ProviderKind::E2b], None, None)
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(selected.provider, ProviderKind::E2b);
    assert!(selected.worker_id.is_none());
    Ok(())
}

#[tokio::test]
async fn e2b_selection_never_turns_an_ineligible_target_into_legacy_authority() -> Result<(), Box<dyn std::error::Error>>
{
    // An empty worker id is no longer representable: `WorkerId::new` rejects it.
    for (worker_id, epochs, current, age) in [
        ("missing-epoch", false, 0, Duration::ZERO),
        ("full-target", true, 4, Duration::ZERO),
        ("stale-target", true, 0, Duration::from_secs(120)),
    ] {
        let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
        let mut targeted = hb(worker_id, ProviderKind::E2b, current, 4, age)?;
        targeted.supports_lease_epoch = CapabilitySupport::from(epochs);
        targeted.dispatch_protocol = WorkerDispatchProtocol::TargetedV1;
        registry.inject(targeted).await;
        assert!(
            select_provider(registry.as_ref(), &[ProviderKind::E2b], None, None)
                .await
                .is_err(),
            "an ineligible targeted worker must not receive provider-wide legacy dispatch: {worker_id}"
        );
    }
    assert!(WorkerId::new("").is_err());
    Ok(())
}

#[tokio::test]
async fn existing_worker_lookup_ignores_placement_capacity_and_cordon() -> Result<(), Box<dyn std::error::Error>> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    let worker = WorkerId::new("owned-busy-worker")?;
    registry
        .inject(hb(worker.as_str(), ProviderKind::E2b, 4, 4, Duration::ZERO)?)
        .await;
    registry.fences().replace(&[WorkerFence {
        provider: ProviderKind::E2b,
        worker_id: worker.clone(),
        state: WorkerPlacementState::Cordoned,
        reason: None,
        updated_at: SystemTime::now(),
    }]);
    assert!(registry.eligible_for(ProviderKind::E2b, None).await.is_empty());
    let current = registry.current_worker(ProviderKind::E2b, &worker).await;
    assert_eq!(
        current.as_ref().map(|heartbeat| heartbeat.worker_id.as_str()),
        Some(worker.as_str()),
        "an owned existing lease still needs command control when placement is full or cordoned"
    );
    assert!(
        registry
            .current_worker(ProviderKind::LocalDocker, &worker)
            .await
            .is_none()
    );
    registry
        .inject(hb(worker.as_str(), ProviderKind::E2b, 0, 4, Duration::from_secs(120))?)
        .await;
    assert!(registry.current_worker(ProviderKind::E2b, &worker).await.is_none());
    Ok(())
}

#[tokio::test]
async fn privileged_microvm_routes_to_a_worker_advertising_guest_root() -> Result<(), Box<dyn std::error::Error>> {
    let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
    for (provider, privileged) in [(ProviderKind::K8sMicroVm, false), (ProviderKind::Firecracker, true)] {
        let mut profile = restore_capable_profile(provider);
        profile.isolation.default_tier = IsolationTier::MicroVm;
        profile.isolation.supported_tiers = vec![IsolationTier::MicroVm];
        profile.isolation.supports_privileged = CapabilitySupport::from(privileged);
        registry
            .inject(WorkerHeartbeat {
                supports_lease_epoch: CapabilitySupport::Unsupported,
                worker_id: WorkerId::new(format!("{provider:?}"))?,
                provider,
                capacity: WorkerCapacity {
                    max_concurrent_leases: 4,
                    current_leases: 0,
                    capabilities: WorkerCapabilities::from(&profile),
                },
                emitted_at: SystemTime::now(),
                cached_digests: vec![],
                profile,
                warm_pool_population: HashMap::new(),
                native_volume_bindings: Default::default(),
                dispatch_protocol: if requires_targeted_dispatch(provider) {
                    WorkerDispatchProtocol::TargetedV1
                } else {
                    Default::default()
                },
            })
            .await;
    }
    let requirements = CapabilityRequirements {
        min_isolation_tier: Some(IsolationTier::MicroVm),
        needs_privileged: CapabilityNeed::Required,
        ..Default::default()
    };
    let selected = select_provider(
        registry.as_ref(),
        &[ProviderKind::K8sMicroVm, ProviderKind::Firecracker],
        Some(&requirements),
        None,
    )
    .await
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(selected.provider, ProviderKind::Firecracker);
    assert!(matches!(
        select_provider(
            registry.as_ref(),
            &[ProviderKind::K8sMicroVm],
            Some(&requirements),
            None,
        )
        .await,
        Err(ProviderSelectionError::NoWorkerSatisfiesCapabilities(_))
    ));
    Ok(())
}

fn restore_capable_profile(provider: ProviderKind) -> ProviderCapabilityProfile {
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

/// Every provider part way through moving to targeted dispatch sends a lease to a targeted worker when one can take
/// it, and to its legacy workers otherwise.
#[tokio::test]
async fn providers_moving_to_targeted_dispatch_prefer_a_target_and_keep_legacy_workers()
-> Result<(), Box<dyn std::error::Error>> {
    for provider in [ProviderKind::E2b, ProviderKind::LocalDocker, ProviderKind::Vercel] {
        assert!(prefers_targeted_dispatch(provider), "{provider:?}");
        let registry = InMemoryWorkerRegistry::new(Duration::from_secs(30));
        let mut legacy = hb("legacy", provider, 0, 4, Duration::ZERO)?;
        legacy.profile.provider = provider;
        registry.inject(legacy).await;
        let only_legacy = select_provider(registry.as_ref(), &[provider], None, None)
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(only_legacy.provider, provider);
        assert!(
            only_legacy.worker_id.is_none(),
            "{provider:?} falls back to legacy dispatch"
        );

        let mut targeted = hb("targeted", provider, 1, 4, Duration::ZERO)?;
        targeted.profile.provider = provider;
        targeted.supports_lease_epoch = CapabilitySupport::Supported;
        targeted.dispatch_protocol = WorkerDispatchProtocol::TargetedV1;
        registry.inject(targeted).await;
        let selected = select_provider(registry.as_ref(), &[provider], None, None)
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            selected.worker_id.as_ref().map(|worker| worker.as_str()),
            Some("targeted"),
            "{provider:?} prefers the targeted worker"
        );
    }
    assert!(!prefers_targeted_dispatch(ProviderKind::Modal));
    assert!(
        !prefers_targeted_dispatch(ProviderKind::Fly),
        "fly requires targeted dispatch outright"
    );
    Ok(())
}
