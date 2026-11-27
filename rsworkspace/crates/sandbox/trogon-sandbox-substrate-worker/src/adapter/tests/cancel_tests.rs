use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_protocol::AuthorityResourceId;

use super::super::tests_support::{FakeControl, TestResult};
use super::*;
use crate::ate_client::AteError;

fn control_that_suspends() -> Arc<FakeControl> {
    Arc::new(FakeControl::running().with_transitions())
}

fn ownership(error: SubstrateError, needle: &str) -> TestResult {
    match error {
        SubstrateError::Ownership(message) => {
            assert!(message.contains(needle), "{message:?} does not mention {needle:?}");
            Ok(())
        }
        other => Err(format!("expected Ownership, got {other:?}").into()),
    }
}

async fn forgotten(provider: &SubstrateProvider, lease_id: &LeaseId) -> bool {
    provider.active_lease(lease_id).await.is_none() && provider.current_lease_count().await == 0
}

#[tokio::test]
async fn cancelling_a_lease_this_worker_does_not_hold_is_a_no_op() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    Provider::cancel(&provider, &LeaseId::new("not-ours")).await?;
    assert_eq!(
        control.get_count() + control.suspend_count() + control.delete_count(),
        0
    );
    Ok(())
}

#[tokio::test]
async fn cancelling_an_anonymous_lease_suspends_then_deletes_the_actor() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J0");
    let lease = anonymous(&lease_id, false, None)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    Provider::cancel(&provider, &lease_id).await?;

    assert!(control.suspend_count() >= 1);
    assert_eq!(control.delete_count(), 1);
    assert!(indexed(&provider, &lease).await?.is_none());
    assert!(forgotten(&provider, &lease_id).await);
    Ok(())
}

#[tokio::test]
async fn cancelling_a_named_persistent_lease_keeps_the_actor_suspended() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J1");
    let lease = named("work", true)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    Provider::cancel(&provider, &lease_id).await?;

    assert_eq!(control.delete_count(), 0);
    let kept = indexed(&provider, &lease).await?.ok_or("the claim was dropped")?;
    assert!(kept.retained);
    assert!(forgotten(&provider, &lease_id).await);
    Ok(())
}

#[tokio::test]
async fn purging_a_named_persistent_lease_deletes_the_actor() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J2");
    let lease = named("work", true)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    Provider::purge(&provider, &lease_id).await?;

    assert_eq!(control.delete_count(), 1);
    assert!(indexed(&provider, &lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn cancelling_a_named_but_non_persistent_lease_deletes_the_actor() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J3");
    let lease = named("scratch", false)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    Provider::cancel(&provider, &lease_id).await?;

    assert_eq!(control.delete_count(), 1);
    assert!(indexed(&provider, &lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_retained_snapshot_that_never_lands_fails_the_cancel() -> TestResult {
    let control = control_that_suspends();
    control.set_snapshot_state(
        1,
        vec![proto::Snapshot {
            durable_snapshot: Some(proto::SnapshotStorage {
                state: proto::SnapshotStorageState::Pending as i32,
                object: None,
            }),
            ..super::super::tests_support::complete_snapshot(1)
        }],
    );
    let provider = provider(control)?;
    let lease_id = LeaseId::new("01J4");
    adopt_test(&provider, &lease_id, named("work", true)?).await?;

    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("cancelled")?;
    assert!(matches!(error, SubstrateError::ResumeTimeout { .. }));
    Ok(())
}

#[tokio::test]
async fn cancelling_a_lease_leaves_the_actor_alone_once_a_newer_lease_holds_it() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J5");
    let lease = named("work", true)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;
    provider
        .active()
        .lock()
        .await
        .insert("newer".to_string(), lease.clone());

    Provider::cancel(&provider, &lease_id).await?;

    assert_eq!(control.suspend_count() + control.delete_count(), 0);
    assert!(provider.active_lease(&lease_id).await.is_none());
    assert!(indexed(&provider, &lease).await?.is_some());
    Ok(())
}

#[tokio::test]
async fn a_suspending_actor_settles_without_an_unnecessary_suspend() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![
        proto::ActorState::Suspending,
        proto::ActorState::Suspended,
    ]));
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J6");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;

    Provider::cancel(&provider, &lease_id).await?;

    assert_eq!(control.suspend_count(), 0);
    assert_eq!(control.delete_count(), 1);
    Ok(())
}

#[tokio::test]
async fn an_actor_stuck_suspending_fails_the_cancel_and_keeps_the_receipt() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Suspending]));
    let provider = provider(control)?;
    let lease_id = LeaseId::new("01J7");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;

    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("cancelled")?;
    assert!(matches!(error, SubstrateError::ResumeTimeout { .. }));
    assert!(provider.active_lease(&lease_id).await.is_some());
    Ok(())
}

#[tokio::test]
async fn a_crashed_actor_is_deleted_instead_of_retained() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Crashed]));
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J8");
    let lease = anonymous(&lease_id, true, None)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    Provider::cancel(&provider, &lease_id).await?;

    assert_eq!(control.delete_count(), 1);
    assert!(indexed(&provider, &lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn a_crashed_named_actor_needs_an_explicit_purge() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Crashed]));
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01J9");
    let lease = named("work", true)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("cancelled")?;
    ownership(error, "explicit purge is required")?;
    assert_eq!(control.delete_count(), 0);

    Provider::purge(&provider, &lease_id).await?;
    assert_eq!(control.delete_count(), 1);
    Ok(())
}

