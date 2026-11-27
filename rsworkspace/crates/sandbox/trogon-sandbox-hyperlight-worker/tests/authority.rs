mod support;

use support::{Rig, TestResult, lease, request};
use trogon_sandbox_contract::{DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, ProviderKind, WorkerId};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityResourceId, FreezeOutcome, HeldLease, Provider, PurgeOutcome, ThawOutcome,
};

fn host(rig: &Rig) -> &dyn AuthorityHost {
    &*rig.provider
}

async fn held(rig: &Rig, id: &str) -> Result<HeldLease, Box<dyn std::error::Error>> {
    rig.provider.lease(&request("echo")?, &lease(id)).await?;
    rig.provider
        .held_leases()
        .await?
        .into_iter()
        .find(|held| held.lease_id() == &lease(id))
        .ok_or_else(|| "lease not reported".into())
}

fn other_resource(held: &HeldLease, resource: &str) -> Result<HeldLease, Box<dyn std::error::Error>> {
    Ok(HeldLease::Provider {
        lease_id: held.lease_id().clone(),
        provider: ProviderKind::Hyperlight,
        resource_id: AuthorityResourceId::new(resource)?,
    })
}

fn foreign(held: &HeldLease) -> Result<HeldLease, Box<dyn std::error::Error>> {
    Ok(HeldLease::Provider {
        lease_id: held.lease_id().clone(),
        provider: ProviderKind::Space,
        resource_id: AuthorityResourceId::new("r")?,
    })
}

#[tokio::test]
async fn reports_the_leases_it_holds() -> TestResult {
    let rig = Rig::start().await?;
    assert!(host(&rig).held_leases().await?.is_empty());
    let held = held(&rig, "l1").await?;
    assert!(matches!(
        held,
        HeldLease::Provider {
            provider: ProviderKind::Hyperlight,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn cannot_freeze_a_guest() -> TestResult {
    let rig = Rig::start().await?;
    let held = held(&rig, "l1").await?;
    assert_eq!(host(&rig).freeze(&held).await?, FreezeOutcome::Unsupported);
    assert_eq!(
        host(&rig).freeze(&other_resource(&held, "elsewhere")?).await?,
        FreezeOutcome::Stale
    );
    assert_eq!(host(&rig).freeze(&foreign(&held)?).await?, FreezeOutcome::Stale);
    Ok(())
}

#[tokio::test]
async fn never_owns_a_thawed_guest() -> TestResult {
    let rig = Rig::start().await?;
    let held = held(&rig, "l1").await?;
    assert_eq!(host(&rig).thaw(&held).await?, ThawOutcome::NoLongerOwned);
    assert_eq!(
        host(&rig).thaw(&other_resource(&held, "elsewhere")?).await?,
        ThawOutcome::Stale
    );
    assert_eq!(host(&rig).thaw(&foreign(&held)?).await?, ThawOutcome::Stale);
    rig.provider.cancel(&lease("l1")).await?;
    assert_eq!(host(&rig).thaw(&held).await?, ThawOutcome::NoLongerOwned);
    Ok(())
}

#[tokio::test]
async fn purges_only_what_it_owns() -> TestResult {
    let rig = Rig::start().await?;
    let held = held(&rig, "l1").await?;
    assert_eq!(
        host(&rig).purge(&other_resource(&held, "elsewhere")?).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(host(&rig).purge(&foreign(&held)?).await?, PurgeOutcome::Stale);
    assert_eq!(host(&rig).purge(&held).await?, PurgeOutcome::Purged);
    assert_eq!(rig.provider.active_lease_count(), 0);
    assert_eq!(rig.hypervisor.unloads(), 0);
    assert_eq!(rig.hypervisor.guests_dropped(), 1);
    assert_eq!(host(&rig).purge(&held).await?, PurgeOutcome::Settled);
    Ok(())
}

#[tokio::test]
async fn purge_loses_to_a_cancel_that_got_there_first() -> TestResult {
    let rig = Rig::start().await?;
    let held = held(&rig, "l1").await?;
    rig.provider.cancel(&lease("l1")).await?;
    assert_eq!(host(&rig).purge(&held).await?, PurgeOutcome::Settled);
    Ok(())
}

#[tokio::test]
async fn ignores_leases_bound_to_an_assignment() -> TestResult {
    let rig = Rig::start().await?;
    let held = held(&rig, "l1").await?;
    let targeted = HeldLease::Targeted(LeaseAttemptIdentity {
        lease_id: held.lease_id().clone(),
        provider: ProviderKind::Hyperlight,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new("a1")?,
            epoch: None,
        },
    });
    assert_eq!(host(&rig).freeze(&targeted).await?, FreezeOutcome::Stale);
    assert_eq!(host(&rig).thaw(&targeted).await?, ThawOutcome::Stale);
    assert_eq!(host(&rig).purge(&targeted).await?, PurgeOutcome::Stale);
    Ok(())
}
