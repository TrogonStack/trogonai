//! The shared conformance suite against a Vercel worker leasing from a real team.
//!
//! Skipped unless `SANDBOX_VERCEL_HOSTED_TEST=1`; see `hosted_support` for the other variables.

mod hosted_support;

use std::time::{Duration, SystemTime};

use async_nats::Client;
use futures::StreamExt;
use hosted_support::{Boxed, GATE, TestResult, api, confirm_absent, exact_id, optional, require, var, verify_running};
use trogon_sandbox_conformance::{GrantLedger, NativeCleanupAuthority, Target, run_suite};
use trogon_sandbox_contract::{ProviderKind, WorkerHeartbeat};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_vercel_worker::{NewSandbox, Resources, VercelApi, VercelProjectId};
use trogon_std::env::SystemEnv;

/// How long the suite may run end to end.
const SUITE_BOUND: Duration = Duration::from_secs(900);
const HEARTBEAT_WAIT: Duration = Duration::from_secs(30);
const MAX_HEARTBEAT_AGE: Duration = Duration::from_secs(30);
/// The control outlives the suite body, so a delete of it is never the reason a lease check passed.
const CONTROL_LIFETIME_MS: u64 = 1_800_000;

/// The first fresh heartbeat of a Vercel worker, optionally of the worker named `pinned`.
async fn observe_worker(client: &Client, pinned: Option<&str>) -> Result<WorkerHeartbeat, Boxed> {
    let mut beats = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Vercel))
        .await?;
    client.flush().await?;
    let deadline = tokio::time::Instant::now() + HEARTBEAT_WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, beats.next())
            .await
            .map_err(|_| "no worker heartbeat arrived in time")?
            .ok_or("the heartbeat subscription closed")?;
        let Ok(heartbeat) = serde_json::from_slice::<WorkerHeartbeat>(&message.payload) else {
            continue;
        };
        let pinned_elsewhere = pinned.is_some_and(|id| heartbeat.worker_id.as_str() != id);
        let stale = SystemTime::now()
            .duration_since(heartbeat.emitted_at)
            .is_ok_and(|age| age > MAX_HEARTBEAT_AGE);
        if heartbeat.provider == ProviderKind::Vercel && !pinned_elsewhere && !stale {
            return Ok(heartbeat);
        }
    }
}

/// The conformance target for the observed worker, with each capability the worker declares switched on.
fn target(client: Client, heartbeat: &WorkerHeartbeat, ledger: GrantLedger) -> Result<Target, Boxed> {
    let mut target = Target::new(client, ProviderKind::Vercel)
        .for_worker(heartbeat)?
        .signed_from_env(&SystemEnv)?
        .with_grant_ledger(ledger);
    target.image = optional("SANDBOX_CONFORMANCE_IMAGE").unwrap_or_else(|| "node24".to_owned());
    target.timeout = Duration::from_secs(120);
    let profile = &heartbeat.profile;
    target.supports_pause_resume = profile.lifecycle.supports_pause_resume.is_supported();
    target.graceful_stop = profile.isolation.supports_graceful_stop.is_supported();
    target.command_restart = profile.isolation.supports_command_restart.is_supported();
    target.named_processes = profile.isolation.supports_named_processes.is_supported();
    target.process_readiness = profile.isolation.supports_process_readiness.is_supported();
    target.checkpoint_running_unsupported = !profile.snapshot.can_checkpoint_running.is_supported();
    target.native_passthrough = profile.placement.supports_native_passthrough.is_supported();
    target.redispatch_idempotent = true;
    target.file_ops = profile.isolation.supports_file_ops.is_supported();
    target.file_search = profile.isolation.supports_file_search.is_supported();
    target.file_archive = profile.isolation.supports_file_archive.is_supported();
    target.denied_connect_fails = profile.isolation.egress.deny_all.is_supported();
    Ok(target)
}

/// An independent sandbox of the same team that stays running for the whole suite, as the positive control for the
/// absence checks.
struct Control {
    name: String,
    session: String,
}

