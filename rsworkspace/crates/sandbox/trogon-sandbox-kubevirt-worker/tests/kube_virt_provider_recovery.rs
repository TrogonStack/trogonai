//! A restarted provider settles what the custody directory records from before.

mod fake_cluster;
mod provider_support;
mod support;

use std::sync::Arc;

use fake_cluster::{Effect, Fault, Verb};
use provider_support::{Harness, lease, refused, registry_request, rejected};
use support::TestResult;
use trogon_sandbox_kubevirt_worker::KubeVirtProviderError;
use trogon_sandbox_protocol::{AuthorityHost, Provider};

fn entry_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

async fn leased() -> Result<(Harness, String), Box<dyn std::error::Error>> {
    let harness = Harness::new()?;
    let granted = harness.provider.lease(&registry_request()?, &lease()).await?;
    Ok((harness, granted.sandbox_id.as_str().to_owned()))
}

#[tokio::test(start_paused = true)]
async fn an_empty_custody_directory_recovers_nothing() -> TestResult {
    let harness = Harness::new()?.restart()?;
    harness.provider.recover_leases().await?;
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_ready_lease_is_held_again_and_can_be_cancelled() -> TestResult {
    let (harness, vm) = leased().await?;
    let before = harness.provider.held_leases().await?;
    let harness = harness.restart()?;
    harness.provider.recover_leases().await?;
    assert_eq!(harness.provider.held_leases().await?, before);
    let guest = trogon_sandbox_kubevirt_worker::GuestLocator::vm_name(&harness.provider, &lease()).await;
    assert_eq!(guest, Some(vm));

    harness.provider.cancel(&lease()).await?;
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.cluster.names("datavolumes").is_empty());
    let harness = harness.restart()?;
    harness.provider.recover_leases().await?;
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_vm_deleted_meanwhile_leaves_its_disk_for_cleanup() -> TestResult {
    let (harness, vm) = leased().await?;
    let harness = harness.restart()?;
    harness.cluster.with_store(|store| {
        store.delete("virtualmachines", &vm);
    });
    harness.provider.recover_leases().await?;
    assert_eq!(harness.provider.held_leases().await?.len(), 1);
    assert_eq!(
        trogon_sandbox_kubevirt_worker::GuestLocator::vm_name(&harness.provider, &lease()).await,
        None
    );
    harness.provider.cancel(&lease()).await?;
    assert!(harness.cluster.names("datavolumes").is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_whose_resources_are_all_gone_is_settled() -> TestResult {
    let (harness, vm) = leased().await?;
    let harness = harness.restart()?;
    harness.cluster.with_store(|store| {
        store.delete("virtualmachines", &vm);
        store.delete("datavolumes", &format!("dv-{}", "lease-1"));
    });
    harness.provider.recover_leases().await?;
    assert!(harness.provider.held_leases().await?.is_empty());
    harness.provider.lease(&registry_request()?, &lease()).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_uncertain_create_stays_unresolved_across_restarts() -> TestResult {
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
    let harness = harness.restart()?;
    let error = harness.provider.recover_leases().await.err().ok_or("recovered")?;
    rejected(error, |kind| {
        matches!(kind, KubeVirtProviderError::Unresolved { count: 1 })
    })?;
    assert_eq!(harness.provider.held_leases().await?.len(), 1);
    let error = harness
        .provider
        .lease(&registry_request()?, &lease())
        .await
        .err()
        .ok_or("leased")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::CleanupRetained))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_cluster_that_cannot_be_read_keeps_the_lease_unresolved() -> TestResult {
    let (harness, _) = leased().await?;
    let harness = harness.restart()?;
    harness
        .cluster
        .inject(Fault::new(Verb::Get, "virtualmachines", Effect::Reject(500)).always());
    let error = harness.provider.recover_leases().await.err().ok_or("recovered")?;
    rejected(error, |kind| {
        matches!(kind, KubeVirtProviderError::Unresolved { count: 1 })
    })?;
    assert_eq!(harness.provider.held_leases().await?.len(), 1);
    harness.cluster.clear_faults();
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_vm_replaced_by_another_is_never_adopted() -> TestResult {
    let (harness, vm) = leased().await?;
    let harness = harness.restart()?;
    harness.cluster.recreate("virtualmachines", &vm);
    let error = harness.provider.recover_leases().await.err().ok_or("recovered")?;
    rejected(error, |kind| {
        matches!(kind, KubeVirtProviderError::Unresolved { count: 1 })
    })?;
    assert!(harness.cluster.get("virtualmachines", &vm).is_some());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_vm_without_the_lease_labels_is_foreign() -> TestResult {
    let (harness, vm) = leased().await?;
    let harness = harness.restart()?;
    harness.cluster.update("virtualmachines", &vm, |object| {
        if let Some(labels) = support::metadata_map(object, "labels") {
            labels.clear();
        }
    });
    let error = harness.provider.recover_leases().await.err().ok_or("recovered")?;
    rejected(error, |kind| matches!(kind, KubeVirtProviderError::ForeignResource))?;
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_disk_that_vanishes_during_recovery_is_reported() -> TestResult {
    let (harness, _) = leased().await?;
    let harness = harness.restart()?;
    harness.cluster.inject(
        Fault::new(
            Verb::Get,
            "datavolumes",
            Effect::before(|store| {
                store.delete("datavolumes", "dv-lease-1");
            }),
        )
        .skip(1)
        .times(1),
    );
    let error = harness.provider.recover_leases().await.err().ok_or("recovered")?;
    rejected(error, |kind| matches!(kind, KubeVirtProviderError::HeldDiskMissing))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_disk_replaced_during_recovery_is_foreign() -> TestResult {
    let (harness, _) = leased().await?;
    let harness = harness.restart()?;
    harness.cluster.inject(
        Fault::new(
            Verb::Get,
            "datavolumes",
            Effect::before(|store| {
                store.recreate("datavolumes", "dv-lease-1");
            }),
        )
        .skip(1)
        .times(1),
    );
    let error = harness.provider.recover_leases().await.err().ok_or("recovered")?;
    rejected(error, |kind| matches!(kind, KubeVirtProviderError::ForeignResource))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_guest_that_does_not_come_back_fails_recovery() -> TestResult {
    let (harness, vm) = leased().await?;
    let harness = harness.restart()?;
    harness.cluster.with_store(|store| {
        store.delete("virtualmachineinstances", &vm);
    });
    harness.cluster.set_reactions(fake_cluster::Reactions::none());
    harness.provider.recover_leases().await.err().ok_or("recovered")?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancel_without_custody_is_refused_while_the_lease_is_held() -> TestResult {
    let (harness, _) = leased().await?;
    for entry in std::fs::read_dir(harness.dir.path())? {
        let path = entry?.path();
        let name = entry_name(&path);
        if name.starts_with("lease-") {
            std::fs::remove_file(path)?;
        }
    }
    let error = harness.provider.cancel(&lease()).await.err().ok_or("cancelled")?;
    refused(error, |kind| matches!(kind, KubeVirtProviderError::CustodyMissing))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_live_provider_excludes_a_second_one_and_a_foreign_namespace_is_refused() -> TestResult {
    let harness = Harness::new()?;
    let second = provider_support::build(
        &harness.cluster,
        harness.dir.path(),
        Arc::new(provider_support::Backend::new()),
        harness.probe.clone(),
    );
    assert!(second.is_err());
    Ok(())
}
