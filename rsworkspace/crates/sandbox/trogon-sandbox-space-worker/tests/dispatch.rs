mod support;

use std::sync::Arc;
use std::time::Duration;

use support::{Rig, TestResult, api, cleanup_request, config, dispatch, identity, lease_request};
use trogon_sandbox_contract::{CancelIntent, LeaseId, ProviderCleanupResponse, ProviderLease};
use trogon_sandbox_protocol::AttemptPhase;
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_space_worker::{SpaceProvider, SpaceProviderError};

fn failed(response: &ProviderCleanupResponse) -> String {
    match response {
        ProviderCleanupResponse::Failed { message, .. } => message.clone(),
        ProviderCleanupResponse::Cleaned { .. } => String::new(),
    }
}

#[tokio::test]
async fn a_targeted_dispatch_tags_the_sandbox_and_fills_the_ledger() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease_targeted("lease-1", "attempt-1").await?;
    let body = rig.space.state().create_bodies.first().cloned().ok_or("no create")?;
    assert_eq!(body["metadata"]["sandbox_dispatch_attempt"], "attempt-1");
    assert_eq!(body["metadata"]["sandbox_owner"], rig.worker());
    let states = rig.provider.dispatch_ledger().try_states().await?;
    assert_eq!(states.len(), 1);
    assert!(states[0].receipt.is_some());
    Ok(())
}

#[tokio::test]
async fn release_then_purge_cleans_once_and_discards_later() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease_targeted("lease-1", "attempt-1").await?;
    let release = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(matches!(
        release,
        ProviderCleanupResponse::Cleaned {
            intent: CancelIntent::Release,
            ..
        }
    ));
    assert!(rig.space.state().sandboxes.is_empty());
    let deletes = rig
        .space
        .state()
        .log
        .iter()
        .filter(|line| line.starts_with("DELETE"))
        .count();
    let again = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(matches!(again, ProviderCleanupResponse::Cleaned { .. }));
    let purge = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "purge")?)
        .await;
    assert!(matches!(
        purge,
        ProviderCleanupResponse::Cleaned {
            intent: CancelIntent::Purge,
            ..
        }
    ));
    let after = rig
        .space
        .state()
        .log
        .iter()
        .filter(|line| line.starts_with("DELETE"))
        .count();
    assert_eq!(deletes, after);
    assert!(
        !rig.provider
            .holds_lease(&"lease-1".parse::<String>().map(trogon_sandbox_contract::LeaseId::new)?)
            .await
    );
    Ok(())
}

#[tokio::test]
async fn cleanup_finds_a_sandbox_whose_create_reply_was_lost() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_without_id = true;
    let worker = rig.worker();
    let payload = dispatch("lease-1", Some((&worker, "attempt-1")))?;
    assert!(rig.provider.lease_dispatch(&payload).await.is_err());
    rig.space.state().sandboxes.clear();
    rig.plant("sb-lost", "lease-1", "attempt-1");
    let response = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(matches!(response, ProviderCleanupResponse::Cleaned { .. }));
    assert!(rig.space.state().sandboxes.is_empty());
    Ok(())
}

#[tokio::test]
async fn cleanup_cannot_tell_which_sandbox_when_there_is_none_or_many() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_status = Some(400);
    let worker = rig.worker();
    let payload = dispatch("lease-1", Some((&worker, "attempt-1")))?;
    assert!(rig.provider.lease_dispatch(&payload).await.is_err());
    let none = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(failed(&none).contains("ambiguous"));
    rig.plant("sb-a", "lease-1", "attempt-1");
    rig.plant("sb-b", "lease-1", "attempt-1");
    let many = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(failed(&many).contains("ambiguous"));
    Ok(())
}

#[tokio::test]
async fn cleanup_refuses_a_sandbox_the_lease_does_not_own() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_status = Some(400);
    let worker = rig.worker();
    let payload = dispatch("lease-1", Some((&worker, "attempt-1")))?;
    assert!(rig.provider.lease_dispatch(&payload).await.is_err());
    rig.space.state().create_status = None;
    rig.lease("lease-1").await?;
    let receipt: ProviderLease = serde_json::from_value(serde_json::json!({
        "sandbox_id": "sb-other", "evidence_subject": "sandbox.evidence.lease-1",
    }))?;
    rig.provider
        .dispatch_ledger()
        .record_backend_receipt(&identity(&rig, "lease-1", "attempt-1")?, receipt)
        .await?;
    let response = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(failed(&response).contains("different backend attempt"));
    Ok(())
}

