//! Leasing and cancelling KubeVirt VMs through the provider, against the in-process fake cluster.

mod fake_cluster;
mod provider_support;
mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use fake_cluster::{Effect, Fault, Verb};
use provider_support::{
    Backend, Harness, Probe, lease, parts, refused, registry_request, request_with, snapshot_request,
};
use support::{TestResult, digest, reference, seed_warm_vm};
use trogon_sandbox_contract::{
    ArtifactType, CloneAuth, EmptyObject, HostMountAccess, NativeSnapshotId, NativeSnapshotRef, PrivilegedAccess,
    ProviderKind, SandboxImage, SnapshotTag, SnapshotTagName, TenantId, VolumeMode, WorkspaceSource,
};
use trogon_sandbox_kubevirt_worker::{KubeVirtOpsError, KubeVirtProviderError, profile};
use trogon_sandbox_protocol::{AuthorityHost, Provider, ProviderError};

type Check = fn(&KubeVirtProviderError) -> bool;

#[tokio::test]
async fn the_provider_describes_itself() -> TestResult {
    let harness = Harness::new()?;
    let provider = &harness.provider;
    assert_eq!(provider.kind(), ProviderKind::KubeVirt);
    assert_eq!(provider.profile(), &profile());
    assert_eq!(provider.worker_id().as_str(), support::worker());
    assert_eq!(provider.config().namespace, harness.cluster.namespace());
    assert_eq!(Arc::strong_count(provider.warm_pool()), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cold_lease_creates_its_vm_and_cancel_removes_everything() -> TestResult {
    let harness = Harness::new()?;
    let granted = harness.provider.lease(&registry_request()?, &lease()).await?;
    assert_eq!(
        granted.worker_id.as_ref().map(|worker| worker.as_str()),
        Some(support::worker())
    );
    assert_eq!(
        harness.cluster.names("virtualmachines"),
        vec![granted.sandbox_id.as_str().to_owned()]
    );
    assert_eq!(harness.cluster.names("datavolumes").len(), 1);
    assert_eq!(harness.probe.probed(), vec![granted.sandbox_id.as_str().to_owned()]);

    harness.provider.cancel(&lease()).await?;
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.cluster.names("datavolumes").is_empty());
    harness.provider.cancel(&lease()).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_small_guest_boots_with_the_minimum_memory() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let vm = harness.cluster.objects("virtualmachines");
    let memory = support::text(&vm[0], "/spec/template/spec/domain/resources/requests/memory");
    let guest = support::text(&vm[0], "/spec/template/spec/domain/memory/guest");
    assert!(
        memory == Some("512Mi") || guest == Some("512Mi"),
        "{memory:?} {guest:?}"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_held_by_this_worker_cannot_be_leased_again() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let error = harness
        .provider
        .lease(&registry_request()?, &lease())
        .await
        .err()
        .ok_or("leased twice")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::LeaseHeld))?;
    assert_eq!(harness.cluster.names("virtualmachines").len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn unsupported_requests_are_refused_before_anything_is_created() -> TestResult {
    let harness = Harness::new()?;
    let workspaces = [
        (
            WorkspaceSource::Clone {
                url: "https://example.invalid/repo.git".to_owned(),
                sha: "0".repeat(40),
                base_sha: None,
                auth: CloneAuth::None(EmptyObject::default()),
            },
            "Clone",
        ),
        (
            WorkspaceSource::HostMount {
                path: "/work".to_owned(),
                access: HostMountAccess::default(),
            },
            "HostMount",
        ),
        (
            WorkspaceSource::S3Volume {
                bucket: "bucket".to_owned(),
                prefix: "prefix".to_owned(),
                mode: VolumeMode::ReadWrite,
                snapshot: None,
            },
            "S3Volume",
        ),
    ];
    for (workspace, name) in workspaces {
        let mut spec = parts(provider_support::registry_image())?;
        spec.workspace = workspace;
        let error = harness
            .provider
            .lease(&request_with(spec)?, &lease())
            .await
            .err()
            .ok_or("leased")?;
        refused(
            error,
            |kind| matches!(kind, KubeVirtProviderError::WorkspaceUnsupported { workspace } if *workspace == name),
        )?;
    }

    let mut spec = parts(provider_support::registry_image())?;
    spec.env = BTreeMap::from([("NAME".to_owned(), "value".to_owned())]);
    let error = harness
        .provider
        .lease(&request_with(spec)?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::EnvUnsupported))?;

    let images: [(SandboxImage, Check); 3] = [
        (
            SandboxImage::SnapshotTag {
                tag: SnapshotTag {
                    tenant: TenantId::new("tenant-1")?,
                    name: SnapshotTagName::new("tag")?,
                },
                artifact_type: ArtifactType::KubevirtDiskV1,
                create_from: None,
            },
            |kind| matches!(kind, KubeVirtProviderError::SnapshotTagUnresolved),
        ),
        (
            SandboxImage::HyperlightModule {
                reference: reference(&digest('a')?),
            },
            |kind| matches!(kind, KubeVirtProviderError::HyperlightUnsupported),
        ),
        (
            SandboxImage::NativeSnapshot {
                reference: NativeSnapshotRef {
                    provider: ProviderKind::KubeVirt,
                    id: NativeSnapshotId::new("native")?,
                    expires_at: None,
                },
            },
            |kind| matches!(kind, KubeVirtProviderError::NativeSnapshotUnsupported),
        ),
    ];
    for (image, expected) in images {
        let error = harness
            .provider
            .lease(&request_with(parts(image)?)?, &lease())
            .await
            .err()
            .ok_or("leased")?;
        refused(error, expected)?;
    }

    let mut spec = parts(provider_support::registry_image())?;
    spec.privileged = PrivilegedAccess::Granted;
    let error = harness
        .provider
        .lease(&request_with(spec)?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    assert!(matches!(error, ProviderError::PrivilegedUnsupported(_)));

    assert_eq!(harness.cluster.count_requests(Verb::Create, "virtualmachines"), 0);
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failing_guest_probe_does_not_fail_the_lease() -> TestResult {
    let harness = Harness::with(Arc::new(Backend::new()), Arc::new(Probe::failing()))?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    assert_eq!(harness.probe.probed().len(), 1);
    assert_eq!(harness.provider.held_leases().await?.len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_lease_restores_its_disk_through_the_backend() -> TestResult {
    let harness = Harness::new()?;
    let artifact = reference(&digest('a')?);
    harness.provider.lease(&snapshot_request(artifact)?, &lease()).await?;
    assert_eq!(harness.backend.requested().len(), 1);
    assert_eq!(harness.cluster.names("virtualmachines").len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_lease_without_a_volume_store_is_refused() -> TestResult {
    let cluster = Arc::new(fake_cluster::FakeCluster::new());
    let dir = tempfile::TempDir::new()?;
    let mut config = provider_support::config(&cluster)?;
    config.volume = None;
    let provider = trogon_sandbox_kubevirt_worker::KubeVirtProvider::new(
        config,
        cluster.client(),
        dir.path(),
        Arc::new(Backend::new()),
    )?;
    let request = snapshot_request(reference(&digest('a')?))?;
    let error = provider.lease(&request, &lease()).await.err().ok_or("leased")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::VolumeUnconfigured))?;
    assert!(cluster.names("virtualmachines").is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failing_disk_source_fails_the_lease_and_leaves_nothing() -> TestResult {
    let harness = Harness::with(Arc::new(Backend::failing_disks()), Arc::new(Probe::default()))?;
    let request = snapshot_request(reference(&digest('a')?))?;
    let error = harness.provider.lease(&request, &lease()).await.err().ok_or("leased")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::DiskSource(_)))?;
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.provider.held_leases().await?.is_empty());
    harness.provider.cancel(&lease()).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_lease_claims_a_matching_warm_vm() -> TestResult {
    let harness = Harness::new()?;
    let digest = digest('a')?;
    let seeded = seed_warm_vm(&harness.cluster, "warm-1", &digest, Some(512))?;
    let granted = harness
        .provider
        .lease(&snapshot_request(reference(&digest))?, &lease())
        .await?;
    assert_eq!(granted.sandbox_id.as_str(), "warm-1");
    assert!(harness.backend.requested().is_empty());
    let vm = harness
        .cluster
        .get("virtualmachines", "warm-1")
        .ok_or("the warm VM is gone")?;
    assert_eq!(
        support::text(&vm, "/metadata/labels/sandbox.trogon.ai~1lease-id"),
        Some("lease-1")
    );
    assert_eq!(support::uid(&vm)?, support::uid(&seeded.vm)?);
    harness.provider.cancel(&lease()).await?;
    assert!(harness.cluster.get("virtualmachines", "warm-1").is_none());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_sized_snapshot_lease_cold_starts_instead_of_claiming_a_warm_vm() -> TestResult {
    let harness = Harness::new()?;
    let digest = digest('a')?;
    seed_warm_vm(&harness.cluster, "warm-1", &digest, Some(512))?;
    let mut spec = parts(provider_support::snapshot_image(reference(&digest)))?;
    spec.vcpu_millis = Some(500);
    let granted = harness.provider.lease(&request_with(spec)?, &lease()).await?;
    assert_ne!(granted.sandbox_id.as_str(), "warm-1");
    assert_eq!(harness.backend.requested().len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_warm_claim_that_never_becomes_ready_is_released() -> TestResult {
    let harness = Harness::new()?;
    let digest = digest('a')?;
    seed_warm_vm(&harness.cluster, "warm-1", &digest, Some(512))?;
    harness.cluster.set_reactions(fake_cluster::Reactions::none());
    harness.cluster.with_store(|store| {
        store.delete("virtualmachineinstances", "warm-1");
    });
    let error = harness
        .provider
        .lease(&snapshot_request(reference(&digest))?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::Ops(_)))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_guest_that_never_boots_fails_the_lease_and_removes_everything() -> TestResult {
    let harness = Harness::new()?;
    harness.cluster.set_reactions(fake_cluster::Reactions {
        vms_boot: false,
        ..fake_cluster::Reactions::default()
    });
    let error = harness
        .provider
        .lease(&registry_request()?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    drop(error);
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.cluster.names("datavolumes").is_empty());
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_uncertain_vm_create_keeps_custody_until_it_is_resolved() -> TestResult {
    let harness = Harness::new()?;
    harness
        .cluster
        .inject(Fault::new(Verb::Create, "virtualmachines", Effect::Reject(500)).always());
    harness
        .provider
        .lease(&registry_request()?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    assert_eq!(harness.cluster.names("datavolumes").len(), 1);
    assert_eq!(harness.provider.held_leases().await?.len(), 1);
    let error = harness.provider.cancel(&lease()).await.err().ok_or("cancelled")?;
    refused(error, |kind| {
        matches!(kind, KubeVirtProviderError::Ops(KubeVirtOpsError::OwnershipUnresolved))
    })?;
    assert_eq!(harness.cluster.names("datavolumes").len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_cleanup_after_a_failed_allocation_blocks_the_next_lease_until_cancel() -> TestResult {
    let harness = Harness::new()?;
    harness.cluster.set_reactions(fake_cluster::Reactions {
        vms_boot: false,
        ..fake_cluster::Reactions::default()
    });
    harness
        .cluster
        .inject(Fault::new(Verb::Delete, "virtualmachines", Effect::Reject(500)).times(1));
    harness
        .provider
        .lease(&registry_request()?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    assert_eq!(harness.provider.held_leases().await?.len(), 1);
    let error = harness
        .provider
        .lease(&registry_request()?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::CleanupRetained))?;
    harness.provider.cancel(&lease()).await?;
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.cluster.names("datavolumes").is_empty());
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_dropped_lease_future_keeps_its_custody_for_cancel() -> TestResult {
    let harness = Harness::new()?;
    let release = Arc::new(tokio::sync::Notify::new());
    harness.cluster.inject(Fault::new(
        Verb::Create,
        "virtualmachines",
        Effect::ApplyThenHold(release),
    ));
    let request = registry_request()?;
    let id = lease();
    let leasing = harness.provider.lease(&request, &id);
    let abandoned = tokio::time::timeout(std::time::Duration::from_secs(5), leasing).await;
    assert!(abandoned.is_err());
    assert_eq!(harness.cluster.names("virtualmachines").len(), 1);
    let error = harness.provider.lease(&request, &lease()).await.err().ok_or("leased")?;
    refused(error, |kind| {
        matches!(kind, KubeVirtProviderError::AllocationInterrupted)
    })?;
    let error = harness.provider.cancel(&lease()).await.err().ok_or("cancelled")?;
    refused(error, |kind| {
        matches!(kind, KubeVirtProviderError::Ops(KubeVirtOpsError::OwnershipUnresolved))
    })?;
    assert_eq!(harness.cluster.names("virtualmachines").len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn snapshot_and_checkpoint_run_through_the_backend() -> TestResult {
    let harness = Harness::new()?;
    let granted = harness.provider.lease(&registry_request()?, &lease()).await?;
    let target = reference(&digest('b')?);
    let exported = harness.provider.snapshot(&lease(), &target).await?;
    assert_eq!(exported.digest, target.digest);
    let checkpoint = harness.provider.checkpoint_running(&lease(), &target).await?;
    assert_eq!(checkpoint.bytes, 7);
    let vm = granted.sandbox_id.as_str();
    assert_eq!(
        harness.backend.calls(),
        vec![format!("export {vm}"), format!("checkpoint {vm}")]
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn snapshots_refuse_what_they_cannot_serve() -> TestResult {
    let harness = Harness::new()?;
    let target = reference(&digest('b')?);
    let error = harness
        .provider
        .snapshot(&lease(), &target)
        .await
        .err()
        .ok_or("exported")?;
    refused(error, |kind| {
        matches!(kind, KubeVirtProviderError::NoActiveLease { .. })
    })?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let mut other = target.clone();
    other.artifact_type = ArtifactType::SbomV1;
    let error = harness
        .provider
        .snapshot(&lease(), &other)
        .await
        .err()
        .ok_or("exported")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::ArtifactType { .. }))?;
    let error = harness
        .provider
        .checkpoint_running(&lease(), &other)
        .await
        .err()
        .ok_or("exported")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::ArtifactType { .. }))?;
    assert!(harness.backend.calls().is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failing_export_is_reported() -> TestResult {
    let harness = Harness::with(Arc::new(Backend::failing_exports()), Arc::new(Probe::default()))?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let target = reference(&digest('b')?);
    let error = harness
        .provider
        .snapshot(&lease(), &target)
        .await
        .err()
        .ok_or("exported")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::Snapshot(_)))?;
    let error = harness
        .provider
        .checkpoint_running(&lease(), &target)
        .await
        .err()
        .ok_or("exported")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::Snapshot(_)))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_checkpoint_needs_a_running_guest_before_and_after() -> TestResult {
    let harness = Harness::new()?;
    let granted = harness.provider.lease(&registry_request()?, &lease()).await?;
    let target = reference(&digest('b')?);
    harness.cluster.with_store(|store| {
        store.delete("virtualmachineinstances", granted.sandbox_id.as_str());
    });
    let error = harness
        .provider
        .checkpoint_running(&lease(), &target)
        .await
        .err()
        .ok_or("checkpointed")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::NotRunning { .. }))?;

    let cluster = Arc::new(fake_cluster::FakeCluster::new());
    let backend = Arc::new(Backend::stopping(cluster.clone()));
    let stopping = Harness::over(cluster, tempfile::TempDir::new()?, backend, Arc::new(Probe::default()))?;
    stopping.provider.lease(&registry_request()?, &lease()).await?;
    let error = stopping
        .provider
        .checkpoint_running(&lease(), &target)
        .await
        .err()
        .ok_or("checkpointed")?;
    refused(error, |kind| {
        matches!(kind, KubeVirtProviderError::StoppedDuringCheckpoint { .. })
    })?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn operations_on_one_lease_wait_for_each_other() -> TestResult {
    let harness = Harness::new()?;
    let release = Arc::new(tokio::sync::Notify::new());
    harness.cluster.inject(Fault::new(
        Verb::Create,
        "virtualmachines",
        Effect::ApplyThenHold(release.clone()),
    ));
    let request = registry_request()?;
    let id = lease();
    let leasing = harness.provider.lease(&request, &id);
    let cancelling = async {
        harness
            .cluster
            .wait_for_requests(Verb::Create, "virtualmachines", 1)
            .await;
        harness.provider.cancel(&id).await
    };
    let releasing = async {
        harness
            .cluster
            .wait_for_requests(Verb::Create, "virtualmachines", 1)
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        release.notify_one();
    };
    let (leased, cancelled, ()) = tokio::join!(leasing, cancelling, releasing);
    leased?;
    cancelled?;
    assert!(harness.cluster.names("virtualmachines").is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn custody_that_vanishes_during_an_interrupted_allocation_is_reported_not_fatal() -> TestResult {
    let harness = Harness::new()?;
    let release = Arc::new(tokio::sync::Notify::new());
    harness.cluster.inject(Fault::new(
        Verb::Create,
        "virtualmachines",
        Effect::ApplyThenHold(release),
    ));
    let request = registry_request()?;
    let id = lease();
    let leasing = harness.provider.lease(&request, &id);
    let vanishing = async {
        harness
            .cluster
            .wait_for_requests(Verb::Create, "virtualmachines", 1)
            .await;
        for entry in std::fs::read_dir(harness.dir.path())? {
            let path = entry?.path();
            let named = path
                .file_name()
                .map(|name| name.to_string_lossy().starts_with("lease-"));
            if named == Some(true) {
                std::fs::remove_file(path)?;
            }
        }
        Ok::<(), std::io::Error>(())
    };
    let outcome = tokio::select! {
        _ = leasing => None,
        removed = vanishing => Some(removed),
    };
    assert!(matches!(outcome, Some(Ok(()))));
    Ok(())
}
