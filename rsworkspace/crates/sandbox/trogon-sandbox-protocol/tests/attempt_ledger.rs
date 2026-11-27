use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rusqlite::Connection;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderKind, ProviderLease, SandboxId, WorkerId,
};
use trogon_sandbox_protocol::{
    AttemptLedger, AttemptLedgerError, AttemptPhase, AuthorityHost, CleanupState, HeldLease, LedgerAuthorityHost,
    PurgeOutcome,
};

type Boxed = std::io::Error;
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn boxed(message: &str) -> Boxed {
    Boxed::other(message.to_owned())
}

fn identity(ledger: &AttemptLedger) -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new("lease-a"),
        provider: ProviderKind::Fly,
        target: DispatchTarget {
            epoch: None,
            worker_id: ledger.worker_id().clone(),
            attempt_id: DispatchAttemptId::new("attempt-a")?,
        },
    })
}

fn receipt() -> Result<ProviderLease, Boxed> {
    receipt_for("backend-a")
}

fn receipt_for(sandbox: &str) -> Result<ProviderLease, Boxed> {
    Ok(ProviderLease {
        epoch: None,
        sandbox_id: SandboxId::new(sandbox).map_err(Boxed::other)?,
        evidence_subject: "sandbox.evidence.lease-a".to_owned(),
        worker_id: None,
        attempt_id: None,
    })
}

fn release(identity: &LeaseAttemptIdentity) -> ProviderCleanupRequest {
    ProviderCleanupRequest {
        identity: identity.clone(),
        intent: CancelIntent::Release,
    }
}

fn purge(identity: &LeaseAttemptIdentity) -> ProviderCleanupRequest {
    ProviderCleanupRequest {
        identity: identity.clone(),
        intent: CancelIntent::Purge,
    }
}

async fn done() -> Result<(), Boxed> {
    Ok(())
}

fn open(path: &Path) -> Result<AttemptLedger, AttemptLedgerError> {
    AttemptLedger::open(path, ProviderKind::Fly, None)
}

fn sql(path: &Path, statements: &str) -> TestResult {
    let connection = Connection::open(path)?;
    connection.pragma_update(None, "ignore_check_constraints", "ON")?;
    connection.execute_batch(statements)?;
    Ok(())
}

fn failed_message(response: ProviderCleanupResponse) -> Result<String, Box<dyn std::error::Error>> {
    match response {
        ProviderCleanupResponse::Failed { message, .. } => Ok(message),
        ProviderCleanupResponse::Cleaned { .. } => Err("cleanup unexpectedly succeeded".into()),
    }
}

fn assert_cleaned(response: &ProviderCleanupResponse) -> TestResult {
    match response {
        ProviderCleanupResponse::Cleaned { .. } => Ok(()),
        ProviderCleanupResponse::Failed { message, .. } => Err(format!("cleanup failed: {message}").into()),
    }
}

async fn ready_attempt(path: &Path) -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    let ledger = open(path)?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    Ok(identity)
}

#[tokio::test]
async fn assignment_epoch_survives_reopen_for_exact_cleanup() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let mut assignment = identity(&ledger)?;
    assignment.target.epoch = Some(LeaseEpoch::new(u64::MAX)?);
    ledger.dispatch(&assignment, || async { receipt() }).await?;
    drop(ledger);
    let ledger = open(&path)?;
    let states = ledger.try_states().await?;
    assert_eq!(
        states.first().map(|state| &state.identity),
        Some(&assignment),
        "durable inventory must retain the broker-issued epoch without truncation"
    );
    let expected = assignment.clone();
    let response = ledger
        .cleanup(&purge(&assignment), |state| async move {
            let sandbox = state.receipt.map(|receipt| receipt.sandbox_id.to_string());
            if state.identity == expected && sandbox.as_deref() == Some("backend-a") {
                Ok(())
            } else {
                Err(boxed("teardown saw the wrong record"))
            }
        })
        .await;
    assert_cleaned(&response)
}

