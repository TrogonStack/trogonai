use trogon_sandbox_contract::WorkerId;
use trogon_std::env::ReadEnv;

use super::super::tests_support::{BrokenIndex, FakeControl, TestResult, provider_with};
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

#[tokio::test]
async fn a_lease_cannot_be_adopted_without_an_owner() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L0");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = claim(&lease.atespace, &lease.actor, &lease_id);
    let error = provider
        .adopt(lease_id, lease, entry, true)
        .await
        .err()
        .ok_or("adopted")?;
    ownership(error, "unbound")
}

#[tokio::test]
async fn a_lease_whose_resource_cannot_be_tracked_is_not_adopted() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L9");
    let lease = anonymous(&lease_id, false, None)?;
    let mut entry = owned_entry(&provider, &lease_id, &lease)?;
    entry.atespace = "tenant\0a".to_string();
    let error = provider
        .adopt(lease_id, lease, entry, true)
        .await
        .err()
        .ok_or("adopted")?;
    assert!(matches!(error, SubstrateError::Resource(_)), "{error:?}");
    Ok(())
}

#[tokio::test]
async fn the_provider_reports_the_lease_it_holds() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L1");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;
    assert_eq!(provider.current_lease_count().await, 1);
    let held = provider.active_lease(&lease_id).await.ok_or("not held")?;
    assert_eq!(held.sandbox_id(), format!("tenant-a/{}", held.actor.as_str()));
    Ok(())
}

#[tokio::test]
async fn an_abandoned_provisioning_turns_into_cleanup_that_the_authority_can_see() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L2");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;

    let receipt = provider.adopt(lease_id.clone(), lease, entry, false).await?;
    assert!(AuthorityHost::held_leases(&provider).await?.is_empty());
    drop(receipt);
    assert_eq!(AuthorityHost::held_leases(&provider).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_finished_provisioning_stays_ready_when_its_receipt_is_dropped() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L3");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;

    let receipt = provider.adopt(lease_id, lease, entry, false).await?;
    receipt.ready();
    drop(receipt);
    assert_eq!(AuthorityHost::held_leases(&provider).await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn an_agent_is_available_only_for_a_ready_lease_whose_actor_exists() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01L4");
    assert!(provider.agent_for(&lease_id).await.is_none());

    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    provider.index().put(&entry).await?;
    provider.adopt(lease_id.clone(), lease.clone(), entry, false).await?;
    assert!(provider.agent_for(&lease_id).await.is_none());

    let ready = LeaseId::new("01L5");
    adopt_test(&provider, &ready, anonymous(&ready, false, None)?).await?;
    assert!(provider.agent_for(&ready).await.is_some());

    control.fail_get_with(AteError::NotFound { message: "gone".into() });
    assert!(provider.agent_for(&ready).await.is_none());
    Ok(())
}

#[tokio::test]
async fn an_agent_is_withheld_while_the_ownership_store_is_down() -> TestResult {
    let provider = provider_with(Arc::new(FakeControl::running()), Arc::new(BrokenIndex))?;
    let lease_id = LeaseId::new("01L6");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    provider.adopt(lease_id.clone(), lease, entry, true).await?;
    assert!(provider.agent_for(&lease_id).await.is_none());
    Ok(())
}

#[tokio::test]
async fn the_single_exposed_port_resolves_to_the_router_endpoint() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L7");
    let lease = anonymous(&lease_id, false, Some(Port::new(8080)?))?;
    adopt_test(&provider, &lease_id, lease.clone()).await?;

    match provider.sandbox_domain(&lease_id, 8080).await {
        GetSandboxDomainResponse::Available {
            endpoint,
            url,
            expires_at,
            access,
        } => {
            assert_eq!(
                endpoint,
                provider
                    .config()
                    .actor_endpoint(lease.actor.as_str(), lease.atespace.as_str())
            );
            assert!(url.is_none() && expires_at.is_none() && access.is_none());
        }
        other => return Err(format!("expected Available, got {other:?}").into()),
    }
    assert!(matches!(
        provider.sandbox_domain(&lease_id, 9090).await,
        GetSandboxDomainResponse::PortNotExposed
    ));
    Ok(())
}

