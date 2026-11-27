use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::{Connection, params};
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderKind, ProviderLease, SandboxId, WorkerId,
};
use trogon_sandbox_protocol::{AttemptLedger, AttemptLedgerError, AttemptPhase};

type Boxed = std::io::Error;
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn boxed(message: &str) -> Boxed {
    Boxed::other(message.to_owned())
}

fn worker() -> Result<WorkerId, Box<dyn std::error::Error>> {
    Ok(WorkerId::new("adoption-worker")?)
}

fn identity() -> Result<LeaseAttemptIdentity, Box<dyn std::error::Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new("adoption-lease"),
        provider: ProviderKind::E2b,
        target: DispatchTarget {
            worker_id: worker()?,
            attempt_id: DispatchAttemptId::new("adoption-attempt")?,
            epoch: Some(LeaseEpoch::new(7)?),
        },
    })
}

fn receipt() -> Result<ProviderLease, Boxed> {
    Ok(ProviderLease {
        sandbox_id: SandboxId::new("native-adoption-sandbox").map_err(Boxed::other)?,
        evidence_subject: "sandbox.evidence.adoption-lease".to_owned(),
        worker_id: None,
        attempt_id: None,
        epoch: Some(LeaseEpoch::new(7).map_err(Boxed::other)?),
    })
}

fn open(path: &Path) -> Result<AttemptLedger, Box<dyn std::error::Error>> {
    Ok(AttemptLedger::open(path, ProviderKind::E2b, Some(worker()?))?)
}

fn adopt(path: &Path) -> Result<AttemptLedger, Box<dyn std::error::Error>> {
    Ok(AttemptLedger::open_for_adoption(
        path,
        ProviderKind::E2b,
        Some(worker()?),
    )?)
}

fn sql(path: &Path, statements: &str) -> TestResult {
    Connection::open(path)?.execute_batch(statements)?;
    Ok(())
}

fn cleaned(response: &ProviderCleanupResponse) -> bool {
    matches!(response, ProviderCleanupResponse::Cleaned { .. })
}

fn failed(response: &ProviderCleanupResponse) -> bool {
    matches!(response, ProviderCleanupResponse::Failed { .. })
}