#[tokio::test]
async fn assignment_epoch_allows_the_owned_allocation_receipt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let mut assignment = identity(&ledger)?;
    assignment.target.epoch = Some(LeaseEpoch::new(7)?);
    let allocated = ledger
        .dispatch(&assignment, || async {
            ledger
                .record_backend_receipt(&assignment, receipt()?)
                .await
                .map_err(Boxed::other)?;
            receipt()
        })
        .await?;
    assert_eq!(allocated.sandbox_id.as_str(), "backend-a");
    let reallocations = AtomicUsize::new(0);
    let replay = ledger
        .dispatch(&assignment, || async {
            reallocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await?;
    assert_eq!(replay.sandbox_id, allocated.sandbox_id);
    assert_eq!(reallocations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn completed_backend_receipts_are_compacted_without_losing_purge_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    assert_cleaned(&ledger.cleanup(&release(&identity), |_| done()).await)?;
    assert!(ledger.try_states().await?.is_empty());
    let response = ledger
        .cleanup(&purge(&identity), |state| async move {
            if state.backend_cleaned && state.receipt.is_none() && state.intent == CancelIntent::Purge {
                Ok(())
            } else {
                Err(boxed("a cleaned record must be compacted and escalated"))
            }
        })
        .await;
    assert!(matches!(
        response,
        ProviderCleanupResponse::Cleaned {
            intent: CancelIntent::Purge,
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn a_repeated_cleanup_of_a_cleaned_attempt_does_not_tear_down_again() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let teardowns = AtomicUsize::new(0);
    for request in [purge(&identity), purge(&identity), release(&identity)] {
        let response = ledger
            .cleanup(&request, |_| async {
                teardowns.fetch_add(1, Ordering::SeqCst);
                Ok::<(), Boxed>(())
            })
            .await;
        assert_cleaned(&response)?;
    }
    assert_eq!(teardowns.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_cleaned_lease_identity_cannot_be_reused_by_a_different_attempt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    assert_cleaned(&ledger.cleanup(&release(&identity), |_| done()).await)?;
    let next = LeaseAttemptIdentity {
        target: DispatchTarget {
            attempt_id: DispatchAttemptId::new("attempt-b")?,
            ..identity.target.clone()
        },
        ..identity
    };
    let outcome = ledger.dispatch(&next, || async { receipt() }).await;
    assert!(
        matches!(outcome, Err(AttemptLedgerError::Sqlite(_))),
        "an old attempt must never gain authority over a later backend reusing the same lease identity"
    );
    Ok(())
}

#[tokio::test]
async fn restart_never_replays_a_ready_receipt_from_a_previous_process() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let reallocations = AtomicUsize::new(0);
    let duplicate = ledger
        .dispatch(&identity, || async {
            reallocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await?;
    assert_eq!(duplicate.sandbox_id.as_str(), "backend-a");
    drop(ledger);
    let ledger = open(&path)?;
    let outcome = ledger
        .dispatch(&identity, || async {
            reallocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(
        matches!(outcome, Err(AttemptLedgerError::Cancelled)),
        "startup recovery may have already removed the previous backend"
    );
    assert_eq!(reallocations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn purge_after_completed_release_must_run_backend_cleanup_again() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let calls = AtomicUsize::new(0);
    let released = ledger
        .cleanup(&release(&identity), |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Boxed>(())
        })
        .await;
    assert!(matches!(
        released,
        ProviderCleanupResponse::Cleaned {
            intent: CancelIntent::Release,
            ..
        }
    ));
    let purged = ledger
        .cleanup(&purge(&identity), |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Boxed>(())
        })
        .await;
    assert!(matches!(
        purged,
        ProviderCleanupResponse::Cleaned {
            intent: CancelIntent::Purge,
            ..
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "a release receipt cannot prove purge");
    Ok(())
}

#[tokio::test]
async fn purge_arriving_during_release_cannot_inherit_its_cleanup_receipt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = Arc::new(open(&dir.path().join("attempts.sqlite3"))?);
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel::<()>();
    let request = release(&identity);
    let release_task = {
        let ledger = ledger.clone();
        let request = request.clone();
        tokio::spawn(async move {
            ledger
                .cleanup(&request, |_| async move {
                    let _ = entered_tx.send(());
                    let _ = resume_rx.await;
                    Ok::<(), Boxed>(())
                })
                .await
        })
    };
    entered_rx.await?;
    let called = AtomicUsize::new(0);
    let purge_request = purge(&identity);
    let mut pending = Box::pin(ledger.cleanup(&purge_request, |_| async {
        called.fetch_add(1, Ordering::SeqCst);
        Ok::<(), Boxed>(())
    }));
    assert!(futures::poll!(pending.as_mut()).is_pending());
    resume_tx.send(()).map_err(|_| "the release teardown stopped waiting")?;
    let message = failed_message(release_task.await?)?;
    assert!(
        message.contains("escalated"),
        "release completion must leave an in-flight purge pending: {message}"
    );
    assert!(matches!(
        pending.await,
        ProviderCleanupResponse::Cleaned {
            intent: CancelIntent::Purge,
            ..
        }
    ));
    assert_eq!(called.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_second_release_waiting_on_an_inflight_release_finds_the_attempt_already_cleaned() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = Arc::new(open(&dir.path().join("attempts.sqlite3"))?);
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel::<()>();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel::<()>();
    let request = release(&identity);
    let first = {
        let ledger = ledger.clone();
        let request = request.clone();
        tokio::spawn(async move {
            ledger
                .cleanup(&request, |_| async move {
                    let _ = entered_tx.send(());
                    let _ = resume_rx.await;
                    Ok::<(), Boxed>(())
                })
                .await
        })
    };
    entered_rx.await?;
    let called = AtomicUsize::new(0);
    let mut second = Box::pin(ledger.cleanup(&request, |_| async {
        called.fetch_add(1, Ordering::SeqCst);
        Ok::<(), Boxed>(())
    }));
    assert!(futures::poll!(second.as_mut()).is_pending());
    resume_tx.send(()).map_err(|_| "the first teardown stopped waiting")?;
    assert_cleaned(&first.await?)?;
    assert_cleaned(&second.await)?;
    assert_eq!(
        called.load(Ordering::SeqCst),
        0,
        "the cleaned attempt is not torn down again"
    );
    Ok(())
}

#[tokio::test]
async fn backend_identity_is_durable_before_allocation_finishes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    let outcome = ledger
        .dispatch(&identity, || async {
            ledger
                .record_backend_receipt(&identity, receipt()?)
                .await
                .map_err(Boxed::other)?;
            Err::<ProviderLease, Boxed>(boxed("boot failed after create"))
        })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::Allocation(_))));
    drop(ledger);
    let ledger = open(&path)?;
    let response = ledger
        .cleanup(&release(&identity), |state| async move {
            match state.receipt {
                Some(receipt) if receipt.sandbox_id.as_str() == "backend-a" => Ok(()),
                _ => Err(boxed("the recorded receipt was lost")),
            }
        })
        .await;
    assert_cleaned(&response)
}

#[tokio::test]
async fn recording_a_receipt_stamps_the_worker_and_attempt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    let outcome = ledger
        .dispatch(&identity, || async {
            ledger
                .record_backend_receipt(&identity, receipt()?)
                .await
                .map_err(Boxed::other)?;
            Err::<ProviderLease, Boxed>(boxed("interrupted"))
        })
        .await;
    assert!(outcome.is_err());
    let states = ledger.try_states().await?;
    let stored = states
        .first()
        .and_then(|state| state.receipt.as_ref())
        .ok_or("the receipt was not stored")?;
    assert_eq!(stored.worker_id.as_ref(), Some(&identity.target.worker_id));
    assert_eq!(stored.attempt_id.as_ref(), Some(&identity.target.attempt_id));
    Ok(())
}

#[tokio::test]
async fn a_second_owner_of_the_same_ledger_path_is_refused_until_the_first_drops() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let contender = AttemptLedger::open(&path, ProviderKind::Fly, None);
    assert!(matches!(contender, Err(AttemptLedgerError::Locked)));
    let adopter = AttemptLedger::open_for_adoption(&path, ProviderKind::Fly, None);
    assert!(matches!(adopter, Err(AttemptLedgerError::Locked)));
    drop(ledger);
    let replacement = open(&path)?;
    drop(replacement);
    Ok(())
}

#[tokio::test]
async fn unseen_cancel_is_a_durable_fence_against_late_dispatch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    let teardowns = AtomicUsize::new(0);
    let response = ledger
        .cleanup(&purge(&identity), |_| async {
            teardowns.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Boxed>(())
        })
        .await;
    assert_cleaned(&response)?;
    assert_eq!(teardowns.load(Ordering::SeqCst), 0, "an unseen attempt has no backend");
    drop(ledger);
    let ledger = open(&path)?;
    assert_eq!(ledger.worker_id(), &identity.target.worker_id);
    let outcome = ledger
        .dispatch(&identity, || async {
            teardowns.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::Cancelled)));
    assert_eq!(teardowns.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn cleanup_fences_then_waits_for_the_inflight_backend_before_acknowledging() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = Arc::new(open(&dir.path().join("attempts.sqlite3"))?);
    let identity = identity(&ledger)?;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let (finish_tx, finish_rx) = tokio::sync::oneshot::channel::<()>();
    let allocation = {
        let ledger = ledger.clone();
        let identity = identity.clone();
        tokio::spawn(async move {
            ledger
                .dispatch(&identity, || async move {
                    let _ = started_tx.send(());
                    let _ = finish_rx.await;
                    receipt()
                })
                .await
        })
    };
    started_rx.await?;
    let cleaned = Arc::new(AtomicUsize::new(0));
    let cleanup = {
        let ledger = ledger.clone();
        let identity = identity.clone();
        let cleaned = cleaned.clone();
        tokio::spawn(async move {
            ledger
                .cleanup(&purge(&identity), |state| async move {
                    let stamped = state.receipt.is_some_and(|receipt| {
                        receipt.worker_id.as_ref() == Some(&identity.target.worker_id)
                            && receipt.attempt_id.as_ref() == Some(&identity.target.attempt_id)
                    });
                    if !stamped {
                        return Err(boxed("allocation must finish before cleanup"));
                    }
                    cleaned.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let states = ledger.try_states().await?;
            if states.first().map(|state| state.phase) == Some(AttemptPhase::CleanupRequested) {
                return Ok::<(), AttemptLedgerError>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    assert!(!cleanup.is_finished());
    assert_eq!(cleaned.load(Ordering::SeqCst), 0);
    finish_tx.send(()).map_err(|_| "the allocation stopped waiting")?;
    assert!(matches!(
        allocation.await?,
        Err(AttemptLedgerError::CancelledDuringAllocation)
    ));
    assert_cleaned(&cleanup.await?)?;
    assert_eq!(cleaned.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn ambiguous_start_survives_restart_and_failed_cleanup_never_acknowledges() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    let outcome = ledger
        .dispatch(&identity, || async {
            Err::<ProviderLease, Boxed>(boxed("remote create timed out"))
        })
        .await;
    assert!(matches!(&outcome, Err(AttemptLedgerError::Allocation(error)) if error.to_string().contains("timed out")));
    drop(ledger);
    let ledger = open(&path)?;
    let request = release(&identity);
    let response = ledger
        .cleanup(&request, |state| async move {
            assert!(state.allocation_started && state.receipt.is_none());
            Err::<(), Boxed>(boxed("remote operation is not proven quiescent"))
        })
        .await;
    let message = failed_message(response)?;
    assert!(message.contains("not proven quiescent"));
    let states = ledger.try_states().await?;
    assert_eq!(
        states.first().map(|state| state.phase),
        Some(AttemptPhase::CleanupRequested)
    );
    assert_cleaned(&ledger.cleanup(&request, |_| done()).await)
}

#[cfg(unix)]
#[tokio::test]
async fn failed_persistence_prevents_both_allocation_and_cleanup_ack() -> TestResult {
    let dir = tempfile::tempdir()?;
    let parent = dir.path().join("journal");
    let ledger = open(&parent.join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    std::fs::rename(&parent, dir.path().join("moved"))?;
    std::fs::write(&parent, b"not a directory")?;
    let allocations = AtomicUsize::new(0);
    let outcome = ledger
        .dispatch(&identity, || async {
            allocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::PathUnavailable(_))));
    let response = ledger
        .cleanup(&release(&identity), |_| async {
            allocations.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Boxed>(())
        })
        .await;
    failed_message(response)?;
    assert_eq!(allocations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn receipt_and_cleanup_are_bound_to_the_exact_attempt_identity() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    let lease = ledger.dispatch(&identity, || async { receipt() }).await?;
    assert_eq!(lease.attempt_id.as_ref(), Some(&identity.target.attempt_id));
    assert_eq!(lease.worker_id.as_ref(), Some(&identity.target.worker_id));
    let mut other = identity.clone();
    other.lease_id = LeaseId::new("lease-b");
    let teardowns = AtomicUsize::new(0);
    let response = ledger
        .cleanup(&release(&other), |_| async {
            teardowns.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Boxed>(())
        })
        .await;
    let message = failed_message(response)?;
    assert!(message.contains("reused"));
    assert_eq!(teardowns.load(Ordering::SeqCst), 0);
    assert_cleaned(&ledger.cleanup(&release(&identity), |_| done()).await)?;
    let outcome = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::Cancelled)));
    Ok(())
}

#[tokio::test]
async fn a_new_ledger_generates_a_stable_worker_identity_and_honors_a_preferred_one() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("generated.sqlite3");
    let ledger = open(&path)?;
    let generated = ledger.worker_id().clone();
    assert!(generated.as_str().starts_with("fly-"));
    drop(ledger);
    assert_eq!(open(&path)?.worker_id(), &generated);
    let preferred = WorkerId::new("worker-preferred")?;
    let named = AttemptLedger::open(
        dir.path().join("named.sqlite3"),
        ProviderKind::E2b,
        Some(preferred.clone()),
    )?;
    assert_eq!(named.worker_id(), &preferred);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn the_ledger_and_lock_files_are_private_to_the_owner() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let _ledger = open(&path)?;
    for file in [path.clone(), path.with_extension("lock")] {
        let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(file)?.permissions());
        assert_eq!(mode & 0o077, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_ledger_opens_beneath_directories_that_do_not_exist_yet() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a").join("b").join("attempts.sqlite3");
    let ledger = open(&path)?;
    assert!(ledger.try_states().await?.is_empty());
    assert!(path.is_file());
    Ok(())
}

#[tokio::test]
async fn an_existing_empty_file_is_treated_as_a_new_ledger() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    std::fs::write(&path, [])?;
    let ledger = open(&path)?;
    assert!(ledger.try_states().await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn try_states_lists_unfinished_attempts_by_attempt_id() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let base = identity(&ledger)?;
    for (attempt, lease) in [
        ("attempt-c", "lease-c"),
        ("attempt-a", "lease-a"),
        ("attempt-b", "lease-b"),
    ] {
        let identity = LeaseAttemptIdentity {
            lease_id: LeaseId::new(lease),
            target: DispatchTarget {
                attempt_id: DispatchAttemptId::new(attempt)?,
                ..base.target.clone()
            },
            ..base.clone()
        };
        ledger.dispatch(&identity, || async { receipt() }).await?;
        if attempt == "attempt-b" {
            assert_cleaned(&ledger.cleanup(&release(&identity), |_| done()).await)?;
        }
    }
    let listed: Vec<String> = ledger
        .try_states()
        .await?
        .iter()
        .map(|state| state.identity.target.attempt_id.to_string())
        .collect();
    assert_eq!(listed, ["attempt-a", "attempt-c"]);
    assert!(ledger.has_lease(&LeaseId::new("lease-b")).await?);
    Ok(())
}

#[tokio::test]
async fn a_foreign_identity_is_refused_everywhere() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let base = identity(&ledger)?;
    let mut foreign_worker = base.clone();
    foreign_worker.target.worker_id = WorkerId::new("someone-else")?;
    let mut foreign_provider = base.clone();
    foreign_provider.provider = ProviderKind::E2b;
    for foreign in [foreign_worker, foreign_provider] {
        let outcome = ledger.dispatch(&foreign, || async { receipt() }).await;
        assert!(matches!(outcome, Err(AttemptLedgerError::ForeignIdentity)));
        let recorded = ledger.record_backend_receipt(&foreign, receipt()?).await;
        assert!(matches!(recorded, Err(AttemptLedgerError::ForeignIdentity)));
        let message = failed_message(ledger.cleanup(&release(&foreign), |_| done()).await)?;
        assert!(message.contains("different worker or provider"));
    }
    Ok(())
}

#[tokio::test]
async fn an_empty_lease_identity_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let mut empty = identity(&ledger)?;
    empty.lease_id = LeaseId::new("");
    let outcome = ledger.dispatch(&empty, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::EmptyLease)));
    let recorded = ledger.record_backend_receipt(&empty, receipt()?).await;
    assert!(matches!(recorded, Err(AttemptLedgerError::EmptyLease)));
    Ok(())
}

#[tokio::test]
async fn a_receipt_needs_a_started_matching_attempt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    let unseen = ledger.record_backend_receipt(&identity, receipt()?).await;
    assert!(matches!(unseen, Err(AttemptLedgerError::ReceiptWithoutAttempt)));
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let mut other_lease = identity.clone();
    other_lease.lease_id = LeaseId::new("lease-b");
    let mismatched = ledger.record_backend_receipt(&other_lease, receipt()?).await;
    assert!(matches!(mismatched, Err(AttemptLedgerError::ReceiptMismatch)));
    assert_cleaned(&ledger.cleanup(&release(&identity), |_| done()).await)?;
    let cleaned = ledger.record_backend_receipt(&identity, receipt()?).await;
    assert!(matches!(cleaned, Err(AttemptLedgerError::ReceiptMismatch)));
    Ok(())
}

#[tokio::test]
async fn a_cancel_before_any_allocation_refuses_a_later_receipt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    assert_cleaned(&ledger.cleanup(&release(&identity), |_| done()).await)?;
    let recorded = ledger.record_backend_receipt(&identity, receipt()?).await;
    assert!(matches!(recorded, Err(AttemptLedgerError::ReceiptMismatch)));
    Ok(())
}

#[tokio::test]
async fn the_backend_resource_of_an_attempt_cannot_change() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    let outcome = ledger
        .dispatch(&identity, || async {
            ledger
                .record_backend_receipt(&identity, receipt_for("first")?)
                .await
                .map_err(Boxed::other)?;
            let swapped = ledger.record_backend_receipt(&identity, receipt_for("second")?).await;
            assert!(matches!(swapped, Err(AttemptLedgerError::BackendIdentityChanged)));
            receipt_for("third")
        })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::BackendIdentityChanged)));
    Ok(())
}