#[tokio::test]
async fn a_lease_without_an_exposed_port_resolves_nothing() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L8");
    adopt_test(&provider, &lease_id, anonymous(&lease_id, false, None)?).await?;
    assert!(matches!(
        provider.sandbox_domain(&lease_id, 8080).await,
        GetSandboxDomainResponse::PortNotExposed
    ));
    Ok(())
}

#[tokio::test]
async fn a_domain_lookup_for_an_unheld_lease_does_not_invent_an_endpoint() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    assert!(matches!(
        provider.sandbox_domain(&LeaseId::new("nobody"), 8080).await,
        GetSandboxDomainResponse::PortNotExposed
    ));
    Ok(())
}

#[tokio::test]
async fn a_pending_uid_confirmation_is_completed_by_its_own_worker() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01L9");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    let mut pending = entry.clone();
    if let Some(owner) = pending.owner.as_mut() {
        owner.actor_uid = None;
    }
    provider.index().put(&pending).await?;
    provider.adopt(lease_id, lease.clone(), entry.clone(), false).await?;

    let actor = provider.confirm_actor(&entry).await?.ok_or("actor absent")?;

    assert_eq!(actor_uid(&actor)?.as_str(), TEST_UID);
    assert_eq!(indexed(&provider, &lease).await?, Some(entry));
    Ok(())
}

#[tokio::test]
async fn a_claim_that_changed_is_not_confirmed() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01LA");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    let error = provider.confirm_actor(&entry).await.err().ok_or("confirmed")?;
    ownership(error, "another or unconfirmed assignment")
}

#[tokio::test]
async fn an_unowned_claim_cannot_be_confirmed() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01LB");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = claim(&lease.atespace, &lease.actor, &lease_id);
    let error = provider.confirm_actor(&entry).await.err().ok_or("confirmed")?;
    ownership(error, "unbound")
}

#[tokio::test]
async fn a_broken_ownership_store_blocks_confirmation() -> TestResult {
    let provider = provider_with(Arc::new(FakeControl::running()), Arc::new(BrokenIndex))?;
    let lease_id = LeaseId::new("01LC");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    let error = provider.confirm_actor(&entry).await.err().ok_or("confirmed")?;
    ownership(error, "storage is down")
}

#[tokio::test]
async fn an_actor_whose_uid_changed_is_not_read_as_owned() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01LD");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    control.set_uid("00000000-0000-4000-8000-000000000002");
    let error = provider.read_owned_actor(&entry).await.err().ok_or("read")?;
    ownership(error, "UID or address changed")
}

#[tokio::test]
async fn an_actor_without_a_confirmed_uid_is_not_read() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01LE");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = claim(&lease.atespace, &lease.actor, &lease_id);
    let error = provider.read_owned_actor(&entry).await.err().ok_or("read")?;
    ownership(error, "UID is unconfirmed")
}

#[tokio::test]
async fn reading_an_owned_actor_surfaces_control_plane_errors() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_get_with(AteError::Unavailable { message: "down".into() });
    let provider = provider(control)?;
    let lease_id = LeaseId::new("01LF");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    let error = provider.read_owned_actor(&entry).await.err().ok_or("read")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Unavailable { .. })));
    Ok(())
}

async fn stored(provider: &SubstrateProvider, lease_id: &LeaseId) -> Result<(ActiveLease, ActorEntry), Box<dyn Error>> {
    let lease = anonymous(lease_id, false, None)?;
    let entry = owned_entry(provider, lease_id, &lease)?;
    provider.index().put(&entry).await?;
    Ok((lease, entry))
}

#[tokio::test]
async fn deleting_an_actor_that_is_already_gone_succeeds() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let (_, entry) = stored(&provider, &LeaseId::new("01LG")).await?;
    control.fail_get_with(AteError::NotFound { message: "gone".into() });
    provider.delete_owned(&entry).await?;
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_delete_the_control_plane_rejected_is_reported() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_delete_with(AteError::Unavailable { message: "down".into() });
    let provider = provider(control)?;
    let (_, entry) = stored(&provider, &LeaseId::new("01LH")).await?;
    let error = provider.delete_owned(&entry).await.err().ok_or("deleted")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Unavailable { .. })));
    Ok(())
}

#[tokio::test]
async fn a_delete_that_leaves_the_actor_behind_is_unconfirmed() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_delete_with(AteError::NotFound {
        message: "already".into(),
    });
    let provider = provider(control)?;
    let (_, entry) = stored(&provider, &LeaseId::new("01LI")).await?;
    let error = provider.delete_owned(&entry).await.err().ok_or("deleted")?;
    ownership(error, "deletion remains unconfirmed")
}

