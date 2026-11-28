#[allow(dead_code, reason = "shared support; this binary uses only the server")]
mod nats_support;
mod snapshot_support;
mod support;

use std::time::{Duration, Instant};

use serde_json::json;
use snapshot_support::{
    LEASE, NAME, SNAPSHOT_ID, Served, capture_route, fake, quick, snapshot, snapshot_of, snapshot_route,
    targeted_cleanup,
};
use support::{Reply, SANDBOX, TestResult, lease_id};
use trogon_sandbox_contract::{ProviderCleanupResponse, ProviderKind, SnapshotExportError, SnapshotExportOutcome};
use trogon_sandbox_daytona_worker::SnapshotWait;
use trogon_sandbox_protocol::{AuthorityHost, AuthorityResourceId, HeldLease, Provider, PurgeOutcome};

/// Daytona reports no snapshot of this name until the capture is asked for, then `state`.
fn script_snapshot(fake: &support::Fake, name: &str, state: &str) {
    fake.script(
        "GET",
        &snapshot_route(name),
        vec![
            Reply::json(404, json!({"message": "not created"})),
            Reply::json(200, snapshot(name, state)),
        ],
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_native_snapshot_waits_until_the_snapshot_is_active() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "active");
    let mut served = Served::start(fake, quick(5_000), true).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 1);
    assert_eq!(served.fake.count("GET", &snapshot_route(NAME)), 2);
    let SnapshotExportOutcome::Native(receipt) = outcome else {
        return Err(format!("expected a native receipt, got {outcome:?}").into());
    };
    assert_eq!(receipt.reference.provider, ProviderKind::Daytona);
    assert_eq!(receipt.reference.id.as_str(), NAME);
    assert_eq!(receipt.bytes, Some(2));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_that_fails_to_build_is_permanent() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "error");
    let mut served = Served::start(fake, quick(5_000), true).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 1);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Permanent { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_that_never_activates_is_transient() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "pending");
    let mut served = Served::start(fake, quick(30), true).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 1);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_configured_wait_bounds_the_native_snapshot() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "pending");
    let mut served = Served::start(fake, quick(60), true).await?;
    let started = Instant::now();
    let outcome = served.capture(NAME).await?;
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(served.fake.count("GET", &snapshot_route(NAME)) > 2);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_capture_request_is_transient() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "pending");
    fake.json("POST", &capture_route(), 500, json!({"message": "create failed"}));
    let mut served = Served::start(fake, SnapshotWait::default(), true).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 1);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_without_custody_refuses_a_native_snapshot() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "active");
    let mut served = Served::start(fake, quick(5_000), false).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 0);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_that_already_exists_refuses_before_any_capture() -> TestResult {
    let fake = fake().await?;
    fake.json("GET", &snapshot_route(NAME), 200, snapshot(NAME, "active"));
    let mut served = Served::start(fake, quick(5_000), true).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 0);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capture_answer_about_another_sandbox_is_not_accepted() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "active");
    fake.json(
        "POST",
        &capture_route(),
        200,
        json!({"id": "sbx-other", "organizationId": "org-1"}),
    );
    let mut served = Served::start(fake, quick(5_000), true).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("GET", &snapshot_route(NAME)), 1);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_taken_from_another_sandbox_is_never_accepted() -> TestResult {
    let fake = fake().await?;
    fake.script(
        "GET",
        &snapshot_route(NAME),
        vec![
            Reply::json(404, json!({"message": "not created"})),
            Reply::json(200, snapshot_of(NAME, "active", "sbx-other", "org-1")),
        ],
    );
    let mut served = Served::start(fake, quick(5_000), true).await?;
    let outcome = served.capture(NAME).await?;
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_lease_stays_held_after_a_snapshot() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "active");
    let mut served = Served::start(fake, quick(5_000), true).await?;
    served.capture(NAME).await?;
    assert_eq!(served.provider.active_lease_count().await, 1);
    assert_eq!(served.fake.count("DELETE", &format!("/sandbox/{SANDBOX}")), 0);
    served.stop().await
}

fn sandbox_route() -> String {
    format!("/sandbox/{SANDBOX}")
}

fn id_route() -> String {
    snapshot_route(SNAPSHOT_ID)
}

fn unheld() -> Result<HeldLease, support::Boxed> {
    Ok(HeldLease::Provider {
        lease_id: lease_id("elsewhere"),
        provider: ProviderKind::Daytona,
        resource_id: AuthorityResourceId::new(SANDBOX)?,
    })
}

