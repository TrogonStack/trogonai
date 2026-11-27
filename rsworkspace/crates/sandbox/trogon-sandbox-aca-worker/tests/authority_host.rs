mod support;

use support::TestResult;
use support::lease::Lease;
use support::provider::Fixture;
use trogon_sandbox_aca_worker::SessionDeleteOutcome;
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, FreezeOutcome, HeldLease, Provider, PurgeOutcome,
    ThawOutcome,
};

async fn held(fixture: &Fixture, lease: &str) -> Result<HeldLease, Box<dyn std::error::Error>> {
    fixture
        .provider
        .lease(&Lease::empty().build()?, &LeaseId::new(lease))
        .await?;
    fixture
        .provider
        .held_leases()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| "the lease is not held".into())
}

fn unknown_lease() -> Result<HeldLease, Box<dyn std::error::Error>> {
    Ok(HeldLease::Provider {
        lease_id: LeaseId::new("never-leased"),
        provider: ProviderKind::AzureContainerApps,
        resource_id: AuthorityResourceId::new("no-such-session")?,
    })
}

fn foreign_lease() -> Result<HeldLease, Box<dyn std::error::Error>> {
    Ok(HeldLease::Provider {
        lease_id: LeaseId::new("someone-elses"),
        provider: ProviderKind::V8Wasm,
        resource_id: AuthorityResourceId::new("anything")?,
    })
}

#[tokio::test]
async fn a_lease_is_held_until_it_is_cancelled() -> TestResult {
    let fixture = Fixture::new()?;
    assert!(fixture.provider.held_leases().await?.is_empty());

    let lease = held(&fixture, "lease-a").await?;
    assert!(matches!(
        &lease,
        HeldLease::Provider { provider: ProviderKind::AzureContainerApps, lease_id, .. } if lease_id.as_str() == "lease-a"
    ));
    fixture.provider.cancel(&LeaseId::new("lease-a")).await?;

    assert!(fixture.provider.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn freezing_is_unsupported_for_held_leases_and_stale_otherwise() -> TestResult {
    let fixture = Fixture::new()?;
    let lease = held(&fixture, "lease-freeze").await?;

    assert_eq!(
        AuthorityHost::freeze(fixture.provider.as_ref(), &lease).await?,
        FreezeOutcome::Unsupported
    );
    assert_eq!(
        AuthorityHost::freeze(fixture.provider.as_ref(), &unknown_lease()?).await?,
        FreezeOutcome::Stale
    );
    assert_eq!(
        AuthorityHost::freeze(fixture.provider.as_ref(), &foreign_lease()?).await?,
        FreezeOutcome::Stale
    );
    Ok(())
}

#[tokio::test]
async fn thawing_releases_held_and_completed_leases_and_defers_unknown_ones() -> TestResult {
    let fixture = Fixture::new()?;
    let lease = held(&fixture, "lease-thaw").await?;

    assert_eq!(
        AuthorityHost::thaw(fixture.provider.as_ref(), &lease).await?,
        ThawOutcome::NoLongerOwned
    );
    fixture.provider.cancel(&LeaseId::new("lease-thaw")).await?;
    assert_eq!(
        AuthorityHost::thaw(fixture.provider.as_ref(), &lease).await?,
        ThawOutcome::NoLongerOwned
    );
    assert_eq!(
        AuthorityHost::thaw(fixture.provider.as_ref(), &unknown_lease()?).await?,
        ThawOutcome::Stale
    );
    assert_eq!(
        AuthorityHost::thaw(fixture.provider.as_ref(), &foreign_lease()?).await?,
        ThawOutcome::Stale
    );
    Ok(())
}

#[tokio::test]
async fn purging_deletes_the_session_once() -> TestResult {
    let fixture = Fixture::new()?;
    let lease = held(&fixture, "lease-purge").await?;

    assert_eq!(
        AuthorityHost::purge(fixture.provider.as_ref(), &lease).await?,
        PurgeOutcome::Purged
    );
    assert_eq!(
        AuthorityHost::purge(fixture.provider.as_ref(), &lease).await?,
        PurgeOutcome::Settled
    );
    assert_eq!(fixture.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn purging_a_session_the_service_no_longer_has_is_settled() -> TestResult {
    let fixture = Fixture::new()?;
    let lease = held(&fixture, "lease-gone").await?;
    fixture.aca.script_delete(Ok(SessionDeleteOutcome::Absent));

    assert_eq!(
        AuthorityHost::purge(fixture.provider.as_ref(), &lease).await?,
        PurgeOutcome::Settled
    );
    Ok(())
}

#[tokio::test]
async fn purging_unknown_leases_is_stale() -> TestResult {
    let fixture = Fixture::new()?;

    assert_eq!(
        AuthorityHost::purge(fixture.provider.as_ref(), &unknown_lease()?).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(
        AuthorityHost::purge(fixture.provider.as_ref(), &foreign_lease()?).await?,
        PurgeOutcome::Stale
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_purge_is_reported_and_keeps_the_lease() -> TestResult {
    let fixture = Fixture::new()?;
    let lease = held(&fixture, "lease-purge-fails").await?;
    fixture.aca.script_delete(Err(500));

    let result = AuthorityHost::purge(fixture.provider.as_ref(), &lease).await;

    assert!(matches!(result, Err(AuthorityHostError::Failed(_))));
    assert_eq!(fixture.provider.active_lease_count().await, 1);
    Ok(())
}
