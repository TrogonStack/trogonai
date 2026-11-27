use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, OwnedLease, ProviderKind,
    WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId,
};
use trogon_sandbox_protocol::{
    AttemptLedgerError, AuthorityConfig, AuthorityGuard, AuthorityHost, AuthorityHostError, AuthorityMode,
    AuthorityOracle, AuthorityStats, FreezeOutcome, HeldLease, MAX_LEASES_PER_REQUEST, OracleError, PurgeOutcome,
    ThawOutcome,
};

type TestResult = Result<(), Box<dyn Error>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn identity(attempt: &str, epoch: u64) -> Result<LeaseAttemptIdentity, Box<dyn Error>> {
    Ok(LeaseAttemptIdentity {
        lease_id: LeaseId::new("reused"),
        provider: ProviderKind::Fly,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new(attempt)?,
            epoch: Some(LeaseEpoch::new(epoch)?),
        },
    })
}

/// A broker that owns every asked lease at epoch 2 once told to.
#[derive(Default)]
struct Broker {
    owns: AtomicBool,
}

#[async_trait]
impl AuthorityOracle for Broker {
    async fn owned(&self, request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        let epoch = LeaseEpoch::new(2).map_err(|error| OracleError::Failed(Box::new(error)))?;
        let owned = if self.owns.load(Ordering::SeqCst) {
            request
                .lease_ids
                .iter()
                .map(|id| OwnedLease {
                    lease_id: id.clone(),
                    epoch: Some(epoch),
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(WorkerAuthorityResponse {
            owned,
            not_assigned: Vec::new(),
        })
    }
}

/// A host that holds one exact assignment and can be made to fail in each way the guard must survive.
struct AssignmentHost {
    held: Mutex<Option<LeaseAttemptIdentity>>,
    events: Mutex<Vec<String>>,
    paused: AtomicBool,
    fail_freeze: AtomicBool,
    lose_freeze_reply: AtomicBool,
    stale_freeze: AtomicBool,
    fail_purge: AtomicBool,
    settle_purge: AtomicBool,
    fail_thaw: AtomicBool,
    stale_thaw: AtomicBool,
    purge_attempts: AtomicU32,
    thaw_attempts: AtomicU32,
}

impl AssignmentHost {
    fn new(attempt: &str, epoch: u64) -> Result<Arc<Self>, Box<dyn Error>> {
        Ok(Arc::new(Self {
            held: Mutex::new(Some(identity(attempt, epoch)?)),
            events: Mutex::new(Vec::new()),
            paused: AtomicBool::new(false),
            fail_freeze: AtomicBool::new(false),
            lose_freeze_reply: AtomicBool::new(false),
            stale_freeze: AtomicBool::new(false),
            fail_purge: AtomicBool::new(false),
            settle_purge: AtomicBool::new(false),
            fail_thaw: AtomicBool::new(false),
            stale_thaw: AtomicBool::new(false),
            purge_attempts: AtomicU32::new(0),
            thaw_attempts: AtomicU32::new(0),
        }))
    }

    fn record(&self, action: &str, identity: &LeaseAttemptIdentity) {
        let epoch = identity.target.epoch.map_or(0, LeaseEpoch::get);
        locked(&self.events).push(format!("{action}:{}:{epoch}", identity.target.attempt_id.as_str()));
    }

    fn events(&self) -> Vec<String> {
        locked(&self.events).clone()
    }

    fn is_held(&self) -> bool {
        locked(&self.held).is_some()
    }
}

fn matching(held: &Option<LeaseAttemptIdentity>, expected: &HeldLease) -> Option<LeaseAttemptIdentity> {
    held.as_ref()
        .filter(|identity| HeldLease::Targeted((*identity).clone()) == *expected)
        .cloned()
}

#[async_trait]
impl AuthorityHost for AssignmentHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(locked(&self.held).iter().cloned().map(HeldLease::Targeted).collect())
    }

    async fn freeze(&self, expected: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        let held = locked(&self.held);
        let Some(identity) = matching(&held, expected) else {
            return Ok(FreezeOutcome::Stale);
        };
        if self.stale_freeze.swap(false, Ordering::SeqCst) {
            return Ok(FreezeOutcome::Stale);
        }
        if self.fail_freeze.swap(false, Ordering::SeqCst) {
            return Err(AuthorityHostError::Failed("pause failed before effect".into()));
        }
        self.record("freeze", &identity);
        self.paused.store(true, Ordering::SeqCst);
        if self.lose_freeze_reply.swap(false, Ordering::SeqCst) {
            return Err(AuthorityHostError::Failed("pause reply was lost after effect".into()));
        }
        Ok(FreezeOutcome::Frozen)
    }

    async fn thaw(&self, expected: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        self.thaw_attempts.fetch_add(1, Ordering::SeqCst);
        if self.stale_thaw.swap(false, Ordering::SeqCst) {
            return Ok(ThawOutcome::Stale);
        }
        if self.fail_thaw.swap(false, Ordering::SeqCst) {
            return Err(AuthorityHostError::Failed("owned thaw failed".into()));
        }
        let held = locked(&self.held);
        let Some(identity) = matching(&held, expected) else {
            return Ok(ThawOutcome::Stale);
        };
        self.record("thaw", &identity);
        self.paused.store(false, Ordering::SeqCst);
        Ok(ThawOutcome::Thawed)
    }

    async fn purge(&self, expected: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        self.purge_attempts.fetch_add(1, Ordering::SeqCst);
        if self.fail_purge.swap(false, Ordering::SeqCst) {
            return Err(AuthorityHostError::CleanupFailed {
                message: "owned purge failed".to_owned(),
            });
        }
        let mut held = locked(&self.held);
        if let Some(identity) = matching(&held, expected) {
            self.record("purge", &identity);
            *held = None;
            if self.settle_purge.swap(false, Ordering::SeqCst) {
                Ok(PurgeOutcome::Settled)
            } else {
                Ok(PurgeOutcome::Purged)
            }
        } else {
            Ok(PurgeOutcome::Stale)
        }
    }
}

/// Replaces the held assignment while the broker is being asked about the old one.
struct ReassigningOracle {
    host: Arc<AssignmentHost>,
    replacement: LeaseAttemptIdentity,
}

#[async_trait]
impl AuthorityOracle for ReassigningOracle {
    async fn owned(&self, request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        *locked(&self.host.held) = Some(self.replacement.clone());
        Ok(WorkerAuthorityResponse {
            owned: Vec::new(),
            not_assigned: request.lease_ids.clone(),
        })
    }
}

fn enforcing_config() -> AuthorityConfig {
    let mut config = AuthorityConfig::new(AuthorityMode::Enforce);
    config.min_lease_age = Duration::ZERO;
    config.confirmations = 1;
    config.grace = Duration::from_secs(1);
    config
}

fn assignment_guard(
    oracle: Arc<dyn AuthorityOracle>,
    host: Arc<dyn AuthorityHost>,
) -> Result<AuthorityGuard, Box<dyn Error>> {
    Ok(AuthorityGuard::new(
        enforcing_config(),
        WorkerId::new("w1")?,
        oracle,
        host,
    ))
}

async fn advance(secs: u64) {
    tokio::time::advance(Duration::from_secs(secs)).await;
}

#[tokio::test(start_paused = true)]
async fn a_stale_answer_cannot_freeze_a_new_assignment_under_the_same_lease_id() -> TestResult {
    let host = AssignmentHost::new("old-attempt", 1)?;
    let replacement = identity("new-attempt", 2)?;
    let oracle = Arc::new(ReassigningOracle {
        host: host.clone(),
        replacement: replacement.clone(),
    });
    let mut guard = assignment_guard(oracle, host.clone())?;
    guard.tick().await;
    assert_eq!(locked(&host.held).as_ref(), Some(&replacement));
    assert!(
        host.events().is_empty(),
        "an answer about the observed old assignment must not mutate its replacement"
    );
    assert!(!host.paused.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_owned_reply_for_another_epoch_cannot_thaw_the_older_assignment() -> TestResult {
    let broker = Arc::new(Broker::default());
    let host = AssignmentHost::new("old-attempt", 1)?;
    let mut guard = assignment_guard(broker.clone(), host.clone())?;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze:old-attempt:1"]);
    broker.owns.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert_eq!(
        host.events(),
        vec!["freeze:old-attempt:1"],
        "broker epoch 2 is not authority to thaw a held epoch 1"
    );
    assert!(host.paused.load(Ordering::SeqCst));
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_purge_retains_the_expired_assignment_and_retries_without_counting_success() -> TestResult {
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.fail_purge.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(Arc::new(Broker::default()), host.clone())?;
    guard.tick().await;
    advance(1).await;
    guard.tick().await;
    assert!(host.is_held());
    assert_eq!(host.purge_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        AuthorityStats::get(&guard.stats().purged),
        0,
        "a backend resource still held after a failed purge is not a successful cleanup"
    );
    guard.tick().await;
    assert_eq!(
        host.purge_attempts.load(Ordering::SeqCst),
        2,
        "the expired cleanup retries without restarting its grace"
    );
    assert!(!host.is_held());
    assert_eq!(AuthorityStats::get(&guard.stats().purged), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failed_thaw_retains_pause_ownership_and_retries() -> TestResult {
    let broker = Arc::new(Broker::default());
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.fail_thaw.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(broker.clone(), host.clone())?;
    guard.tick().await;
    broker.owns.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert!(host.paused.load(Ordering::SeqCst));
    assert_eq!(host.thaw_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 0);
    guard.tick().await;
    assert_eq!(
        host.thaw_attempts.load(Ordering::SeqCst),
        2,
        "a failed thaw must retain the guard's owned transition for retry"
    );
    assert!(!host.paused.load(Ordering::SeqCst));
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn reclaiming_recovers_a_guard_owned_pause_after_its_freeze_reply_was_lost() -> TestResult {
    let broker = Arc::new(Broker::default());
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.lose_freeze_reply.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(broker.clone(), host.clone())?;
    guard.tick().await;
    assert!(
        host.paused.load(Ordering::SeqCst),
        "the freeze effect happened before its error"
    );
    assert_eq!(
        AuthorityStats::get(&guard.stats().frozen),
        0,
        "an unacknowledged freeze is not reported successful"
    );
    broker.owns.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert!(
        !host.paused.load(Ordering::SeqCst),
        "a recovered assignment must release only the host-owned uncertain pause"
    );
    assert_eq!(host.thaw_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_transient_freeze_failure_retries_without_resetting_the_purge_grace() -> TestResult {
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.fail_freeze.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(Arc::new(Broker::default()), host.clone())?;
    guard.tick().await;
    assert!(!host.paused.load(Ordering::SeqCst));
    guard.tick().await;
    assert!(
        host.paused.load(Ordering::SeqCst),
        "an unowned assignment must retry a transient freeze failure"
    );
    assert_eq!(AuthorityStats::get(&guard.stats().frozen), 1);
    advance(1).await;
    guard.tick().await;
    assert!(
        !host.is_held(),
        "retrying freeze must not extend the original purge grace"
    );
    assert_eq!(AuthorityStats::get(&guard.stats().purged), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn confirmed_external_cleanup_retires_the_assignment_without_counting_a_purge() -> TestResult {
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.settle_purge.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(Arc::new(Broker::default()), host.clone())?;
    guard.tick().await;
    advance(1).await;
    guard.tick().await;
    assert!(
        !host.is_held(),
        "the host positively confirmed that exact assignment ended"
    );
    assert_eq!(host.purge_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        AuthorityStats::get(&guard.stats().purged),
        0,
        "external settlement must not invent a guard purge effect"
    );
    guard.tick().await;
    assert_eq!(
        host.purge_attempts.load(Ordering::SeqCst),
        1,
        "a positively settled assignment must not remain in the guard's retry inventory"
    );
    assert_eq!(AuthorityStats::get(&guard.stats().purged), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stale_thaw_retains_pause_ownership_until_a_confirmed_transition() -> TestResult {
    let broker = Arc::new(Broker::default());
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.stale_thaw.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(broker.clone(), host.clone())?;
    guard.tick().await;
    broker.owns.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert!(host.paused.load(Ordering::SeqCst));
    assert_eq!(host.thaw_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 0);
    guard.tick().await;
    assert_eq!(
        host.thaw_attempts.load(Ordering::SeqCst),
        2,
        "an unresolved stale thaw must retain the owned transition rather than silently abandon it"
    );
    assert!(!host.paused.load(Ordering::SeqCst));
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stale_freeze_retry_retains_an_earlier_unacknowledged_pause() -> TestResult {
    let broker = Arc::new(Broker::default());
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.lose_freeze_reply.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(broker.clone(), host.clone())?;
    guard.tick().await;
    assert!(host.paused.load(Ordering::SeqCst));
    host.stale_freeze.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert!(host.paused.load(Ordering::SeqCst));
    broker.owns.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert!(
        !host.paused.load(Ordering::SeqCst),
        "an unconfirmed retry must not erase ownership of the earlier freeze effect"
    );
    assert_eq!(host.thaw_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(AuthorityStats::get(&guard.stats().frozen), 0);
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_stale_purge_keeps_the_assignment_in_the_retry_inventory() -> TestResult {
    let host = AssignmentHost::new("held-attempt", 2)?;
    let mut guard = assignment_guard(Arc::new(Broker::default()), host.clone())?;
    guard.tick().await;
    // The host now holds another assignment, so a purge of the observed one is stale.
    *locked(&host.held) = Some(identity("held-attempt", 3)?);
    advance(1).await;
    guard.tick().await;
    assert_eq!(AuthorityStats::get(&guard.stats().purged), 0);
    let attempts = host.purge_attempts.load(Ordering::SeqCst);
    assert!(attempts >= 1);
    guard.tick().await;
    assert!(
        host.purge_attempts.load(Ordering::SeqCst) > attempts,
        "a stale purge is retried"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_disowned_again_while_a_thaw_is_pending_restarts_its_grace() -> TestResult {
    let broker = Arc::new(Broker::default());
    let host = AssignmentHost::new("held-attempt", 2)?;
    host.stale_thaw.store(true, Ordering::SeqCst);
    let mut guard = assignment_guard(broker.clone(), host.clone())?;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze:held-attempt:2"]);
    broker.owns.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert_eq!(host.thaw_attempts.load(Ordering::SeqCst), 1);

    advance(10).await;
    broker.owns.store(false, Ordering::SeqCst);
    guard.tick().await;
    assert_eq!(
        host.purge_attempts.load(Ordering::SeqCst),
        0,
        "the grace restarts when the broker disowns the lease again"
    );
    advance(1).await;
    guard.tick().await;
    assert_eq!(host.purge_attempts.load(Ordering::SeqCst), 1);
    assert_eq!(AuthorityStats::get(&guard.stats().purged), 1);
    Ok(())
}

/// Wraps the assignment host, overriding what the test needs.
struct Wrapped {
    inner: Arc<AssignmentHost>,
    freeze: Option<FreezeOutcome>,
    thaw: Option<ThawOutcome>,
}

#[async_trait]
impl AuthorityHost for Wrapped {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        self.inner.held_leases().await
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        match self.freeze {
            Some(outcome) => Ok(outcome),
            None => self.inner.freeze(lease).await,
        }
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        match self.thaw {
            Some(outcome) => {
                self.inner.thaw_attempts.fetch_add(1, Ordering::SeqCst);
                Ok(outcome)
            }
            None => self.inner.thaw(lease).await,
        }
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        self.inner.purge(lease).await
    }
}

#[tokio::test(start_paused = true)]
async fn a_thaw_that_the_host_no_longer_owns_forgets_the_suspect() -> TestResult {
    let broker = Arc::new(Broker::default());
    let inner = AssignmentHost::new("held-attempt", 2)?;
    let host = Arc::new(Wrapped {
        inner: inner.clone(),
        freeze: None,
        thaw: Some(ThawOutcome::NoLongerOwned),
    });
    let mut guard = assignment_guard(broker.clone(), host)?;
    guard.tick().await;
    broker.owns.store(true, Ordering::SeqCst);
    guard.tick().await;
    assert_eq!(inner.thaw_attempts.load(Ordering::SeqCst), 1);
    advance(5).await;
    guard.tick().await;
    assert_eq!(
        inner.thaw_attempts.load(Ordering::SeqCst),
        1,
        "the suspect was forgotten"
    );
    assert_eq!(
        inner.purge_attempts.load(Ordering::SeqCst),
        0,
        "a forgotten suspect is not purged"
    );
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_already_paused_lease_is_purged_after_the_grace_without_counting_a_freeze() -> TestResult {
    let inner = AssignmentHost::new("held-attempt", 2)?;
    let host = Arc::new(Wrapped {
        inner: inner.clone(),
        freeze: Some(FreezeOutcome::AlreadyPaused),
        thaw: None,
    });
    let mut guard = assignment_guard(Arc::new(Broker::default()), host)?;
    guard.tick().await;
    advance(1).await;
    guard.tick().await;
    assert_eq!(inner.events(), vec!["purge:held-attempt:2"]);
    assert_eq!(AuthorityStats::get(&guard.stats().frozen), 0);
    assert_eq!(AuthorityStats::get(&guard.stats().purged), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_default_host_methods_report_no_freeze_support_and_no_owned_freeze() -> TestResult {
    struct MinimalHost;

    #[async_trait]
    impl AuthorityHost for MinimalHost {
        async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
            Ok(Vec::new())
        }

        async fn purge(&self, _lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
            Ok(PurgeOutcome::Stale)
        }
    }

    let held = HeldLease::Targeted(identity("a", 1)?);
    assert_eq!(MinimalHost.freeze(&held).await?, FreezeOutcome::Unsupported);
    assert_eq!(MinimalHost.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    assert_eq!(MinimalHost.purge(&held).await?, PurgeOutcome::Stale);
    assert!(MinimalHost.held_leases().await?.is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_failing_inventory_counts_a_failed_check_and_changes_nothing() -> TestResult {
    struct BrokenHost;

    #[async_trait]
    impl AuthorityHost for BrokenHost {
        async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
            Err(AuthorityHostError::Ledger(AttemptLedgerError::PathNotFile))
        }

        async fn purge(&self, _lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
            Err(AuthorityHostError::ConfirmationMissing)
        }
    }

    let mut guard = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::Enforce),
        WorkerId::new("w1")?,
        Arc::new(Broker::default()),
        Arc::new(BrokenHost),
    );
    guard.tick().await;
    advance(600).await;
    guard.tick().await;
    let stats = guard.stats();
    assert_eq!(AuthorityStats::get(&stats.check_failures), 2);
    assert_eq!(AuthorityStats::get(&stats.partitioned), 0);
    assert_eq!(AuthorityStats::get(&stats.unowned), 0);
    Ok(())
}

/// Counts requests and their sizes.
#[derive(Default)]
struct RecordingOracle {
    sizes: Mutex<Vec<usize>>,
    ids: Mutex<Vec<String>>,
}

#[async_trait]
impl AuthorityOracle for RecordingOracle {
    async fn owned(&self, request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        locked(&self.sizes).push(request.lease_ids.len());
        locked(&self.ids).extend(request.lease_ids.iter().map(|id| id.as_str().to_owned()));
        Ok(WorkerAuthorityResponse::default())
    }
}

struct ListHost(Vec<HeldLease>);

#[async_trait]
impl AuthorityHost for ListHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self.0.clone())
    }

    async fn purge(&self, _lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        Ok(PurgeOutcome::Stale)
    }
}

fn held_with_lease(lease: &str, attempt: &str) -> Result<HeldLease, Box<dyn Error>> {
    Ok(HeldLease::Targeted(LeaseAttemptIdentity {
        lease_id: LeaseId::new(lease),
        provider: ProviderKind::Mock,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new(attempt)?,
            epoch: Some(LeaseEpoch::FIRST),
        },
    }))
}

#[tokio::test(start_paused = true)]
async fn large_inventories_are_asked_in_bounded_requests() -> TestResult {
    let total = MAX_LEASES_PER_REQUEST + 1;
    let held = (0..total)
        .map(|index| held_with_lease(&format!("lease-{index:05}"), &format!("attempt-{index}")))
        .collect::<Result<Vec<_>, _>>()?;
    let oracle = Arc::new(RecordingOracle::default());
    let mut guard = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::Warn),
        WorkerId::new("w1")?,
        oracle.clone(),
        Arc::new(ListHost(held)),
    );
    guard.tick().await;
    assert_eq!(*locked(&oracle.sizes), vec![MAX_LEASES_PER_REQUEST, 1]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_request_lists_each_lease_once_in_sorted_order() -> TestResult {
    let held = vec![
        held_with_lease("b", "attempt-1")?,
        held_with_lease("a", "attempt-2")?,
        held_with_lease("b", "attempt-3")?,
    ];
    let oracle = Arc::new(RecordingOracle::default());
    let mut guard = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::Warn),
        WorkerId::new("w1")?,
        oracle.clone(),
        Arc::new(ListHost(held)),
    );
    guard.tick().await;
    assert_eq!(*locked(&oracle.sizes), vec![2]);
    assert_eq!(*locked(&oracle.ids), vec!["a".to_owned(), "b".to_owned()]);
    Ok(())
}

struct SlowOracle;

#[async_trait]
impl AuthorityOracle for SlowOracle {
    async fn owned(&self, _request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        tokio::time::sleep(Duration::from_secs(3600)).await;
        Ok(WorkerAuthorityResponse::default())
    }
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_request_times_out_and_counts_as_a_failed_check() -> TestResult {
    let held = vec![held_with_lease("a", "attempt-1")?];
    let mut guard = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::Warn),
        WorkerId::new("w1")?,
        Arc::new(SlowOracle),
        Arc::new(ListHost(held)),
    );
    let started = tokio::time::Instant::now();
    guard.tick().await;
    assert_eq!(started.elapsed(), Duration::from_secs(5));
    let stats = guard.stats();
    assert_eq!(AuthorityStats::get(&stats.check_failures), 1);
    assert_eq!(AuthorityStats::get(&stats.no_responders), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn warn_counts_a_partition_but_never_freezes_or_purges() -> TestResult {
    let held = vec![held_with_lease("a", "attempt-1")?];
    let mut guard = AuthorityGuard::new(
        AuthorityConfig::new(AuthorityMode::Warn),
        WorkerId::new("w1")?,
        Arc::new(SlowOracle),
        Arc::new(ListHost(held)),
    );
    guard.tick().await;
    advance(100).await;
    guard.tick().await;
    let stats = guard.stats();
    assert_eq!(AuthorityStats::get(&stats.check_failures), 2);
    assert_eq!(AuthorityStats::get(&stats.partitioned), 1);
    assert_eq!(AuthorityStats::get(&stats.frozen), 0);
    assert_eq!(AuthorityStats::get(&stats.purged), 0);
    Ok(())
}
