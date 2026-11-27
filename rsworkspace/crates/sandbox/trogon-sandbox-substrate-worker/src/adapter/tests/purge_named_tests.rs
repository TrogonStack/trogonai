use trogon_sandbox_contract::WorkerId;

use super::super::tests_support::{FakeControl, TestResult};
use super::*;
use crate::ate_client::AteError;

fn ownership(error: SubstrateError, needle: &str) -> TestResult {
    match error {
        SubstrateError::Ownership(message) => {
            assert!(message.contains(needle), "{message:?} does not mention {needle:?}");
            Ok(())
        }
        other => Err(format!("expected Ownership, got {other:?}").into()),
    }
}

fn retained_entry(provider: &SubstrateProvider, lease: &ActiveLease) -> Result<ActorEntry, Box<dyn Error>> {
    let mut entry = owned_entry(provider, &LeaseId::new("01K0"), lease)?;
    entry.retained = true;
    Ok(entry)
}

#[tokio::test]
async fn purge_named_deletes_a_suspended_actor_left_by_a_release() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    let provider = provider(control.clone())?;
    let lease = named("work", true)?;
    provider.index().put(&retained_entry(&provider, &lease)?).await?;

    let outcome = provider.purge_named(&tenant_a()?, "work").await?;

    assert_eq!(outcome, PurgeOutcome::Purged);
    assert_eq!(control.delete_count(), 1);
    assert!(indexed(&provider, &lease).await?.is_none());
    assert_eq!(provider.current_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn purge_named_reports_not_found_for_an_unknown_name() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_get_with(AteError::NotFound { message: "gone".into() });
    let provider = provider(control)?;
    assert_eq!(
        provider.purge_named(&tenant_a()?, "nothing").await?,
        PurgeOutcome::NotFound
    );
    Ok(())
}

#[tokio::test]
async fn purge_named_reports_not_found_when_the_retained_actor_vanished() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    let provider = provider(control.clone())?;
    let lease = named("work", true)?;
    provider.index().put(&retained_entry(&provider, &lease)?).await?;
    control.fail_get_with(AteError::NotFound { message: "gone".into() });

    assert_eq!(
        provider.purge_named(&tenant_a()?, "work").await?,
        PurgeOutcome::NotFound
    );
    assert!(indexed(&provider, &lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn purge_named_surfaces_other_control_plane_errors() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_get_with(AteError::Unavailable { message: "down".into() });
    let provider = provider(control)?;
    let error = provider.purge_named(&tenant_a()?, "work").await.err().ok_or("purged")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Unavailable { .. })));
    Ok(())
}

#[tokio::test]
async fn purge_named_refuses_an_actor_nobody_recorded() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let error = provider.purge_named(&tenant_a()?, "work").await.err().ok_or("purged")?;
    ownership(error, "no confirmed fleet ownership")
}

#[tokio::test]
async fn purge_named_refuses_an_actor_another_worker_is_running() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease = named("work", true)?;
    let mut entry = owned_entry(&provider, &LeaseId::new("01K1"), &lease)?;
    if let Some(owner) = entry.owner.as_mut() {
        owner.worker_id = WorkerId::new("another-worker")?;
    }
    provider.index().put(&entry).await?;

    let error = provider.purge_named(&tenant_a()?, "work").await.err().ok_or("purged")?;
    ownership(error, "another active worker")
}

#[tokio::test]
async fn purge_named_refuses_a_retained_actor_that_is_not_suspended() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let lease = named("work", true)?;
    provider.index().put(&retained_entry(&provider, &lease)?).await?;

    let error = provider.purge_named(&tenant_a()?, "work").await.err().ok_or("purged")?;
    ownership(error, "not positively suspended")?;
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn purge_named_evicts_a_lease_still_claiming_the_actor() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended]));
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01K2");
    let lease = named("work", true)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    let outcome = provider.purge_named(&tenant_a()?, "work").await?;

    assert_eq!(outcome, PurgeOutcome::Purged);
    assert!(provider.active_lease(&lease_id).await.is_none());
    assert!(indexed(&provider, &lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn purge_named_rejects_a_retained_claim_without_an_owner() -> TestResult {
    let provider = provider(Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspended])))?;
    let lease = named("work", true)?;
    let mut entry = claim(&lease.atespace, &lease.actor, &LeaseId::new("01K3"));
    entry.retained = true;
    provider.index().put(&entry).await?;

    let error = provider.purge_named(&tenant_a()?, "work").await.err().ok_or("purged")?;
    ownership(error, "unconfirmed")
}