/// A capture Daytona built badly: bound to an id, never handed to the caller.
async fn failed_capture(wait: SnapshotWait) -> Result<Served, support::Boxed> {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "error");
    let served = Served::start(fake, wait, true).await?;
    served.capture(NAME).await?;
    Ok(served)
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_deletes_unkept_snapshots_before_the_sandbox() -> TestResult {
    let mut served = failed_capture(quick(5_000)).await?;
    let fake = &served.fake;
    fake.script(
        "GET",
        &id_route(),
        vec![
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(404, json!({"message": "gone"})),
        ],
    );
    fake.script(
        "DELETE",
        &id_route(),
        vec![
            Reply::json(500, json!({"message": "busy"})),
            Reply::json(200, json!({})),
        ],
    );
    assert!(served.provider.cancel(&lease_id(LEASE)).await.is_err());
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
    served.provider.cancel(&lease_id(LEASE)).await?;
    assert_eq!(served.fake.count("DELETE", &id_route()), 2);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 1);
    served.stop().await
}

/// A capture Daytona built badly, taken from a lease the worker holds through a targeted dispatch.
async fn failed_targeted_capture(ledger: &std::path::Path) -> Result<Served, support::Boxed> {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "error");
    let served = Served::start_targeted(fake, quick(5_000), ledger).await?;
    served.capture(NAME).await?;
    Ok(served)
}

/// Daytona reports the bad capture through a failed delete and the retry's look, then no longer.
fn script_capture_until_deleted(fake: &support::Fake) {
    fake.script(
        "GET",
        &id_route(),
        vec![
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(404, json!({"message": "gone"})),
        ],
    );
}

/// A targeted cleanup deletes the sandbox the way a cancel does: the snapshots it still owns first, so a snapshot
/// that cannot be deleted yet fails the cleanup, keeps the sandbox, and the broker's retry finishes the job.
#[tokio::test(flavor = "multi_thread")]
async fn a_targeted_cleanup_deletes_unkept_snapshots_before_the_sandbox() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut served = failed_targeted_capture(&directory.path().join("ledger.sqlite3")).await?;
    script_capture_until_deleted(&served.fake);
    served.fake.script(
        "DELETE",
        &id_route(),
        vec![
            Reply::json(500, json!({"message": "busy"})),
            Reply::json(200, json!({})),
        ],
    );

    let failed = served.provider.cleanup_dispatch(&targeted_cleanup()?).await;
    assert!(matches!(failed, ProviderCleanupResponse::Failed { .. }), "{failed:?}");
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
    assert_eq!(served.provider.active_lease_count().await, 1);

    let cleaned = served.provider.cleanup_dispatch(&targeted_cleanup()?).await;
    assert!(
        matches!(cleaned, ProviderCleanupResponse::Cleaned { .. }),
        "{cleaned:?}"
    );
    assert_eq!(served.fake.count("DELETE", &id_route()), 2);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 1);
    assert_eq!(served.provider.active_lease_count().await, 0);
    served.stop().await
}