async fn start_control(api: &VercelApi) -> Result<Control, Boxed> {
    let project = VercelProjectId::new(api.config().project_id().as_str())?;
    let mut new = NewSandbox::new(project, CONTROL_LIFETIME_MS, false);
    new.runtime = Some("node24".to_owned());
    new.resources = Some(Resources { vcpus: 1 });
    let created = api
        .create_sandbox(&new)
        .await
        .map_err(|_| "the native control was not created; a lost reply needs reconciliation")?;
    let control = Control {
        name: created.sandbox.name,
        session: created.session.id,
    };
    verify_running(api, &control.name, &control.session).await?;
    Ok(control)
}

/// Deletes the control and proves it is gone. Every failure is returned so none is lost.
async fn retire_control(api: &VercelApi, control: &Control) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(error) = api.delete_sandbox(&control.name).await {
        errors.push(format!("deleting the native control failed: {error}"));
    }
    if let Err(error) = confirm_absent(api, &control.name).await {
        errors.push(format!("the native control is not provably gone: {error}"));
    }
    errors
}

/// Every sandbox the suite leased must be gone, and each one is checked while the control still runs.
async fn settle_grants(api: &VercelApi, control: &Control, ledger: &GrantLedger) -> Vec<String> {
    let mut errors = Vec::new();
    let grants = ledger.snapshot();
    if grants.is_empty() {
        errors.push("the suite created no sandbox".to_owned());
    }
    for grant in grants {
        let id = grant.native.sandbox_id.as_str();
        if grant.native_cleanup_authority == NativeCleanupAuthority::ReconciliationRequired || !exact_id(id) {
            errors.push("a grant carries no exact cleanup authority and needs reconciliation".to_owned());
            continue;
        }
        if let Err(error) = verify_running(api, &control.name, &control.session).await {
            errors.push(format!("the positive control failed before an absence check: {error}"));
            continue;
        }
        if let Err(error) = hosted_support::settle_absence(api, id).await {
            errors.push(format!("sandbox {id} was not provably removed: {error}"));
        }
    }
    errors
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "paid hosted Vercel: set SANDBOX_VERCEL_HOSTED_TEST=1 with VERCEL_TOKEN, VERCEL_TEAM_ID, VERCEL_PROJECT_ID and a same-team worker on SANDBOX_CONFORMANCE_NATS_URL"]
async fn hosted_vercel_conforms_and_every_known_native_grant_is_absent() -> TestResult {
    require(GATE)?;
    let api = api(Duration::from_secs(10))?;
    let client = async_nats::connect(var("SANDBOX_CONFORMANCE_NATS_URL")?).await?;
    let pinned = optional("SANDBOX_CONFORMANCE_WORKER_ID");
    let heartbeat = observe_worker(&client, pinned.as_deref()).await?;
    let ledger = GrantLedger::default();
    let target = target(client, &heartbeat, ledger.clone())?;

    let control = start_control(&api).await?;
    let outcome = tokio::time::timeout(SUITE_BOUND, run_suite(&target)).await;
    let mut errors = settle_grants(&api, &control, &ledger).await;
    errors.extend(retire_control(&api, &control).await);

    let failed: Vec<&str> = outcome
        .as_ref()
        .map(|failures| failures.iter().map(|failure| failure.name).collect())
        .unwrap_or_default();
    println!(
        "{}",
        serde_json::json!({
            "receipt": "vercel_conformance",
            "grants": ledger.snapshot().len(),
            "timed_out": outcome.is_err(),
            "failed_checks": failed,
            "cleanup_errors": errors.len(),
        })
    );
    if outcome.is_err() {
        return Err("the conformance suite did not finish within its bound".into());
    }
    if !failed.is_empty() {
        return Err(format!("the worker is not conformant: {}", failed.join(", ")).into());
    }
    match errors.first() {
        Some(error) => Err(format!("cleanup was not exact: {error}").into()),
        None => Ok(()),
    }
}
