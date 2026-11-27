//! The authority host view of the provider: which leases it holds, and how it freezes, thaws and purges them.

mod fake_cluster;
mod provider_support;
mod support;

use std::sync::Arc;

use fake_cluster::{Effect, Fault, Verb};
use provider_support::{Harness, lease, registry_request};
use support::TestResult;
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_kubevirt_worker::KubeVirtProviderError;
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, FreezeOutcome, HeldLease, Provider, PurgeOutcome,
    ThawOutcome,
};

async fn held_one(harness: &Harness) -> Result<HeldLease, Box<dyn std::error::Error>> {
    let mut held = harness.provider.held_leases().await?;
    assert_eq!(held.len(), 1);
    held.pop().ok_or_else(|| "no held lease".into())
}

fn provider_lease(
    lease_id: &LeaseId,
    provider: ProviderKind,
    key: &str,
) -> Result<HeldLease, Box<dyn std::error::Error>> {
    Ok(HeldLease::Provider {
        lease_id: lease_id.clone(),
        provider,
        resource_id: AuthorityResourceId::new(key)?,
    })
}

#[tokio::test(start_paused = true)]
async fn a_held_lease_is_reported_with_its_resource_identity() -> TestResult {
    let harness = Harness::new()?;
    assert!(harness.provider.held_leases().await?.is_empty());
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let held = held_one(&harness).await?;
    assert_eq!(held.lease_id(), &lease());
    assert!(matches!(
        held,
        HeldLease::Provider {
            provider: ProviderKind::KubeVirt,
            ..
        }
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn freeze_is_unsupported_and_thaw_releases_nothing() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let held = held_one(&harness).await?;
    assert_eq!(harness.provider.freeze(&held).await?, FreezeOutcome::Unsupported);
    assert_eq!(harness.provider.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    assert_eq!(harness.cluster.count_requests(Verb::Delete, "virtualmachines"), 0);

    let other = provider_lease(&lease(), ProviderKind::KubeVirt, "another-vm")?;
    assert_eq!(harness.provider.freeze(&other).await?, FreezeOutcome::Stale);
    assert_eq!(harness.provider.thaw(&other).await?, ThawOutcome::Stale);
    let mock = provider_lease(&lease(), ProviderKind::Mock, "another-vm")?;
    assert_eq!(harness.provider.freeze(&mock).await?, FreezeOutcome::Stale);
    assert_eq!(
        AuthorityHost::purge(&harness.provider, &mock).await?,
        PurgeOutcome::Stale
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn purge_deletes_exactly_the_held_resources() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let held = held_one(&harness).await?;
    let other = provider_lease(&lease(), ProviderKind::KubeVirt, "another-vm")?;
    assert_eq!(
        AuthorityHost::purge(&harness.provider, &other).await?,
        PurgeOutcome::Stale
    );
    assert!(harness.cluster.names("virtualmachines").len() == 1);

    assert_eq!(
        AuthorityHost::purge(&harness.provider, &held).await?,
        PurgeOutcome::Purged
    );
    assert!(harness.cluster.names("virtualmachines").is_empty());
    assert!(harness.cluster.names("datavolumes").is_empty());
    assert!(harness.provider.held_leases().await?.is_empty());
    assert_eq!(
        AuthorityHost::purge(&harness.provider, &held).await?,
        PurgeOutcome::Stale
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_purge_keeps_the_lease_held_once() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    let held = held_one(&harness).await?;
    harness
        .cluster
        .inject(Fault::new(Verb::Delete, "virtualmachines", Effect::Reject(500)).times(1));
    let error = AuthorityHost::purge(&harness.provider, &held)
        .await
        .err()
        .ok_or("purged")?;
    let AuthorityHostError::Failed(inner) = error else {
        return Err("not a failed purge".into());
    };
    assert!(
        inner
            .downcast::<KubeVirtProviderError>()
            .is_ok_and(|kind| matches!(*kind, KubeVirtProviderError::Ops(_)))
    );
    assert_eq!(harness.cluster.names("virtualmachines").len(), 1);
    assert_eq!(held_one(&harness).await?, held);
    assert_eq!(
        AuthorityHost::purge(&harness.provider, &held).await?,
        PurgeOutcome::Purged
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_interrupted_allocation_is_reported_as_held() -> TestResult {
    let harness = Harness::new()?;
    let release = Arc::new(tokio::sync::Notify::new());
    harness.cluster.inject(Fault::new(
        Verb::Create,
        "virtualmachines",
        Effect::ApplyThenHold(release),
    ));
    let request = registry_request()?;
    let id = lease();
    let abandoned =
        tokio::time::timeout(std::time::Duration::from_secs(5), harness.provider.lease(&request, &id)).await;
    assert!(abandoned.is_err());
    assert_eq!(harness.provider.held_leases().await?.len(), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_allocation_that_holds_nothing_yet_is_not_reported() -> TestResult {
    let harness = Harness::new()?;
    let release = Arc::new(tokio::sync::Notify::new());
    harness
        .cluster
        .inject(Fault::new(Verb::Create, "datavolumes", Effect::ApplyThenHold(release)));
    let request = registry_request()?;
    let id = lease();
    let abandoned =
        tokio::time::timeout(std::time::Duration::from_secs(5), harness.provider.lease(&request, &id)).await;
    assert!(abandoned.is_err());
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_held_and_retained_is_reported_once() -> TestResult {
    let harness = Harness::new()?;
    harness.provider.lease(&registry_request()?, &lease()).await?;
    harness
        .cluster
        .inject(Fault::new(Verb::Delete, "virtualmachines", Effect::Reject(500)).times(1));
    harness.provider.cancel(&lease()).await.err().ok_or("cancelled")?;
    held_one(&harness).await?;
    harness.provider.cancel(&lease()).await?;
    assert!(harness.provider.held_leases().await?.is_empty());
    Ok(())
}
