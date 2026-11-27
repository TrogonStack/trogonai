//! The diagnostics the authority guard and the verifier configuration log. This lives in its own test binary so
//! the log subscriber is global and no other test races it for tracing's per-callsite interest cache.

use std::error::Error;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderKind, WorkerAuthorityRequest,
    WorkerAuthorityResponse, WorkerId,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityHost, AuthorityHostError, AuthorityMode, AuthorityOracle,
    DispatchVerifier, FreezeOutcome, HeldLease, OracleError, PurgeOutcome, ThawOutcome,
};

type TestResult = Result<(), Box<dyn Error>>;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A broker that never answers.
struct DownBroker;

#[async_trait]
impl AuthorityOracle for DownBroker {
    async fn owned(&self, _request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        Err(OracleError::Failed("request timed out".into()))
    }
}

/// A worker holding one lease.
struct OneLeaseHost(HeldLease);

#[async_trait]
impl AuthorityHost for OneLeaseHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(vec![self.0.clone()])
    }

    async fn freeze(&self, _lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        Ok(FreezeOutcome::Frozen)
    }

    async fn thaw(&self, _lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        Ok(ThawOutcome::Thawed)
    }

    async fn purge(&self, _lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        Ok(PurgeOutcome::Purged)
    }
}

fn captured_log(captured: &Captured) -> Result<String, Box<dyn Error>> {
    Ok(String::from_utf8(
        captured.0.lock().unwrap_or_else(PoisonError::into_inner).clone(),
    )?)
}

#[tokio::test(start_paused = true)]
async fn failed_checks_and_suspect_leases_are_logged_and_an_unreadable_fence_journal_is_reported() -> TestResult {
    let captured = Captured::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish(),
    )?;

    let held = HeldLease::Targeted(LeaseAttemptIdentity {
        lease_id: LeaseId::new("l1"),
        provider: ProviderKind::Mock,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new("a1")?,
            epoch: Some(LeaseEpoch::FIRST),
        },
    });
    let mut guard = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::Enforce),
        WorkerId::new("w1")?,
        Arc::new(DownBroker),
        Arc::new(OneLeaseHost(held)),
    );
    for _ in 0..4 {
        guard.tick().await;
        tokio::time::advance(Duration::from_secs(15)).await;
    }

    let directory = tempfile::tempdir()?;
    let blocker = directory.path().join("not-a-directory");
    std::fs::write(&blocker, b"")?;
    let fence = blocker.join("fence.journal");
    DispatchVerifier::from_lookup(&|_| None, Some(fence))?;

    let log = captured_log(&captured)?;
    assert!(log.contains("lease authority check failed"), "{log}");
    assert!(log.contains("silent_secs="), "{log}");
    assert!(log.contains("worker holds a lease it may not own"), "{log}");
    assert!(log.contains("lease_id=l1"), "{log}");
    assert!(log.contains("dispatch fence journal unavailable"), "{log}");
    assert!(log.contains("not-a-directory"), "{log}");
    Ok(())
}