#[tokio::test]
async fn an_exchange_with_nothing_on_either_side_is_a_no_op() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    provider.exchange_ownership(None, None).await?;
    Ok(())
}

#[tokio::test]
async fn an_exchange_against_a_changed_claim_is_refused() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01LJ");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    let error = provider
        .exchange_ownership(Some(&entry), None)
        .await
        .err()
        .ok_or("exchanged")?;
    ownership(error, "claim changed during cleanup")
}

#[tokio::test]
async fn an_exchange_fails_when_the_store_is_down() -> TestResult {
    let provider = provider_with(Arc::new(FakeControl::running()), Arc::new(BrokenIndex))?;
    let lease_id = LeaseId::new("01LK");
    let lease = anonymous(&lease_id, false, None)?;
    let entry = owned_entry(&provider, &lease_id, &lease)?;
    let error = provider
        .exchange_ownership(None, Some(&entry))
        .await
        .err()
        .ok_or("exchanged")?;
    ownership(error, "storage is down")
}

#[tokio::test]
async fn a_suspend_that_raced_the_actor_is_not_an_error() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    let actor = object_ref("tenant-a", "sbx-01");
    provider.suspend_ignoring_races(&actor).await?;
    control.fail_suspend_with(AteError::FailedPrecondition { message: "busy".into() });
    provider.suspend_ignoring_races(&actor).await?;
    control.fail_suspend_with(AteError::NotFound { message: "gone".into() });
    provider.suspend_ignoring_races(&actor).await?;
    control.fail_suspend_with(AteError::Unavailable { message: "down".into() });
    let error = provider.suspend_ignoring_races(&actor).await.err().ok_or("suspended")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Unavailable { .. })));
    Ok(())
}

#[tokio::test]
async fn the_reaper_leaves_what_it_does_not_own_alone() -> TestResult {
    let control = Arc::new(FakeControl::running().with_transitions());
    let provider = provider(control.clone())?;
    let lease_id = LeaseId::new("01LL");
    let (lease, entry) = stored(&provider, &lease_id).await?;

    let mut retained = entry.clone();
    retained.retained = true;
    assert!(!provider.reap_owned(&retained).await?);

    let mut foreign = entry.clone();
    if let Some(owner) = foreign.owner.as_mut() {
        owner.worker_id = WorkerId::new("another-worker")?;
    }
    assert!(!provider.reap_owned(&foreign).await?);

    let mut unconfirmed = entry.clone();
    if let Some(owner) = unconfirmed.owner.as_mut() {
        owner.actor_uid = None;
    }
    assert!(!provider.reap_owned(&unconfirmed).await?);

    provider.adopt(lease_id, lease, entry.clone(), true).await?;
    assert!(!provider.reap_owned(&entry).await?);
    assert_eq!(control.delete_count(), 0);
    Ok(())
}

#[tokio::test]
async fn the_reaper_deletes_an_orphaned_actor_and_its_claim() -> TestResult {
    let control = Arc::new(FakeControl::running().with_transitions());
    let provider = provider(control.clone())?;
    let (lease, entry) = stored(&provider, &LeaseId::new("01LM")).await?;

    assert!(provider.reap_owned(&entry).await?);

    assert_eq!(control.delete_count(), 1);
    assert!(indexed(&provider, &lease).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn the_reaper_rejects_an_identity_it_cannot_name() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let lease_id = LeaseId::new("01LN");
    let (_, entry) = stored(&provider, &lease_id).await?;

    let mut stranger = entry.clone();
    stranger.actor = "bogus".to_string();
    let error = provider.reap_owned(&stranger).await.err().ok_or("reaped")?;
    ownership(error, "unrecognized actor identity")?;

    let mut elsewhere = entry;
    elsewhere.atespace = "Not A Label".to_string();
    let error = provider.reap_owned(&elsewhere).await.err().ok_or("reaped")?;
    assert!(matches!(error, SubstrateError::Naming(_)), "{error:?}");
    Ok(())
}

#[test]
fn the_agent_environment_is_empty() {
    assert!(EmptyEnv.var("ANYTHING").is_err());
}