#[tokio::test]
async fn an_attempt_id_cannot_be_reused_by_another_lease() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let mut reused = identity.clone();
    reused.lease_id = LeaseId::new("lease-b");
    let outcome = ledger.dispatch(&reused, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::AttemptIdentityReused)));
    Ok(())
}

#[tokio::test]
async fn a_redispatch_while_the_first_allocation_is_unfinished_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    let outcome = ledger
        .dispatch(&identity, || async { Err::<ProviderLease, Boxed>(boxed("lost")) })
        .await;
    assert!(outcome.is_err());
    let again = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(again, Err(AttemptLedgerError::AlreadyStarted)));
    Ok(())
}

#[tokio::test]
async fn an_oversized_receipt_is_refused_by_both_write_paths() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    let oversized = || -> Result<ProviderLease, Boxed> {
        let mut lease = receipt()?;
        lease.evidence_subject = "x".repeat(70 * 1024);
        Ok(lease)
    };
    let outcome = ledger.dispatch(&identity, || async { oversized() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::ReceiptTooLarge)));
    let outcome = ledger
        .dispatch(&identity, || async { Err::<ProviderLease, Boxed>(boxed("retry")) })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::AlreadyStarted)));
    let recorded = ledger.record_backend_receipt(&identity, oversized()?).await;
    assert!(matches!(recorded, Err(AttemptLedgerError::ReceiptTooLarge)));
    Ok(())
}