/// After a restart the worker has neither the lease nor the snapshot in memory; the ledger's receipt names the
/// sandbox and custody names its snapshots, so the cleanup still deletes both. The restarted provider shares the
/// ledger the first one kept, as a restarted worker reopens its file.
#[tokio::test(flavor = "multi_thread")]
async fn a_targeted_cleanup_after_a_restart_still_deletes_the_snapshots_and_the_sandbox() -> TestResult {
    let directory = tempfile::tempdir()?;
    let mut served = failed_targeted_capture(&directory.path().join("ledger.sqlite3")).await?;
    served.fake.script(
        "GET",
        &id_route(),
        vec![
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(404, json!({"message": "gone"})),
        ],
    );
    served.fake.json("DELETE", &id_route(), 200, json!({}));
    let ledger = served.provider.dispatch_ledger().cloned().ok_or("no ledger")?;
    let restarted = served.restarted_provider()?;
    restarted.configure_dispatch_ledger(ledger)?;
    assert_eq!(restarted.active_lease_count().await, 0);

    let cleaned = restarted.cleanup_dispatch(&targeted_cleanup()?).await;

    assert!(
        matches!(cleaned, ProviderCleanupResponse::Cleaned { .. }),
        "{cleaned:?}"
    );
    assert_eq!(served.fake.count("DELETE", &id_route()), 1);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 1);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_leaves_a_published_snapshot_alone() -> TestResult {
    let fake = fake().await?;
    script_snapshot(&fake, NAME, "active");
    let mut served = Served::start(fake, quick(5_000), true).await?;
    served.capture(NAME).await?;
    served.provider.cancel(&lease_id(LEASE)).await?;
    assert_eq!(served.fake.count("DELETE", &id_route()), 0);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 1);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capture_that_never_showed_up_blocks_cancel_until_it_is_visible() -> TestResult {
    let fake = fake().await?;
    fake.json("GET", &snapshot_route(NAME), 404, json!({"message": "not yet"}));
    let mut served = Served::start(fake, quick(30), true).await?;
    let outcome = served.capture(NAME).await?;
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    assert_eq!(served.fake.count("POST", &capture_route()), 1);
    assert!(served.provider.cancel(&lease_id(LEASE)).await.is_err());
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
    assert_eq!(served.fake.count("DELETE", &id_route()), 0);

    served
        .fake
        .json("GET", &snapshot_route(NAME), 200, snapshot(NAME, "pending"));
    served.fake.script(
        "GET",
        &id_route(),
        vec![
            Reply::json(200, snapshot(NAME, "pending")),
            Reply::json(404, json!({"message": "gone"})),
        ],
    );
    served.fake.json("DELETE", &id_route(), 200, json!({}));
    served.provider.cancel(&lease_id(LEASE)).await?;
    assert_eq!(served.fake.count("DELETE", &id_route()), 1);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 1);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_snapshot_from_another_sandbox_is_never_deleted() -> TestResult {
    let fake = fake().await?;
    fake.json("GET", &snapshot_route(NAME), 404, json!({"message": "not yet"}));
    let mut served = Served::start(fake, quick(30), true).await?;
    served.capture(NAME).await?;
    served.fake.json(
        "GET",
        &snapshot_route(NAME),
        200,
        snapshot_of(NAME, "pending", "sbx-other", "org-1"),
    );
    assert!(served.provider.cancel(&lease_id(LEASE)).await.is_err());
    assert_eq!(served.fake.count("DELETE", &id_route()), 0);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn purge_settles_only_a_source_custody_has_an_identity_for() -> TestResult {
    let fake = fake().await?;
    let mut served = Served::start(fake, quick(5_000), true).await?;
    served
        .fake
        .json("GET", &sandbox_route(), 404, json!({"message": "gone"}));
    assert_eq!(
        AuthorityHost::purge(&*served.provider, &unheld()?).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn purge_with_an_identity_needs_the_source_to_be_confirmed_absent() -> TestResult {
    let mut served = failed_capture(quick(5_000)).await?;
    served.provider.cancel(&lease_id(LEASE)).await.ok();
    assert_eq!(
        AuthorityHost::purge(&*served.provider, &unheld()?).await?,
        PurgeOutcome::Stale
    );
    served
        .fake
        .json("GET", &sandbox_route(), 500, json!({"message": "down"}));
    assert!(AuthorityHost::purge(&*served.provider, &unheld()?).await.is_err());
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn purge_of_an_absent_source_cleans_its_snapshots() -> TestResult {
    let mut served = failed_capture(quick(5_000)).await?;
    served
        .fake
        .json("GET", &sandbox_route(), 404, json!({"message": "gone"}));
    served.fake.script(
        "GET",
        &id_route(),
        vec![
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(404, json!({"message": "gone"})),
        ],
    );
    served.fake.json("DELETE", &id_route(), 200, json!({}));
    assert_eq!(
        AuthorityHost::purge(&*served.provider, &unheld()?).await?,
        PurgeOutcome::Settled
    );
    assert_eq!(served.fake.count("DELETE", &id_route()), 1);
    served.stop().await
}

const PUBLISHED: &str = "trogon-published";
const PUBLISHED_ID: &str = "snap-2";

fn published_snapshot(state: &str) -> serde_json::Value {
    let mut value = snapshot(PUBLISHED, state);
    value["id"] = json!(PUBLISHED_ID);
    value
}

fn organization_route(id: &str) -> String {
    format!("/organizations/{id}")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_whose_absence_cannot_be_confirmed_refuses_before_any_capture() -> TestResult {
    let fake = fake().await?;
    fake.json("GET", &snapshot_route(NAME), 500, json!({"message": "down"}));
    let mut served = Served::start(fake, quick(5_000), true).await?;
    let outcome = served.capture(NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 0);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_of_a_source_without_captures_persists_a_close_fence() -> TestResult {
    let fake = fake().await?;
    let mut served = Served::start(fake, quick(5_000), true).await?;
    assert!(served.custody_record().await?.is_none());
    served.provider.cancel(&lease_id(LEASE)).await?;
    let record = served.custody_record().await?.ok_or("cancel wrote no close fence")?;
    assert_eq!(record["closed"], json!(true));
    assert_eq!(record["captures"], json!({}));

    served
        .provider
        .lease(&support::request()?, &lease_id("lease-2"))
        .await?;
    let outcome = served.capture_for("lease-2", NAME).await?;
    assert_eq!(served.fake.count("POST", &capture_route()), 0);
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Transient { .. })
    ));
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn cleanup_of_an_absent_source_needs_no_organization_metadata() -> TestResult {
    let fake = fake().await?;
    let mut served = Served::start(fake, quick(5_000), true).await?;
    served
        .fake
        .json("GET", &sandbox_route(), 404, json!({"message": "gone"}));
    served.provider.cancel(&lease_id(LEASE)).await?;
    let record = served.custody_record().await?.ok_or("cancel wrote no close fence")?;
    assert_eq!(record["closed"], json!(true));
    assert!(record["organization"].is_null());
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 1);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capture_whose_creation_was_never_acknowledged_never_authorizes_cleanup() -> TestResult {
    let fake = fake().await?;
    fake.script(
        "GET",
        &snapshot_route(NAME),
        vec![
            Reply::json(404, json!({"message": "none"})),
            Reply::json(200, snapshot(NAME, "pending")),
        ],
    );
    fake.json("POST", &capture_route(), 500, json!({"message": "unknown outcome"}));
    let mut served = Served::start(fake, quick(5_000), true).await?;
    served.capture(NAME).await?;
    assert!(served.provider.cancel(&lease_id(LEASE)).await.is_err());
    assert_eq!(served.fake.count("DELETE", &id_route()), 0);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_late_snapshot_with_a_drifting_name_or_account_is_never_deleted() -> TestResult {
    let mut other_name = snapshot(NAME, "pending");
    other_name["name"] = json!("someone-elses");
    let mut shared = snapshot(NAME, "pending");
    shared["general"] = json!(true);
    for late in [
        snapshot_of(NAME, "pending", support::SANDBOX, "org-2"),
        other_name,
        shared,
    ] {
        let fake = fake().await?;
        fake.json("GET", &snapshot_route(NAME), 404, json!({"message": "not yet"}));
        let mut served = Served::start(fake, quick(30), true).await?;
        served.capture(NAME).await?;
        served.fake.json("GET", &snapshot_route(NAME), 200, late);
        assert!(served.provider.cancel(&lease_id(LEASE)).await.is_err());
        assert_eq!(served.fake.count("DELETE", &id_route()), 0);
        assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
        served.stop().await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_that_moved_account_cannot_have_its_snapshots_settled() -> TestResult {
    let mut served = failed_capture(quick(5_000)).await?;
    served.fake.json(
        "GET",
        &sandbox_route(),
        200,
        json!({"id": support::SANDBOX, "state": "started", "target": "us", "organizationId": "org-2"}),
    );
    assert!(served.provider.cancel(&lease_id(LEASE)).await.is_err());
    assert_eq!(served.fake.count("DELETE", &id_route()), 0);
    assert_eq!(served.fake.count("DELETE", &sandbox_route()), 0);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hidden_snapshot_in_another_account_context_is_not_absence() -> TestResult {
    let mut served = failed_capture(quick(5_000)).await?;
    served
        .fake
        .json("GET", &sandbox_route(), 404, json!({"message": "gone"}));
    served
        .fake
        .json("GET", &id_route(), 404, json!({"message": "not visible here"}));
    served
        .fake
        .json("GET", &organization_route("org-1"), 200, json!({"id": "org-2"}));
    assert!(AuthorityHost::purge(&*served.provider, &unheld()?).await.is_err());
    assert_eq!(served.fake.count("DELETE", &id_route()), 0);

    served
        .fake
        .json("GET", &organization_route("org-1"), 404, json!({"message": "unknown"}));
    assert!(AuthorityHost::purge(&*served.provider, &unheld()?).await.is_err());
    assert_eq!(served.fake.count("DELETE", &id_route()), 0);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restarted_worker_drains_an_absent_sources_unpublished_captures_only() -> TestResult {
    let fake = fake().await?;
    fake.script(
        "GET",
        &snapshot_route(PUBLISHED),
        vec![
            Reply::json(404, json!({"message": "not created"})),
            Reply::json(200, published_snapshot("active")),
        ],
    );
    script_snapshot(&fake, NAME, "error");
    let mut served = Served::start(fake, quick(5_000), true).await?;
    assert!(matches!(
        served.capture(PUBLISHED).await?,
        SnapshotExportOutcome::Native(_)
    ));
    served.capture(NAME).await?;

    let fake = &served.fake;
    fake.json("GET", &sandbox_route(), 404, json!({"message": "gone"}));
    fake.script(
        "GET",
        &id_route(),
        vec![
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(200, snapshot(NAME, "error")),
            Reply::json(404, json!({"message": "gone"})),
        ],
    );
    fake.json("DELETE", &id_route(), 200, json!({}));
    let restarted = served.restarted_provider()?;
    assert_eq!(
        AuthorityHost::purge(&restarted, &unheld()?).await?,
        PurgeOutcome::Settled
    );
    assert_eq!(served.fake.count("DELETE", &id_route()), 1);
    assert_eq!(served.fake.count("DELETE", &snapshot_route(PUBLISHED_ID)), 0);
    served.stop().await
}
