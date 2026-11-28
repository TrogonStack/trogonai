//! Targeted dispatch: each attempt is kept in the dispatch ledger, so a repeat gets its first receipt, a cleanup is
//! confirmed only once the sandbox is gone, and a cleanup that failed can be retried until it is.

mod provider_support;
mod support;

use std::path::Path;
use std::sync::Arc;

use provider_support::*;
use serde_json::json;
use tempfile::TempDir;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderDispatchPayload, ProviderKind, WorkerId,
};
use trogon_sandbox_protocol::AttemptLedger;
use trogon_sandbox_vercel_worker::{VercelDispatchError, VercelWorkerProvider};

const WORKER: &str = "vercel-worker-1";
const SANDBOX: &str = "/v2/sandboxes/sbx-a";

struct Rig {
    fake: Fake,
    provider: VercelWorkerProvider,
    directory: TempDir,
}

fn ledger(directory: &Path) -> Result<Arc<AttemptLedger>, Boxed> {
    Ok(Arc::new(AttemptLedger::open(
        directory.join("ledger.sqlite3"),
        ProviderKind::Vercel,
        Some(WorkerId::new(WORKER)?),
    )?))
}

impl Rig {
    async fn start() -> Result<Self, Boxed> {
        let fake = Fake::start().await?;
        fake.created();
        fake.on("DELETE", SANDBOX, 200, json!({}));
        let directory = tempfile::tempdir()?;
        let provider = fake.provider()?;
        provider.configure_dispatch_ledger(ledger(directory.path())?)?;
        Ok(Self {
            fake,
            provider,
            directory,
        })
    }

    /// The same worker after a restart: a new provider on the ledger the last one kept.
    fn restarted(self) -> Result<Self, Boxed> {
        let provider = self.fake.provider()?;
        drop(self.provider);
        provider.configure_dispatch_ledger(ledger(self.directory.path())?)?;
        Ok(Self { provider, ..self })
    }

    fn creates(&self) -> usize {
        self.fake.count("POST", "/v2/sandboxes")
    }

    fn deletes(&self) -> usize {
        self.fake.count("DELETE", SANDBOX)
    }
}

fn target(attempt: &str) -> Result<DispatchTarget, Boxed> {
    Ok(DispatchTarget {
        worker_id: WorkerId::new(WORKER)?,
        attempt_id: DispatchAttemptId::new(attempt)?,
        epoch: Some(LeaseEpoch::new(1)?),
    })
}

fn dispatch(lease: &str, attempt: &str) -> Result<ProviderDispatchPayload, Boxed> {
    Ok(ProviderDispatchPayload {
        lease_id: lease_id(lease),
        request: request()?,
        correlation_id: None,
        resolved_drives: Vec::new(),
        target: Some(target(attempt)?),
    })
}

fn cleanup(lease: &str, attempt: &str) -> Result<ProviderCleanupRequest, Boxed> {
    Ok(ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: lease_id(lease),
            provider: ProviderKind::Vercel,
            target: target(attempt)?,
        },
        intent: CancelIntent::Release,
    })
}

fn cleaned(response: &ProviderCleanupResponse) -> bool {
    matches!(response, ProviderCleanupResponse::Cleaned { .. })
}