#[tokio::test]
async fn cleanup_refuses_a_receipt_that_is_not_a_safe_id() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_status = Some(400);
    let worker = rig.worker();
    let payload = dispatch("lease-1", Some((&worker, "attempt-1")))?;
    assert!(rig.provider.lease_dispatch(&payload).await.is_err());
    let receipt: ProviderLease = serde_json::from_value(serde_json::json!({
        "sandbox_id": "bad id/x", "evidence_subject": "sandbox.evidence.lease-1",
    }))?;
    rig.provider
        .dispatch_ledger()
        .record_backend_receipt(&identity(&rig, "lease-1", "attempt-1")?, receipt)
        .await?;
    let response = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(!failed(&response).is_empty());
    Ok(())
}

#[tokio::test]
async fn cleanup_reports_a_sandbox_that_will_not_go() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease_targeted("lease-1", "attempt-1").await?;
    rig.space.state().delete_status = Some(400);
    let response = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(!failed(&response).is_empty());
    Ok(())
}

#[tokio::test]
async fn a_cancel_that_races_a_create_leaves_nothing_behind() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_delay = Duration::from_millis(200);
    let worker = rig.worker();
    let payload = dispatch("lease-1", Some((&worker, "attempt-1")))?;
    let provider = Arc::clone(&rig.provider);
    let create = tokio::spawn(async move { provider.lease_dispatch(&payload).await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let response = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(create.await?.is_err());
    assert!(rig.space.state().sandboxes.is_empty());
    assert!(matches!(response, ProviderCleanupResponse::Cleaned { .. }));
    Ok(())
}

#[tokio::test]
async fn a_cancel_that_cannot_stop_the_create_in_time_says_so() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_delay = Duration::from_millis(1500);
    let lease = LeaseId::new("lease-1");
    let request: trogon_sandbox_contract::LeaseRequest = serde_json::from_value(lease_request("seed"))?;
    let provider = Arc::clone(&rig.provider);
    let id = lease.clone();
    let create = tokio::spawn(async move { provider.lease(&request, &id).await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let error = rig.provider.cancel(&lease).await.err().ok_or("cancelled")?;
    assert!(!format!("{error}").is_empty());
    let _ = create.await?;
    Ok(())
}

#[tokio::test]
async fn cancelling_a_lease_deletes_its_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    let lease = rig.lease("lease-1").await?;
    rig.provider.cancel(&lease).await?;
    assert!(rig.space.state().sandboxes.is_empty());
    rig.provider.cancel(&lease).await?;
    Ok(())
}

#[tokio::test]
async fn a_cancel_after_the_sandbox_exists_deletes_it() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().operation_pending_polls = 20;
    let worker = rig.worker();
    let payload = dispatch("lease-1", Some((&worker, "attempt-1")))?;
    let provider = Arc::clone(&rig.provider);
    let create = tokio::spawn(async move { provider.lease_dispatch(&payload).await });
    tokio::time::sleep(Duration::from_millis(60)).await;
    let response = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    let created = create.await?;
    assert!(created.is_err());
    assert!(rig.space.state().sandboxes.is_empty());
    let _ = response;
    Ok(())
}

#[tokio::test]
async fn lists_every_page_when_recovering() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    rig.lease("lease-2").await?;
    rig.space.state().list_in_pages = true;
    assert_eq!(rig.provider.recover().await?, 2);
    assert!(rig.space.state().sandboxes.is_empty());
    assert_eq!(rig.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn ignores_sandboxes_it_does_not_own_or_that_are_deleted() -> TestResult {
    let rig = Rig::start().await?;
    rig.plant("sb-mine", "lease-1", "attempt-1");
    rig.plant("sb-gone", "lease-2", "attempt-2");
    {
        let mut state = rig.space.state();
        state.sandboxes.get_mut("sb-gone").ok_or("planted")?.state = "deleted".into();
        state.sandboxes.insert(
            "sb-foreign".into(),
            support::Sandbox {
                state: "running".into(),
                metadata: serde_json::json!({"sandbox_owner": "someone-else"}),
                failure_code: None,
            },
        );
    }
    assert_eq!(rig.provider.recover().await?, 1);
    assert!(rig.space.state().sandboxes.contains_key("sb-foreign"));
    Ok(())
}

#[tokio::test]
async fn recovery_records_the_receipt_of_a_sandbox_from_an_earlier_run() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_without_id = true;
    let worker = rig.worker();
    let payload = dispatch("lease-1", Some((&worker, "attempt-1")))?;
    assert!(rig.provider.lease_dispatch(&payload).await.is_err());
    {
        let mut state = rig.space.state();
        state.sandboxes.clear();
        state.create_without_id = false;
    }
    rig.plant("sb-old", "lease-1", "attempt-1");
    assert_eq!(rig.provider.recover().await?, 1);
    let states = rig.provider.dispatch_ledger().try_states().await?;
    assert!(
        states
            .iter()
            .any(|state| state.receipt.is_some() && state.phase != AttemptPhase::Cleaned)
    );
    Ok(())
}

