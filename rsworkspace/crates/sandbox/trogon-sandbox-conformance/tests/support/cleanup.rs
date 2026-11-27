//! Runs the suite against a worker someone else operates, then proves every native resource it granted is gone.
//!
//! Cleanup authority follows each grant: a grant that matched its request may be checked for absence and, when the
//! worker failed to remove it, deleted as a fallback. A grant that needs reconciliation is reported, never deleted.
//! A fallback never turns a worker's failed cleanup into a pass.

use std::collections::BTreeSet;
use std::future::Future;
use std::time::{Duration, Instant, SystemTime};

use async_nats::Client;
use futures::StreamExt;
use trogon_sandbox_conformance::{GrantLedger, NativeCleanupAuthority, Target, run_suite};
use trogon_sandbox_contract::{ProviderKind, SandboxId, WorkerHeartbeat, WorkerId};
use trogon_sandbox_protocol::subjects::{worker_heartbeat, worker_heartbeat_filter};
use trogon_std::env::{ReadEnv, SystemEnv};

use super::wire::Sendable;

/// How long the whole suite may run before its allocations count as unknown.
const SUITE_BOUND: Duration = Duration::from_secs(900);

/// How long a worker has to remove a native resource after the suite cancelled it.
const ABSENCE_BOUND: Duration = Duration::from_secs(15);

/// How long a heartbeat may be old and still describe the worker.
const FRESH: Duration = Duration::from_secs(30);

/// The fresh heartbeat of the worker under test, the one `SANDBOX_CONFORMANCE_WORKER_ID` names when it is set.
pub async fn observe_worker(client: &Client, provider: ProviderKind) -> Sendable<WorkerHeartbeat> {
    let expected = match SystemEnv.var("SANDBOX_CONFORMANCE_WORKER_ID") {
        Ok(id) => Some(WorkerId::new(id)?),
        Err(_) => None,
    };
    let mut subscription = client.subscribe(worker_heartbeat_filter(provider)).await?;
    client.flush().await?;
    let heartbeat = tokio::time::timeout(FRESH, async {
        loop {
            let message = subscription.next().await.ok_or("the heartbeat subscription ended")?;
            let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
            if expected.as_ref().is_some_and(|id| id != &heartbeat.worker_id) {
                continue;
            }
            if heartbeat.provider != provider || heartbeat.profile.provider != provider {
                return Err("the worker and its profile must be of the intended provider".into());
            }
            if message.subject.as_str() != worker_heartbeat(provider, &heartbeat.worker_id) {
                return Err("the heartbeat subject and worker identity must agree".into());
            }
            let fresh = SystemTime::now()
                .duration_since(heartbeat.emitted_at)
                .is_ok_and(|age| age <= FRESH);
            if !fresh {
                return Err("the worker must advertise a fresh profile".into());
            }
            return Sendable::Ok(heartbeat);
        }
    })
    .await??;
    tracing::info!(
        provider = ?heartbeat.provider,
        worker_id = %heartbeat.worker_id.as_str(),
        dispatch_protocol = ?heartbeat.dispatch_protocol,
        "observed the worker under test"
    );
    Ok(heartbeat)
}

/// Runs the suite on `target`, then confirms every native resource its grants name is absent.
///
/// `absent` reports whether the resource is gone, and errs when that is unknown. `delete` removes one the worker
/// left behind; it is only called for a grant that matched its request.
pub async fn run_with_exact_cleanup<Check, CheckFuture, Delete, DeleteFuture>(
    target: &Target,
    ledger: &GrantLedger,
    mut absent: Check,
    mut delete: Delete,
) -> Sendable<()>
where
    Check: FnMut(SandboxId) -> CheckFuture,
    CheckFuture: Future<Output = Sendable<bool>>,
    Delete: FnMut(SandboxId) -> DeleteFuture,
    DeleteFuture: Future<Output = Sendable<()>>,
{
    let outcome = tokio::time::timeout(SUITE_BOUND, run_suite(target)).await;
    let grants = ledger.snapshot();
    let native_ids: BTreeSet<SandboxId> = grants.iter().map(|grant| grant.native.sandbox_id.clone()).collect();
    let owned: BTreeSet<SandboxId> = grants
        .iter()
        .filter(|grant| grant.native_cleanup_authority == NativeCleanupAuthority::RequestMatched)
        .map(|grant| grant.native.sandbox_id.clone())
        .collect();
    let mut errors = Vec::new();
    let mut confirmed = 0;
    let mut fallbacks = 0;
    for id in &native_ids {
        if !owned.contains(id) {
            errors.push(format!("{id}: the grant needs ownership reconciliation before cleanup"));
            continue;
        }
        let deadline = Instant::now() + ABSENCE_BOUND;
        let disposition = loop {
            match absent(id.clone()).await {
                Ok(true) => break Ok(()),
                Ok(false) if Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(250)).await,
                Ok(false) => break Err("the worker did not remove it after cancel".to_owned()),
                Err(error) => break Err(error.to_string()),
            }
        };
        let Err(error) = disposition else {
            confirmed += 1;
            continue;
        };
        errors.push(format!("{id}: {error}"));
        fallbacks += 1;
        if delete(id.clone()).await.is_err() {
            errors.push(format!("{id}: the fallback delete failed"));
            continue;
        }
        match absent(id.clone()).await {
            Ok(true) => confirmed += 1,
            _ => errors.push(format!("{id}: absence after the fallback delete is unconfirmed")),
        }
    }
    tracing::info!(
        grants = grants.len(),
        distinct = native_ids.len(),
        confirmed,
        fallbacks,
        "native cleanup"
    );
    if grants.is_empty() {
        errors.push("the suite recorded no native grants, so any lost reply is unaccounted for".to_owned());
    }
    let primary = match outcome {
        Ok(failures) if failures.is_empty() => None,
        Ok(failures) => Some(format!("the worker is not conformant: {failures:?}")),
        Err(_) => Some("the suite outran its bound, so its allocations need reconciliation".to_owned()),
    };
    match (primary, errors.is_empty()) {
        (None, true) => Ok(()),
        (None, false) => Err(format!("native cleanup failed: {errors:?}").into()),
        (Some(primary), true) => Err(primary.into()),
        (Some(primary), false) => Err(format!("{primary}; native cleanup also failed: {errors:?}").into()),
    }
}