#[tokio::test]
async fn an_attempt_that_vanishes_during_allocation_or_teardown_is_reported() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    let outcome = ledger
        .dispatch(&identity, || async {
            sql(&path, "DELETE FROM attempts").map_err(|error| boxed(&error.to_string()))?;
            receipt()
        })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::AttemptDisappeared)));

    let mut second = identity.clone();
    second.target.attempt_id = DispatchAttemptId::new("attempt-b")?;
    second.lease_id = LeaseId::new("lease-b");
    ledger.dispatch(&second, || async { receipt() }).await?;
    let response = ledger
        .cleanup(&release(&second), |_| async {
            sql(&path, "DELETE FROM attempts").map_err(|error| boxed(&error.to_string()))
        })
        .await;
    assert!(failed_message(response)?.contains("disappeared"));
    Ok(())
}

#[tokio::test]
async fn stored_rows_that_cannot_be_decoded_are_reported() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;

    sql(&path, "UPDATE attempts SET phase=9")?;
    let outcome = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::InvalidPhase)));
    sql(&path, "UPDATE attempts SET phase=1, intent=5")?;
    let outcome = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::InvalidIntent)));
    sql(&path, "UPDATE attempts SET intent=0, epoch='abc'")?;
    let outcome = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::InvalidEpoch)));
    sql(&path, "UPDATE attempts SET epoch='0'")?;
    let outcome = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::InvalidEpoch)));
    sql(&path, "UPDATE attempts SET epoch=NULL, receipt=X'7B'")?;
    let outcome = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::Json(_))));
    sql(&path, "UPDATE attempts SET receipt=NULL")?;
    let outcome = ledger.dispatch(&identity, || async { receipt() }).await;
    assert!(matches!(outcome, Err(AttemptLedgerError::ReadyWithoutReceipt)));
    sql(&path, "UPDATE attempts SET attempt_id='bad.id'")?;
    let outcome = ledger.try_states().await;
    assert!(matches!(outcome, Err(AttemptLedgerError::InvalidAttemptId(_))));
    Ok(())
}