#[tokio::test]
async fn recovery_fails_when_spaces_listing_fails() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().list_status = Some(400);
    assert!(rig.provider.recover().await.is_err());
    assert!(rig.provider.shutdown().await.is_err());
    Ok(())
}

#[tokio::test]
async fn shutdown_releases_everything() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    rig.provider.shutdown().await?;
    assert!(rig.space.state().sandboxes.is_empty());
    Ok(())
}

#[tokio::test]
async fn refuses_volumes_and_unserved_requests() -> TestResult {
    let rig = Rig::start().await?;
    let mut payload = dispatch("lease-1", None)?;
    payload.request.ttl_seconds = 0;
    let error = rig.provider.lease_dispatch(&payload).await.err().ok_or("leased")?;
    assert!(matches!(error, SpaceProviderError::Violation(_)));
    Ok(())
}

#[tokio::test]
async fn one_worker_holds_the_state_directory_at_a_time() -> TestResult {
    let rig = Rig::start().await?;
    let uri = rig.space.uri();
    let second = SpaceProvider::new(api(&uri, &uri)?, config(rig.dir.path(), 1, Duration::from_secs(1))?);
    assert!(second.is_err());
    Ok(())
}

#[tokio::test]
async fn cancelling_a_create_that_has_a_sandbox_deletes_it() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().operation_pending_polls = 1000;
    let lease = LeaseId::new("lease-1");
    let request: trogon_sandbox_contract::LeaseRequest = serde_json::from_value(lease_request("seed"))?;
    let provider = Arc::clone(&rig.provider);
    let id = lease.clone();
    let create = tokio::spawn(async move { provider.lease(&request, &id).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    rig.provider.cancel(&lease).await?;
    assert!(create.await?.is_err());
    assert!(rig.space.state().sandboxes.is_empty());
    assert_eq!(rig.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_repeated_targeted_dispatch_returns_the_same_receipt_without_a_second_create() -> TestResult {
    let rig = Rig::start().await?;
    let payload = dispatch("lease-1", Some((&rig.worker(), "attempt-1")))?;

    let first = rig.provider.lease_dispatch(&payload).await?;
    let second = rig.provider.lease_dispatch(&payload).await?;

    assert_eq!(first, second);
    assert_eq!(rig.space.state().create_bodies.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_targeted_dispatch_after_its_cleanup_is_refused_without_creating() -> TestResult {
    let rig = Rig::start().await?;
    let payload = dispatch("lease-1", Some((&rig.worker(), "attempt-1")))?;
    rig.provider.lease_dispatch(&payload).await?;
    let release = rig
        .provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;
    assert!(
        matches!(release, ProviderCleanupResponse::Cleaned { .. }),
        "{release:?}"
    );

    let again = rig.provider.lease_dispatch(&payload).await;

    assert!(
        matches!(
            again,
            Err(SpaceProviderError::Ledger(
                trogon_sandbox_protocol::AttemptLedgerError::Cancelled
            ))
        ),
        "{again:?}"
    );
    assert_eq!(rig.space.state().create_bodies.len(), 1);
    assert!(rig.space.state().sandboxes.is_empty());
    Ok(())
}

/// The ledger gives a lease id one attempt per worker, so an old attempt can never gain authority over a later
/// sandbox for the same lease.
#[tokio::test]
async fn a_new_attempt_for_a_released_lease_is_refused_without_creating() -> TestResult {
    let rig = Rig::start().await?;
    let worker = rig.worker();
    rig.provider
        .lease_dispatch(&dispatch("lease-1", Some((&worker, "attempt-1")))?)
        .await?;
    rig.provider
        .cleanup_dispatch(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)
        .await;

    let second = rig
        .provider
        .lease_dispatch(&dispatch("lease-1", Some((&worker, "attempt-2")))?)
        .await;

    assert!(matches!(second, Err(SpaceProviderError::Ledger(_))), "{second:?}");
    assert_eq!(rig.space.state().create_bodies.len(), 1);
    Ok(())
}