#[tokio::test]
async fn an_actor_that_is_already_gone_just_releases_the_claim() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01JA");
    let lease = anonymous(&lease_id, false, None)?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;
    control.fail_get_with(AteError::NotFound { message: "gone".into() });

    Provider::cancel(&provider, &lease_id).await?;

    assert_eq!(control.delete_count(), 0);
    assert!(indexed(&provider, &lease).await?.is_none());
    assert!(forgotten(&provider, &lease_id).await);
    Ok(())
}

#[tokio::test]
async fn a_failing_suspend_keeps_the_lease_for_a_retry() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01JB");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;
    control.fail_get_with(AteError::Unavailable { message: "down".into() });

    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("cancelled")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Unavailable { .. })));
    assert!(provider.active_lease(&lease_id).await.is_some());
    Ok(())
}

#[tokio::test]
async fn a_claim_that_belongs_to_another_worker_is_not_cancelled() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01JC");
    let lease = anonymous(&lease_id, false, None)?;
    let mut entry = owned_entry(&provider, &lease_id, &lease)?;
    if let Some(owner) = entry.owner.as_mut() {
        owner.worker_id = WorkerId::new("another-worker")?;
    }
    provider.index().put(&entry).await?;
    provider.adopt(lease_id.clone(), lease, entry, true).await?;

    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("cancelled")?;
    ownership(error, "belongs to another assignment")?;
    assert_eq!(control.suspend_count() + control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_held_lease_without_a_receipt_cannot_be_cancelled() -> TestResult {
    let provider = provider(control_that_suspends())?;
    let lease_id = LeaseId::new("01JD");
    provider
        .active()
        .lock()
        .await
        .insert(lease_id.as_str().to_string(), anonymous(&lease_id, false, None)?);

    let error = provider
        .cancel_inner(&lease_id, CancelIntent::Release)
        .await
        .err()
        .ok_or("cancelled")?;
    ownership(error, "no allocation receipt")
}

#[tokio::test]
async fn the_authority_lists_every_prepared_lease_the_worker_holds() -> TestResult {
    let provider = provider(control_that_suspends())?;
    let ready = LeaseId::new("01JE");
    adopt_test(&provider, &ready, anonymous(&ready, false, None)?).await?;
    let preparing = LeaseId::new("01JF");
    let lease = anonymous(&preparing, false, None)?;
    let entry = owned_entry(&provider, &preparing, &lease)?;
    let _receipt = provider.adopt(preparing.clone(), lease, entry, false).await?;

    let held = AuthorityHost::held_leases(&provider).await?;

    assert_eq!(held.len(), 1);
    assert!(matches!(
        &held[0],
        HeldLease::Provider { lease_id, provider: ProviderKind::Substrate, .. } if *lease_id == ready
    ));
    Ok(())
}

async fn held_by(provider: &SubstrateProvider, lease_id: &LeaseId) -> Result<HeldLease, Box<dyn Error>> {
    AuthorityHost::held_leases(provider)
        .await?
        .into_iter()
        .find(|held| matches!(held, HeldLease::Provider { lease_id: id, .. } if id == lease_id))
        .ok_or_else(|| "the lease is not held".into())
}

#[tokio::test]
async fn the_authority_purges_a_held_lease_once_and_then_reports_it_settled() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01JG");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;
    let held = held_by(&provider, &lease_id).await?;

    assert_eq!(
        AuthorityHost::purge(&provider, &held).await?,
        AuthorityPurgeOutcome::Purged
    );
    assert_eq!(control.delete_count(), 1);
    assert_eq!(
        AuthorityHost::purge(&provider, &held).await?,
        AuthorityPurgeOutcome::Settled
    );
    Ok(())
}

#[tokio::test]
async fn the_authority_treats_another_provider_or_resource_as_stale() -> TestResult {
    let provider = provider(control_that_suspends())?;
    let lease_id = LeaseId::new("01JH");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;
    let resource_id = AuthorityResourceId::new("other-resource")?;

    let foreign = HeldLease::Provider {
        lease_id: lease_id.clone(),
        provider: ProviderKind::Subprocess,
        resource_id: resource_id.clone(),
    };
    assert_eq!(
        AuthorityHost::purge(&provider, &foreign).await?,
        AuthorityPurgeOutcome::Stale
    );

    let reassigned = HeldLease::Provider {
        lease_id,
        provider: ProviderKind::Substrate,
        resource_id,
    };
    assert_eq!(
        AuthorityHost::purge(&provider, &reassigned).await?,
        AuthorityPurgeOutcome::Stale
    );
    Ok(())
}

#[tokio::test]
async fn the_authority_reports_a_purge_that_failed() -> TestResult {
    let control = control_that_suspends();
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01JI");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;
    let held = held_by(&provider, &lease_id).await?;
    control.fail_get_with(AteError::Unavailable { message: "down".into() });

    let error = AuthorityHost::purge(&provider, &held).await.err().ok_or("purged")?;
    assert!(matches!(error, AuthorityHostError::Failed(_)));
    Ok(())
}