#[tokio::test]
async fn a_database_of_another_format_provider_or_worker_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let worker = open(&path)?.worker_id().clone();

    let other_provider = AttemptLedger::open(&path, ProviderKind::E2b, None);
    assert!(matches!(other_provider, Err(AttemptLedgerError::ProviderMismatch)));
    let other_worker = AttemptLedger::open(&path, ProviderKind::Fly, Some(WorkerId::new("someone-else")?));
    assert!(matches!(other_worker, Err(AttemptLedgerError::WorkerIdentityMismatch)));
    let same_worker = AttemptLedger::open(&path, ProviderKind::Fly, Some(worker))?;
    drop(same_worker);

    sql(&path, "UPDATE metadata SET value='2' WHERE key='version'")?;
    assert!(matches!(open(&path), Err(AttemptLedgerError::FormatMismatch)));
    sql(&path, "UPDATE metadata SET value='1' WHERE key='version'")?;
    sql(&path, "UPDATE metadata SET value='a.b' WHERE key='worker_id'")?;
    assert!(matches!(open(&path), Err(AttemptLedgerError::InvalidWorkerId(_))));
    Ok(())
}

#[tokio::test]
async fn attempts_without_metadata_are_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    ready_attempt(&path).await?;
    sql(&path, "DELETE FROM metadata")?;
    assert!(matches!(open(&path), Err(AttemptLedgerError::MetadataMissing)));
    Ok(())
}