#[tokio::test]
async fn a_targeted_dispatch_receipt_names_the_worker_attempt_and_epoch() -> TestResult {
    let rig = Rig::start().await?;

    let receipt = within(rig.provider.lease_dispatch(&dispatch("l1", "attempt-a")?, None)).await??;

    assert_eq!(receipt.sandbox_id.as_str(), "sbx-a");
    assert_eq!(receipt.worker_id, Some(WorkerId::new(WORKER)?));
    assert_eq!(receipt.attempt_id, Some(DispatchAttemptId::new("attempt-a")?));
    assert_eq!(receipt.epoch, Some(LeaseEpoch::new(1)?));
    assert_eq!(rig.provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn a_repeated_attempt_returns_the_first_receipt_without_a_second_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    let payload = dispatch("l1", "attempt-a")?;

    let first = within(rig.provider.lease_dispatch(&payload, None)).await??;
    let second = within(rig.provider.lease_dispatch(&payload, None)).await??;

    assert_eq!(first, second);
    assert_eq!(rig.creates(), 1);
    Ok(())
}

#[tokio::test]
async fn a_cleanup_deletes_the_sandbox_once_and_a_repeat_is_confirmed_without_deleting() -> TestResult {
    let rig = Rig::start().await?;
    within(rig.provider.lease_dispatch(&dispatch("l1", "attempt-a")?, None)).await??;

    let first = within(rig.provider.cleanup_dispatch(&cleanup("l1", "attempt-a")?)).await?;
    let again = within(rig.provider.cleanup_dispatch(&cleanup("l1", "attempt-a")?)).await?;

    assert!(cleaned(&first), "{first:?}");
    assert!(cleaned(&again), "{again:?}");
    assert_eq!(rig.deletes(), 1);
    assert_eq!(rig.provider.active_lease_count().await, 0);
    Ok(())
}

/// The broker keeps a targeted lease as cancelling and retries its cleanup until the worker confirms it, so a failed
/// delete is reported, keeps the sandbox, and succeeds on the next cleanup.
#[tokio::test]
async fn a_failed_cleanup_is_reported_and_a_retry_deletes_the_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    within(rig.provider.lease_dispatch(&dispatch("l1", "attempt-a")?, None)).await??;
    rig.fake.script(
        "DELETE",
        SANDBOX,
        vec![(500, json!({"error": {"message": "boom"}})), (200, json!({}))],
    );

    let failed = within(rig.provider.cleanup_dispatch(&cleanup("l1", "attempt-a")?)).await?;
    assert!(matches!(failed, ProviderCleanupResponse::Failed { .. }), "{failed:?}");
    assert_eq!(rig.provider.active_lease_count().await, 1, "the sandbox is still held");

    let retried = within(rig.provider.cleanup_dispatch(&cleanup("l1", "attempt-a")?)).await?;
    assert!(cleaned(&retried), "{retried:?}");
    assert_eq!(rig.deletes(), 2);
    assert_eq!(rig.provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn a_dispatch_after_its_cleanup_is_refused_without_a_sandbox() -> TestResult {
    let rig = Rig::start().await?;
    let payload = dispatch("l1", "attempt-a")?;
    within(rig.provider.lease_dispatch(&payload, None)).await??;
    within(rig.provider.cleanup_dispatch(&cleanup("l1", "attempt-a")?)).await?;

    let again = within(rig.provider.lease_dispatch(&payload, None)).await?;

    assert!(matches!(again, Err(VercelDispatchError::Ledger(_))), "{again:?}");
    assert_eq!(rig.creates(), 1);
    Ok(())
}

#[tokio::test]
async fn a_cleanup_after_a_restart_deletes_the_sandbox_the_receipt_names() -> TestResult {
    let rig = Rig::start().await?;
    within(rig.provider.lease_dispatch(&dispatch("l1", "attempt-a")?, None)).await??;
    let rig = rig.restarted()?;
    assert_eq!(rig.provider.active_lease_count().await, 0);

    let response = within(rig.provider.cleanup_dispatch(&cleanup("l1", "attempt-a")?)).await?;

    assert!(cleaned(&response), "{response:?}");
    assert_eq!(rig.deletes(), 1);
    Ok(())
}

#[tokio::test]
async fn a_dispatch_for_another_worker_or_without_an_attempt_is_refused() -> TestResult {
    let rig = Rig::start().await?;
    let mut foreign = dispatch("l1", "attempt-a")?;
    if let Some(target) = foreign.target.as_mut() {
        target.worker_id = WorkerId::new("someone-else")?;
    }
    let mut untargeted = dispatch("l2", "attempt-b")?;
    untargeted.target = None;

    let foreign = within(rig.provider.lease_dispatch(&foreign, None)).await?;
    let untargeted = within(rig.provider.lease_dispatch(&untargeted, None)).await?;

    assert!(
        matches!(foreign, Err(VercelDispatchError::TargetMismatch)),
        "{foreign:?}"
    );
    assert!(
        matches!(untargeted, Err(VercelDispatchError::Untargeted)),
        "{untargeted:?}"
    );
    assert_eq!(rig.creates(), 0);
    Ok(())
}

#[tokio::test]
async fn a_worker_without_a_ledger_takes_no_targeted_dispatch_or_cleanup() -> TestResult {
    let fake = Fake::start().await?;
    let provider = fake.provider()?;

    let dispatched = within(provider.lease_dispatch(&dispatch("l1", "attempt-a")?, None)).await?;
    let cleaned_up = within(provider.cleanup_dispatch(&cleanup("l1", "attempt-a")?)).await?;

    assert!(
        matches!(dispatched, Err(VercelDispatchError::NotConfigured)),
        "{dispatched:?}"
    );
    assert!(
        matches!(cleaned_up, ProviderCleanupResponse::Failed { .. }),
        "{cleaned_up:?}"
    );
    assert!(provider.dispatch_ledger().is_none());
    Ok(())
}

#[tokio::test]
async fn a_second_ledger_is_refused() -> TestResult {
    let rig = Rig::start().await?;
    let other = tempfile::tempdir()?;

    let second = rig.provider.configure_dispatch_ledger(ledger(other.path())?);

    assert!(
        matches!(second, Err(VercelDispatchError::AlreadyConfigured)),
        "{second:?}"
    );
    Ok(())
}
