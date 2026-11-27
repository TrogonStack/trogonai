mod support;

use serde_json::json;
use support::*;
use trogon_sandbox_contract::{LeaseError, ProviderKind, SandboxFreezeResponse};
use trogon_sandbox_protocol::{AuthorityHost, AuthorityResourceId, HeldLease, Provider, PurgeOutcome};

fn held(lease: &str, provider: ProviderKind, resource: &str) -> Result<HeldLease, Boxed> {
    Ok(HeldLease::Provider {
        lease_id: lease_id(lease),
        provider,
        resource_id: AuthorityResourceId::new(resource)?,
    })
}

fn sandbox_route() -> String {
    format!("/sandbox/{SANDBOX}")
}

#[tokio::test]
async fn cancel_deletes_the_sandbox_and_forgets_the_lease() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    provider.cancel(&lease_id("l1")).await?;
    assert_eq!(fake.count("DELETE", &sandbox_route()), 1);
    assert_eq!(provider.active_lease_count().await, 0);
    provider.cancel(&lease_id("l1")).await?;
    provider.cancel(&lease_id("never")).await?;
    assert_eq!(fake.count("DELETE", &sandbox_route()), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_delete_keeps_the_lease_so_cancel_can_retry() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    fake.json("DELETE", &sandbox_route(), 500, json!({}));
    assert!(provider.cancel(&lease_id("l1")).await.is_err());
    assert_eq!(provider.active_lease_count().await, 1);
    fake.json("DELETE", &sandbox_route(), 200, json!({}));
    provider.cancel(&lease_id("l1")).await?;
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn held_leases_name_each_sandbox() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    assert!(provider.held_leases().await?.is_empty());
    provider.lease(&request()?, &lease_id("l1")).await?;
    let held = provider.held_leases().await?;
    assert!(matches!(
        held.as_slice(),
        [HeldLease::Provider { lease_id: id, provider: ProviderKind::Daytona, resource_id }]
            if id == &lease_id("l1") && resource_id.as_str() == SANDBOX
    ));
    Ok(())
}

#[tokio::test]
async fn purging_the_exact_held_sandbox_deletes_it() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    let outcome = AuthorityHost::purge(&provider, &held("l1", ProviderKind::Daytona, SANDBOX)?).await?;
    assert_eq!(outcome, PurgeOutcome::Purged);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn purging_a_sandbox_that_is_already_gone_is_settled() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    fake.json("DELETE", &sandbox_route(), 404, json!({}));
    let outcome = AuthorityHost::purge(&provider, &held("l1", ProviderKind::Daytona, SANDBOX)?).await?;
    assert_eq!(outcome, PurgeOutcome::Settled);
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_purge_delete_is_an_error_and_keeps_the_lease() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    fake.json("DELETE", &sandbox_route(), 500, json!({}));
    assert!(
        AuthorityHost::purge(&provider, &held("l1", ProviderKind::Daytona, SANDBOX)?)
            .await
            .is_err()
    );
    assert_eq!(provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn purging_what_this_worker_does_not_hold_looks_it_up() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    let other = held("elsewhere", ProviderKind::Daytona, SANDBOX)?;
    assert_eq!(AuthorityHost::purge(&provider, &other).await?, PurgeOutcome::Stale);
    fake.json("GET", &sandbox_route(), 404, json!({}));
    assert_eq!(AuthorityHost::purge(&provider, &other).await?, PurgeOutcome::Settled);
    fake.json("GET", &sandbox_route(), 500, json!({}));
    assert!(AuthorityHost::purge(&provider, &other).await.is_err());
    assert_eq!(fake.count("DELETE", &sandbox_route()), 0);
    Ok(())
}

#[tokio::test]
async fn a_lease_held_under_another_sandbox_is_not_purged_by_id() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    fake.json(
        "GET",
        "/sandbox/sbx-old",
        200,
        json!({"id": "sbx-old", "state": "started"}),
    );
    let outcome = AuthorityHost::purge(&provider, &held("l1", ProviderKind::Daytona, "sbx-old")?).await?;
    assert_eq!(outcome, PurgeOutcome::Stale);
    assert_eq!(provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn another_provider_kind_is_stale() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    let outcome = AuthorityHost::purge(&provider, &held("l1", ProviderKind::Vercel, SANDBOX)?).await?;
    assert_eq!(outcome, PurgeOutcome::Stale);
    assert_eq!(fake.total(), 0);
    Ok(())
}

#[tokio::test]
async fn pausing_stops_the_sandbox_and_resuming_starts_it() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    provider.lease(&request()?, &lease_id("l1")).await?;
    let get = sandbox_route();
    fake.script(
        "GET",
        &get,
        vec![
            Reply::json(200, record("started")),
            Reply::json(200, record("stopping")),
            Reply::json(200, record("stopped")),
        ],
    );
    fake.json("POST", &format!("{get}/stop"), 200, json!({}));
    fake.json("POST", &format!("{get}/start"), 200, json!({}));
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), true).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(fake.count("POST", &format!("{get}/stop")), 1);
    // Already stopped: nothing more to ask.
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), true).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(fake.count("POST", &format!("{get}/stop")), 1);
    fake.script(
        "GET",
        &get,
        vec![Reply::json(200, record("stopped")), Reply::json(200, record("started"))],
    );
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), false).await,
        SandboxFreezeResponse::Done
    ));
    assert_eq!(fake.count("POST", &format!("{get}/start")), 1);
    Ok(())
}

#[tokio::test]
async fn pausing_an_unknown_lease_or_a_broken_sandbox_is_denied() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("nope"), true).await,
        SandboxFreezeResponse::Denied(LeaseError::LeaseNotFound { .. })
    ));
    provider.lease(&request()?, &lease_id("l1")).await?;
    fake.json("GET", &sandbox_route(), 200, record("error"));
    assert!(matches!(
        provider.set_paused_for_lease(&lease_id("l1"), true).await,
        SandboxFreezeResponse::Denied(LeaseError::Backend { .. })
    ));
    Ok(())
}