#[tokio::test]
async fn a_database_from_before_assignment_epochs_gains_the_epoch_column() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    sql(
        &path,
        "CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE attempts (
            attempt_id TEXT PRIMARY KEY,
            lease_id TEXT NOT NULL UNIQUE,
            phase INTEGER NOT NULL,
            intent INTEGER NOT NULL,
            allocation_started INTEGER NOT NULL,
            backend_cleaned INTEGER NOT NULL,
            receipt BLOB
        );
        INSERT INTO metadata VALUES ('version','1'),('provider','\"fly\"'),('worker_id','old-worker');
        INSERT INTO attempts VALUES ('attempt-old','lease-old',0,0,1,0,NULL);",
    )?;
    let ledger = open(&path)?;
    assert_eq!(ledger.worker_id().as_str(), "old-worker");
    let states = ledger.try_states().await?;
    let state = states.first().ok_or("the old attempt was lost")?;
    assert_eq!(state.phase, AttemptPhase::CleanupRequested);
    assert_eq!(state.identity.target.epoch, None);
    Ok(())
}

#[tokio::test]
async fn a_file_that_is_not_a_database_is_reported() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let mut bytes = b"SQLite format 3\0".to_vec();
    bytes.extend(std::iter::repeat_n(0xAB, 4096));
    std::fs::write(&path, bytes)?;
    assert!(matches!(open(&path), Err(AttemptLedgerError::Sqlite(_))));
    Ok(())
}

#[tokio::test]
async fn paths_that_cannot_hold_a_ledger_are_reported() -> TestResult {
    assert!(matches!(open(Path::new("/")), Err(AttemptLedgerError::NoParent)));
    let dir = tempfile::tempdir()?;
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"file")?;
    assert!(matches!(
        open(&blocker.join("nested").join("attempts.sqlite3")),
        Err(AttemptLedgerError::Io(_))
    ));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_directory_at_the_ledger_path_is_reported_as_an_io_failure() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    std::fs::create_dir(&path)?;
    assert!(matches!(open(&path), Err(AttemptLedgerError::Io(_))));
    Ok(())
}

#[tokio::test]
async fn the_backing_file_is_checked_before_every_operation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let retained = dir.path().join("retained.sqlite3");
    let ledger = open(&path)?;
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    assert!(ledger.has_lease(&identity.lease_id).await?);

    std::fs::rename(&path, &retained)?;
    assert!(matches!(
        ledger.has_lease(&identity.lease_id).await,
        Err(AttemptLedgerError::PathUnavailable(_))
    ));
    assert!(matches!(
        ledger.try_states().await,
        Err(AttemptLedgerError::PathUnavailable(_))
    ));
    assert!(matches!(
        ledger.record_backend_receipt(&identity, receipt()?).await,
        Err(AttemptLedgerError::PathUnavailable(_))
    ));

    std::fs::create_dir(&path)?;
    assert!(matches!(
        ledger.has_lease(&identity.lease_id).await,
        Err(AttemptLedgerError::PathNotFile)
    ));
    std::fs::remove_dir(&path)?;

    std::fs::write(&path, [])?;
    #[cfg(unix)]
    {
        assert!(matches!(
            ledger.has_lease(&identity.lease_id).await,
            Err(AttemptLedgerError::PathReplaced)
        ));
        assert!(matches!(
            ledger.try_states().await,
            Err(AttemptLedgerError::PathReplaced)
        ));
    }
    let allocations = AtomicUsize::new(0);
    let mut fresh = identity.clone();
    fresh.target.attempt_id = DispatchAttemptId::new("attempt-z")?;
    fresh.lease_id = LeaseId::new("lease-z");
    let outcome = ledger
        .dispatch(&fresh, || async {
            allocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(outcome.is_err());
    assert_eq!(allocations.load(Ordering::SeqCst), 0);
    let response = ledger.cleanup(&release(&identity), |_| done()).await;
    failed_message(response)?;
    Ok(())
}

#[tokio::test]
async fn the_authority_host_only_sees_the_exact_identity_a_ledger_holds() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = Arc::new(open(&dir.path().join("attempts.sqlite3"))?);
    let identity = identity(&ledger)?;
    ledger.dispatch(&identity, || async { receipt() }).await?;
    let host = LedgerAuthorityHost::new(ledger.clone(), |request: ProviderCleanupRequest| async move {
        Ok(ProviderCleanupResponse::Cleaned {
            identity: request.identity,
            intent: request.intent,
        })
    });

    let mut foreign_provider = identity.clone();
    foreign_provider.provider = ProviderKind::E2b;
    let mut foreign_worker = identity.clone();
    foreign_worker.target.worker_id = WorkerId::new("someone-else")?;
    let mut other_lease = identity.clone();
    other_lease.lease_id = LeaseId::new("lease-b");
    let mut unseen = identity.clone();
    unseen.target.attempt_id = DispatchAttemptId::new("attempt-unseen")?;
    for stranger in [foreign_provider, foreign_worker, other_lease, unseen] {
        let outcome = host.purge(&HeldLease::Targeted(stranger)).await?;
        assert_eq!(outcome, PurgeOutcome::Stale);
    }
    Ok(())
}

#[tokio::test]
async fn cleanup_state_round_trips_through_json_with_defaults_for_older_records() -> TestResult {
    let dir = tempfile::tempdir()?;
    let ledger = open(&dir.path().join("attempts.sqlite3"))?;
    let identity = identity(&ledger)?;
    let mut value = serde_json::json!({
        "identity": identity,
        "allocation_started": true,
        "receipt": null,
        "phase": "cleanup_requested"
    });
    let state: CleanupState = serde_json::from_value(value.clone())?;
    assert_eq!(state.intent, CancelIntent::Release);
    assert!(!state.backend_cleaned);
    assert_eq!(state.phase, AttemptPhase::CleanupRequested);
    value["phase"] = serde_json::json!("cleaned");
    let cleaned: CleanupState = serde_json::from_value(value)?;
    assert_eq!(serde_json::to_value(&cleaned)?["phase"], "cleaned");
    Ok(())
}