async fn assert_fenced(ledger: &AttemptLedger, expected: &LeaseAttemptIdentity) -> TestResult {
    let states = ledger.try_states().await?;
    assert_eq!(states.len(), 1);
    let state = states.first().ok_or("no attempt")?;
    assert_eq!(state.identity, *expected);
    assert_eq!(state.phase, AttemptPhase::CleanupRequested);
    let allocations = AtomicUsize::new(0);
    let outcome = ledger
        .dispatch(expected, || async {
            allocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(
        matches!(outcome, Err(AttemptLedgerError::Cancelled)),
        "reopening must never restart a fenced allocation"
    );
    assert_eq!(allocations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn default_reopen_still_fences_ready_receipts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let expected = identity()?;
    let native = ledger.dispatch(&expected, || async { receipt() }).await?;
    drop(ledger);

    let ledger = open(&path)?;
    assert_fenced(&ledger, &expected).await?;
    let state = ledger.try_states().await?.remove(0);
    assert!(state.allocation_started);
    assert!(!state.backend_cleaned);
    assert_eq!(state.receipt, Some(native));
    drop(ledger);

    let ledger = adopt(&path)?;
    assert_fenced(&ledger, &expected).await
}

#[tokio::test]
async fn adoption_reopen_retains_exact_ready_assignment_without_reallocation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let expected = identity()?;
    let native = ledger.dispatch(&expected, || async { receipt() }).await?;
    assert_eq!(native.worker_id.as_ref(), Some(&expected.target.worker_id));
    assert_eq!(native.attempt_id.as_ref(), Some(&expected.target.attempt_id));
    assert_eq!(native.epoch, expected.target.epoch);
    drop(ledger);

    for _ in 0..2 {
        let ledger = adopt(&path)?;
        assert_eq!(ledger.worker_id(), &expected.target.worker_id);
        let states = ledger.try_states().await?;
        assert_eq!(states.len(), 1);
        let state = states.first().ok_or("no attempt")?;
        assert_eq!(state.identity, expected);
        assert_eq!(
            state.phase,
            AttemptPhase::Ready,
            "explicit native adoption must retain the committed Ready phase"
        );
        assert!(state.allocation_started);
        assert!(!state.backend_cleaned);
        assert_eq!(state.intent, CancelIntent::Release);
        assert_eq!(state.receipt.as_ref(), Some(&native));
        let allocations = AtomicUsize::new(0);
        let replay = ledger
            .dispatch(&expected, || async {
                allocations.fetch_add(1, Ordering::SeqCst);
                receipt()
            })
            .await?;
        assert_eq!(replay, native);
        assert_eq!(allocations.load(Ordering::SeqCst), 0);
    }
    Ok(())
}

#[tokio::test]
async fn adoption_reopen_never_promotes_interrupted_allocation() -> TestResult {
    for has_receipt in [false, true] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("attempts.sqlite3");
        let ledger = open(&path)?;
        let expected = identity()?;
        let outcome = ledger
            .dispatch(&expected, || async {
                if has_receipt {
                    ledger
                        .record_backend_receipt(&expected, receipt()?)
                        .await
                        .map_err(Boxed::other)?;
                }
                Err::<ProviderLease, Boxed>(boxed("allocation reply was lost before Ready"))
            })
            .await;
        assert!(outcome.is_err());
        let states = ledger.try_states().await?;
        assert_eq!(states.first().map(|state| state.phase), Some(AttemptPhase::Starting));
        drop(ledger);

        let ledger = adopt(&path)?;
        assert_fenced(&ledger, &expected).await?;
        let state = ledger.try_states().await?.remove(0);
        assert!(state.allocation_started);
        assert!(!state.backend_cleaned);
        assert_eq!(state.receipt.is_some(), has_receipt);
    }
    Ok(())
}

#[tokio::test]
async fn adoption_reopen_preserves_explicit_release_and_purge_fences() -> TestResult {
    for intent in [CancelIntent::Release, CancelIntent::Purge] {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("attempts.sqlite3");
        let ledger = open(&path)?;
        let expected = identity()?;
        let native = ledger.dispatch(&expected, || async { receipt() }).await?;
        let request = ProviderCleanupRequest {
            identity: expected.clone(),
            intent,
        };
        let response = ledger
            .cleanup(&request, |_| async {
                Err::<(), Boxed>(boxed("native absence is not proven"))
            })
            .await;
        assert!(failed(&response));
        drop(ledger);

        let ledger = adopt(&path)?;
        assert_fenced(&ledger, &expected).await?;
        let state = ledger.try_states().await?.remove(0);
        assert_eq!(state.intent, intent);
        assert_eq!(state.receipt, Some(native));
        assert!(!state.backend_cleaned);
        let sound = AtomicUsize::new(0);
        let counter = &sound;
        let wanted = expected.clone();
        let response = ledger
            .cleanup(&request, |state| async move {
                if state.intent == intent && state.identity == wanted {
                    counter.fetch_add(1, Ordering::SeqCst);
                }
                Ok::<(), Boxed>(())
            })
            .await;
        assert!(cleaned(&response));
        assert_eq!(sound.load(Ordering::SeqCst), 1);
        assert!(ledger.try_states().await?.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn adoption_reopen_never_replays_a_cleaned_attempt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let expected = identity()?;
    ledger.dispatch(&expected, || async { receipt() }).await?;
    let request = ProviderCleanupRequest {
        identity: expected.clone(),
        intent: CancelIntent::Purge,
    };
    assert!(cleaned(
        &ledger.cleanup(&request, |_| async { Ok::<(), Boxed>(()) }).await
    ));
    drop(ledger);

    let ledger = adopt(&path)?;
    assert!(ledger.try_states().await?.is_empty());
    let allocations = AtomicUsize::new(0);
    let outcome = ledger
        .dispatch(&expected, || async {
            allocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(matches!(outcome, Err(AttemptLedgerError::Cancelled)));
    assert_eq!(allocations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn adoption_reopen_fences_a_ready_receipt_from_another_epoch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let expected = identity()?;
    let mut stale = receipt()?;
    stale.epoch = Some(LeaseEpoch::new(6)?);
    ledger.dispatch(&expected, || async { Ok::<_, Boxed>(stale) }).await?;
    let states = ledger.try_states().await?;
    assert_eq!(states.first().map(|state| state.phase), Some(AttemptPhase::Ready));
    drop(ledger);

    let ledger = adopt(&path)?;
    assert_fenced(&ledger, &expected).await
}

#[tokio::test]
async fn adoption_reopen_fences_a_ready_receipt_without_native_identity() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let expected = identity()?;
    let mut incomplete = receipt()?;
    incomplete.sandbox_id = SandboxId::new("   ").map_err(Boxed::other)?;
    ledger
        .dispatch(&expected, || async { Ok::<_, Boxed>(incomplete) })
        .await?;
    let states = ledger.try_states().await?;
    assert_eq!(states.first().map(|state| state.phase), Some(AttemptPhase::Ready));
    drop(ledger);

    let ledger = adopt(&path)?;
    assert_fenced(&ledger, &expected).await
}

#[tokio::test]
async fn adoption_reopen_fences_a_ready_record_that_is_not_coherent() -> TestResult {
    let expected = identity()?;
    let mut foreign_worker = ProviderLease {
        worker_id: Some(WorkerId::new("someone-else")?),
        attempt_id: Some(expected.target.attempt_id.clone()),
        ..receipt()?
    };
    let mut foreign_attempt = ProviderLease {
        worker_id: Some(expected.target.worker_id.clone()),
        attempt_id: Some(DispatchAttemptId::new("another-attempt")?),
        ..receipt()?
    };
    foreign_worker.epoch = expected.target.epoch;
    foreign_attempt.epoch = expected.target.epoch;
    let foreign_worker = serde_json::to_vec(&foreign_worker)?;
    let foreign_attempt = serde_json::to_vec(&foreign_attempt)?;

    let cases: Vec<(&str, Option<Vec<u8>>)> = vec![
        ("purge intent", None),
        ("already cleaned backend", None),
        ("allocation never started", None),
        ("missing receipt", None),
        ("foreign worker receipt", Some(foreign_worker)),
        ("foreign attempt receipt", Some(foreign_attempt)),
    ];
    for (name, replacement) in cases {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("attempts.sqlite3");
        let ledger = open(&path)?;
        ledger.dispatch(&expected, || async { receipt() }).await?;
        drop(ledger);
        let connection = Connection::open(&path)?;
        match name {
            "purge intent" => connection.execute("UPDATE attempts SET intent=1", [])?,
            "already cleaned backend" => connection.execute("UPDATE attempts SET backend_cleaned=1", [])?,
            "allocation never started" => connection.execute("UPDATE attempts SET allocation_started=0", [])?,
            "missing receipt" => connection.execute("UPDATE attempts SET receipt=NULL", [])?,
            _ => connection.execute("UPDATE attempts SET receipt=?1", params![replacement])?,
        };
        drop(connection);
        let ledger = adopt(&path)?;
        assert_fenced(&ledger, &expected).await?;
    }
    Ok(())
}

#[tokio::test]
async fn exact_lease_ownership_survives_every_attempt_phase_and_cleaned_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    let expected = identity()?;
    let unknown = LeaseId::new("another-adoption-lease");
    assert!(!ledger.has_lease(&expected.lease_id).await?);
    ledger
        .dispatch(&expected, || async {
            let states = ledger.try_states().await.map_err(Boxed::other)?;
            if states.first().map(|state| state.phase) != Some(AttemptPhase::Starting) {
                return Err(boxed("the attempt must be Starting while allocating"));
            }
            if !ledger.has_lease(&expected.lease_id).await.map_err(Boxed::other)?
                || ledger.has_lease(&unknown).await.map_err(Boxed::other)?
            {
                return Err(boxed("ownership must be exact while allocating"));
            }
            receipt()
        })
        .await?;
    let states = ledger.try_states().await?;
    assert_eq!(states.first().map(|state| state.phase), Some(AttemptPhase::Ready));
    assert!(ledger.has_lease(&expected.lease_id).await?);
    let request = ProviderCleanupRequest {
        identity: expected.clone(),
        intent: CancelIntent::Release,
    };
    let response = ledger
        .cleanup(&request, |_| async {
            Err::<(), Boxed>(boxed("owned native cleanup is still pending"))
        })
        .await;
    assert!(failed(&response));
    let states = ledger.try_states().await?;
    assert_eq!(
        states.first().map(|state| state.phase),
        Some(AttemptPhase::CleanupRequested)
    );
    assert!(ledger.has_lease(&expected.lease_id).await?);
    assert!(cleaned(
        &ledger.cleanup(&request, |_| async { Ok::<(), Boxed>(()) }).await
    ));
    assert!(ledger.try_states().await?.is_empty());
    assert!(
        ledger.has_lease(&expected.lease_id).await?,
        "cleaned tombstones must remain distinguishable from genuine Legacy leases"
    );
    assert!(!ledger.has_lease(&unknown).await?);
    drop(ledger);
    let ledger = adopt(&path)?;
    assert!(ledger.try_states().await?.is_empty());
    assert!(ledger.has_lease(&expected.lease_id).await?);
    assert!(!ledger.has_lease(&unknown).await?);
    let allocations = AtomicUsize::new(0);
    let outcome = ledger
        .dispatch(&expected, || async {
            allocations.fetch_add(1, Ordering::SeqCst);
            receipt()
        })
        .await;
    assert!(
        outcome.is_err(),
        "ownership lookup must not resurrect a cleaned attempt"
    );
    assert_eq!(allocations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn exact_lease_lookup_refuses_missing_or_replaced_backing_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let retained = dir.path().join("retained-attempts.sqlite3");
    let ledger = open(&path)?;
    let expected = identity()?;
    let unknown = LeaseId::new("another-adoption-lease");
    ledger.dispatch(&expected, || async { receipt() }).await?;
    assert!(ledger.has_lease(&expected.lease_id).await?);
    assert!(!ledger.has_lease(&unknown).await?);
    std::fs::rename(&path, &retained)?;
    assert!(matches!(
        ledger.has_lease(&expected.lease_id).await,
        Err(AttemptLedgerError::PathUnavailable(_))
    ));
    assert!(
        matches!(
            ledger.has_lease(&unknown).await,
            Err(AttemptLedgerError::PathUnavailable(_))
        ),
        "a missing canonical ledger cannot establish absence of assigned ownership"
    );
    std::fs::write(&path, [])?;
    assert!(matches!(
        ledger.has_lease(&expected.lease_id).await,
        Err(AttemptLedgerError::PathReplaced)
    ));
    assert!(matches!(
        ledger.has_lease(&unknown).await,
        Err(AttemptLedgerError::PathReplaced)
    ));
    Ok(())
}

#[tokio::test]
async fn an_adoption_reopen_with_an_undecodable_attempt_id_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    ledger.dispatch(&identity()?, || async { receipt() }).await?;
    drop(ledger);
    sql(&path, "UPDATE attempts SET attempt_id='bad.id'")?;
    let outcome = AttemptLedger::open_for_adoption(&path, ProviderKind::E2b, Some(worker()?));
    assert!(matches!(outcome, Err(AttemptLedgerError::InvalidAttemptId(_))));
    Ok(())
}

#[tokio::test]
async fn an_adoption_reopen_with_a_corrupt_receipt_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("attempts.sqlite3");
    let ledger = open(&path)?;
    ledger.dispatch(&identity()?, || async { receipt() }).await?;
    drop(ledger);
    sql(&path, "UPDATE attempts SET receipt=X'7B'")?;
    let outcome = AttemptLedger::open_for_adoption(&path, ProviderKind::E2b, Some(worker()?));
    assert!(matches!(outcome, Err(AttemptLedgerError::Json(_))));
    Ok(())
}