#[test]
fn every_ledger_error_explains_itself() -> TestResult {
    let token = WorkerId::new("a.b")
        .err()
        .ok_or("a dotted worker id must be rejected")?;
    let errors = vec![
        AttemptLedgerError::Io(std::io::Error::other("io")),
        AttemptLedgerError::Sqlite(rusqlite::Error::QueryReturnedNoRows),
        AttemptLedgerError::Json(serde_json::from_str::<u8>("x").err().ok_or("x is not a number")?),
        AttemptLedgerError::NoParent,
        AttemptLedgerError::Locked,
        AttemptLedgerError::LegacyTooLarge,
        AttemptLedgerError::LegacyInvalid(serde_json::from_str::<u8>("x").err().ok_or("x is not a number")?),
        AttemptLedgerError::LegacyIdentityMismatch,
        AttemptLedgerError::LegacyAttemptMismatch,
        AttemptLedgerError::FormatMismatch,
        AttemptLedgerError::ProviderMismatch,
        AttemptLedgerError::MetadataMissing,
        AttemptLedgerError::WorkerIdentityMismatch,
        AttemptLedgerError::InvalidWorkerId(token),
        AttemptLedgerError::InvalidAttemptId(token),
        AttemptLedgerError::InvalidPhase,
        AttemptLedgerError::InvalidIntent,
        AttemptLedgerError::InvalidEpoch,
        AttemptLedgerError::RecoveredAttemptDisappeared,
        AttemptLedgerError::ReceiptTooLarge,
        AttemptLedgerError::AttemptIdentityReused,
        AttemptLedgerError::PathUnavailable(std::io::Error::other("gone")),
        AttemptLedgerError::PathNotFile,
        AttemptLedgerError::PathReplaced,
        AttemptLedgerError::ForeignIdentity,
        AttemptLedgerError::EmptyLease,
        AttemptLedgerError::ReceiptWithoutAttempt,
        AttemptLedgerError::ReceiptMismatch,
        AttemptLedgerError::BackendIdentityChanged,
        AttemptLedgerError::ReadyWithoutReceipt,
        AttemptLedgerError::AlreadyStarted,
        AttemptLedgerError::Cancelled,
        AttemptLedgerError::AttemptDisappeared,
        AttemptLedgerError::CancelledDuringAllocation,
        AttemptLedgerError::CleanupIdentityReused,
        AttemptLedgerError::CleanupAttemptDisappeared,
        AttemptLedgerError::IntentEscalated,
        AttemptLedgerError::Allocation("allocation".into()),
        AttemptLedgerError::Teardown("teardown".into()),
    ];
    let messages: std::collections::BTreeSet<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(messages.len(), errors.len());
    assert!(messages.contains("allocation") && messages.contains("teardown"));
    assert!(format!("{:?}", errors[3]).contains("NoParent"));
    Ok(())
}

fn legacy_journal(
    identity: &LeaseAttemptIdentity,
    extra: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut journal = serde_json::json!({
        "version": 1,
        "provider": identity.provider,
        "worker_id": identity.target.worker_id,
        "attempts": {
            "attempt-a": {
                "identity": identity,
                "allocation_started": true,
                "phase": "ready",
                "receipt": receipt().ok(),
            }
        }
    });
    if let (Some(target), Some(source)) = (journal.as_object_mut(), extra.as_object()) {
        target.extend(source.clone());
    }
    Ok(journal)
}

fn legacy_identity() -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new("lease-a"),
        provider: ProviderKind::Fly,
        target: DispatchTarget {
            epoch: None,
            worker_id: WorkerId::new("worker-a")?,
            attempt_id: DispatchAttemptId::new("attempt-a")?,
        },
    })
}

fn stray_migration_files(dir: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    let mut count = 0;
    for entry in std::fs::read_dir(dir)? {
        if entry?.file_name().to_string_lossy().contains("migrating") {
            count += 1;
        }
    }
    Ok(count)
}

#[tokio::test]
async fn legacy_json_migrates_stable_identity_and_fences_prior_backend_receipts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.json");
    let identity = legacy_identity()?;
    let mut cleaned = identity.clone();
    cleaned.lease_id = LeaseId::new("lease-c");
    cleaned.target.attempt_id = DispatchAttemptId::new("attempt-c")?;
    let mut journal = legacy_journal(&identity, serde_json::json!({}))?;
    journal["attempts"]["attempt-c"] = serde_json::json!({
        "identity": cleaned,
        "allocation_started": true,
        "phase": "cleaned",
        "receipt": receipt().ok(),
    });
    std::fs::write(&path, serde_json::to_vec(&journal)?)?;

    let ledger = open(&path)?;
    assert_eq!(ledger.worker_id(), &identity.target.worker_id);
    let states = ledger.try_states().await?;
    assert_eq!(states.len(), 1);
    let state = states.first().ok_or("the migrated attempt is missing")?;
    assert_eq!(state.identity, identity);
    assert_eq!(state.phase, AttemptPhase::CleanupRequested);
    assert_eq!(state.intent, CancelIntent::Release);
    assert_eq!(
        state.receipt.as_ref().map(|receipt| receipt.sandbox_id.as_str()),
        Some("backend-a")
    );
    let allocations = AtomicUsize::new(0);
    let outcome = ledger
        .dispatch(&identity, || async {
            allocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::Cancelled)));
    assert_eq!(
        allocations.load(Ordering::SeqCst),
        0,
        "migration must fence a recovered allocation"
    );
    assert!(ledger.has_lease(&cleaned.lease_id).await?);
    let response = ledger
        .cleanup(&release(&cleaned), |_| async {
            allocations.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Boxed>(())
        })
        .await;
    assert_cleaned(&response)?;
    assert_eq!(
        allocations.load(Ordering::SeqCst),
        0,
        "a cleaned legacy attempt keeps no receipt"
    );
    let mut header = [0; 16];
    File::open(&path)?.read_exact(&mut header)?;
    assert_eq!(&header, b"SQLite format 3\0");
    assert_eq!(stray_migration_files(dir.path())?, 0);
    Ok(())
}

#[tokio::test]
async fn legacy_json_migrates_for_a_matching_preferred_worker() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.json");
    let identity = legacy_identity()?;
    let mut journal = legacy_journal(&identity, serde_json::json!({}))?;
    journal["attempts"]["attempt-a"]["receipt"]["worker_id"] = serde_json::json!("worker-a");
    journal["attempts"]["attempt-a"]["receipt"]["attempt_id"] = serde_json::json!("attempt-a");
    std::fs::write(&path, serde_json::to_vec(&journal)?)?;
    let ledger = AttemptLedger::open_for_adoption(&path, ProviderKind::Fly, Some(identity.target.worker_id.clone()))?;
    let states = ledger.try_states().await?;
    assert_eq!(states.first().map(|state| state.phase), Some(AttemptPhase::Ready));
    Ok(())
}

#[tokio::test]
async fn legacy_json_of_another_version_provider_or_worker_is_refused_and_left_intact() -> TestResult {
    let identity = legacy_identity()?;
    let cases: [(serde_json::Value, ProviderKind, Option<WorkerId>); 3] = [
        (serde_json::json!({"version": 2}), ProviderKind::Fly, None),
        (serde_json::json!({}), ProviderKind::E2b, None),
        (
            serde_json::json!({}),
            ProviderKind::Fly,
            Some(WorkerId::new("someone-else")?),
        ),
    ];
    for (extra, provider, preferred) in cases {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("attempts.json");
        let bytes = serde_json::to_vec(&legacy_journal(&identity, extra)?)?;
        std::fs::write(&path, &bytes)?;
        let outcome = AttemptLedger::open(&path, provider, preferred);
        assert!(matches!(outcome, Err(AttemptLedgerError::LegacyIdentityMismatch)));
        assert_eq!(std::fs::read(&path)?, bytes);
        assert_eq!(stray_migration_files(dir.path())?, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_legacy_attempt_that_does_not_belong_to_its_journal_is_refused_and_cleaned_up() -> TestResult {
    let identity = legacy_identity()?;
    let mut other_provider = identity.clone();
    other_provider.provider = ProviderKind::E2b;
    let mut other_worker = identity.clone();
    other_worker.target.worker_id = WorkerId::new("someone-else")?;
    let mut other_attempt = identity.clone();
    other_attempt.target.attempt_id = DispatchAttemptId::new("attempt-x")?;
    for foreign in [other_provider, other_worker, other_attempt] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("attempts.json");
        let mut journal = legacy_journal(&identity, serde_json::json!({}))?;
        journal["attempts"]["attempt-a"]["identity"] = serde_json::to_value(&foreign)?;
        let bytes = serde_json::to_vec(&journal)?;
        std::fs::write(&path, &bytes)?;
        let outcome = open(&path);
        assert!(matches!(outcome, Err(AttemptLedgerError::LegacyAttemptMismatch)));
        assert_eq!(std::fs::read(&path)?, bytes);
        assert_eq!(stray_migration_files(dir.path())?, 0);
    }
    Ok(())
}

#[tokio::test]
async fn a_legacy_journal_that_cannot_be_stored_is_refused_and_cleaned_up() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.json");
    let identity = legacy_identity()?;
    let mut journal = legacy_journal(&identity, serde_json::json!({}))?;
    journal["attempts"]["attempt-a"]["receipt"]["evidence_subject"] = serde_json::json!("x".repeat(70 * 1024));
    std::fs::write(&path, serde_json::to_vec(&journal)?)?;
    assert!(matches!(open(&path), Err(AttemptLedgerError::ReceiptTooLarge)));
    assert_eq!(stray_migration_files(dir.path())?, 0);
    Ok(())
}

#[tokio::test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_string_comparison,
        reason = "the rendered message is the contract under test; typed variants are matched separately"
    )
)]
async fn a_file_that_is_neither_a_database_nor_a_legacy_journal_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.json");
    std::fs::write(&path, b"this is not json")?;
    let outcome = open(&path);
    assert!(matches!(&outcome, Err(AttemptLedgerError::LegacyInvalid(_))));
    assert!(
        outcome
            .err()
            .is_some_and(|error| error.to_string().contains("invalid durable dispatch ledger"))
    );
    Ok(())
}

#[tokio::test]
async fn a_legacy_journal_beyond_the_migration_budget_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.json");
    let file = File::create(&path)?;
    file.set_len(64 * 1024 * 1024 + 1)?;
    drop(file);
    assert!(matches!(open(&path), Err(AttemptLedgerError::LegacyTooLarge)));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_ledger_file_is_reported_as_an_io_failure() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    std::fs::write(&path, b"data")?;
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o000))?;
    let readable_anyway = File::open(&path).is_ok();
    let outcome = open(&path);
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    if !readable_anyway {
        assert!(matches!(outcome, Err(AttemptLedgerError::Io(_))));
    }
    Ok(())
}
