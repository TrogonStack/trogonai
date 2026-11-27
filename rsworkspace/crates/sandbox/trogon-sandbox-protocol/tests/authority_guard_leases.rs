use std::error::Error;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, OwnedLease, ProviderKind,
    WorkerAuthorityRequest, WorkerAuthorityResponse, WorkerId,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityHost, AuthorityHostError, AuthorityMode, AuthorityOracle,
    AuthorityResourceId, AuthorityStats, FreezeOutcome, HeldLease, OracleError, PurgeOutcome, ThawOutcome,
};

type TestResult = Result<(), Box<dyn Error>>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn lease(id: &str) -> LeaseId {
    LeaseId::new(id)
}

#[derive(Clone, Copy)]
enum Failure {
    Healthy,
    Down,
    NoResponders,
}

/// What the broker is made to answer.
struct FakeBroker {
    owned: Mutex<Vec<LeaseId>>,
    elsewhere: Mutex<Vec<LeaseId>>,
    failure: Mutex<Failure>,
    epoch: Option<LeaseEpoch>,
}

impl FakeBroker {
    fn new(epoch: Option<LeaseEpoch>) -> Arc<Self> {
        Arc::new(Self {
            owned: Mutex::new(Vec::new()),
            elsewhere: Mutex::new(Vec::new()),
            failure: Mutex::new(Failure::Healthy),
            epoch,
        })
    }

    fn owns(&self, leases: &[&str]) {
        let owned: Vec<_> = leases.iter().map(|id| lease(id)).collect();
        locked(&self.elsewhere).retain(|id| !owned.contains(id));
        *locked(&self.owned) = owned;
    }

    /// Leases the broker knows but assigns to someone else.
    fn assigns_elsewhere(&self, leases: &[&str]) {
        *locked(&self.elsewhere) = leases.iter().map(|id| lease(id)).collect();
    }

    fn set_down(&self, down: bool) {
        *locked(&self.failure) = if down { Failure::Down } else { Failure::Healthy };
    }

    fn set_no_responders(&self, on: bool) {
        *locked(&self.failure) = if on { Failure::NoResponders } else { Failure::Healthy };
    }
}

#[async_trait]
impl AuthorityOracle for FakeBroker {
    async fn owned(&self, request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        match *locked(&self.failure) {
            Failure::Healthy => {}
            Failure::Down => return Err(OracleError::Failed("request timed out".into())),
            Failure::NoResponders => return Err(OracleError::NoResponders),
        }
        let owned = locked(&self.owned);
        let elsewhere = locked(&self.elsewhere);
        Ok(WorkerAuthorityResponse {
            not_assigned: request
                .lease_ids
                .iter()
                .filter(|id| elsewhere.contains(id))
                .cloned()
                .collect(),
            owned: request
                .lease_ids
                .iter()
                .filter(|id| owned.contains(id))
                .map(|id| OwnedLease {
                    lease_id: id.clone(),
                    epoch: self.epoch,
                })
                .collect(),
        })
    }
}

/// An oracle that always gives the same answer.
struct FixedOracle(WorkerAuthorityResponse);

#[async_trait]
impl AuthorityOracle for FixedOracle {
    async fn owned(&self, _request: &WorkerAuthorityRequest) -> Result<WorkerAuthorityResponse, OracleError> {
        Ok(self.0.clone())
    }
}

fn targeted(id: &str, worker: &str) -> Result<HeldLease, Box<dyn Error>> {
    Ok(HeldLease::Targeted(LeaseAttemptIdentity {
        lease_id: lease(id),
        provider: ProviderKind::Mock,
        target: DispatchTarget {
            worker_id: WorkerId::new(worker)?,
            attempt_id: DispatchAttemptId::new(format!("test-{id}"))?,
            epoch: Some(LeaseEpoch::FIRST),
        },
    }))
}

/// A worker holding some leases and recording what the guard did to them.
struct FakeHost {
    held: Mutex<Vec<HeldLease>>,
    can_freeze: bool,
    events: Mutex<Vec<String>>,
}

impl FakeHost {
    fn new(held: &[&str], can_freeze: bool) -> Result<Arc<Self>, Box<dyn Error>> {
        let held = held
            .iter()
            .map(|id| targeted(id, "w1"))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Arc::new(Self {
            held: Mutex::new(held),
            can_freeze,
            events: Mutex::new(Vec::new()),
        }))
    }

    fn with_leases(held: Vec<HeldLease>) -> Arc<Self> {
        Arc::new(Self {
            held: Mutex::new(held),
            can_freeze: true,
            events: Mutex::new(Vec::new()),
        })
    }

    fn events(&self) -> Vec<String> {
        locked(&self.events).clone()
    }
}

#[async_trait]
impl AuthorityHost for FakeHost {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(locked(&self.held).clone())
    }

    async fn freeze(&self, lease: &HeldLease) -> Result<FreezeOutcome, AuthorityHostError> {
        if self.can_freeze {
            locked(&self.events).push(format!("freeze {}", lease.lease_id().as_str()));
            Ok(FreezeOutcome::Frozen)
        } else {
            Ok(FreezeOutcome::Unsupported)
        }
    }

    async fn thaw(&self, lease: &HeldLease) -> Result<ThawOutcome, AuthorityHostError> {
        locked(&self.events).push(format!("thaw {}", lease.lease_id().as_str()));
        Ok(ThawOutcome::Thawed)
    }

    async fn purge(&self, lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        locked(&self.events).push(format!("purge {}", lease.lease_id().as_str()));
        locked(&self.held).retain(|held| held != lease);
        Ok(PurgeOutcome::Purged)
    }
}

fn guard(
    mode: AuthorityMode,
    oracle: Arc<dyn AuthorityOracle>,
    host: &Arc<FakeHost>,
) -> Result<AuthorityGuard, Box<dyn Error>> {
    Ok(AuthorityGuard::new(
        AuthorityConfig::new(mode),
        WorkerId::new("w1")?,
        oracle,
        host.clone(),
    ))
}

async fn advance(secs: u64) {
    tokio::time::advance(Duration::from_secs(secs)).await;
}

/// Runs the checks that first judge a lease the broker disowns: first seen at t0, old enough at 30 s (first
/// strike), confirmed at 45 s.
async fn disown_for_long_enough(guard: &mut AuthorityGuard) {
    guard.tick().await;
    advance(30).await;
    guard.tick().await;
    advance(15).await;
    guard.tick().await;
}

#[tokio::test(start_paused = true)]
async fn an_unknown_lease_is_frozen_then_purged_after_the_grace_under_enforce() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.owns(&["mine"]);
    let host = FakeHost::new(&["mine", "gone"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    disown_for_long_enough(&mut guard).await;
    assert_eq!(host.events(), vec!["freeze gone"]);

    advance(59).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze gone"], "still inside the grace");

    advance(1).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze gone", "purge gone"]);
    let stats = guard.stats();
    assert_eq!(AuthorityStats::get(&stats.unowned), 1);
    assert_eq!(AuthorityStats::get(&stats.frozen), 1);
    assert_eq!(AuthorityStats::get(&stats.purged), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn warn_logs_and_counts_but_never_touches_the_lease() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&["gone"], true)?;
    let mut guard = guard(AuthorityMode::Warn, broker, &host)?;

    for _ in 0..10 {
        guard.tick().await;
        advance(15).await;
    }
    assert_eq!(host.events(), Vec::<String>::new());
    let stats = guard.stats();
    assert_eq!(AuthorityStats::get(&stats.unowned), 1, "counted once");
    assert_eq!(AuthorityStats::get(&stats.frozen), 0);
    assert_eq!(AuthorityStats::get(&stats.purged), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn off_never_asks() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.set_down(true);
    let host = FakeHost::new(&["gone"], true)?;
    let mut guard = guard(AuthorityMode::Off, broker, &host)?;

    guard.tick().await;
    assert_eq!(AuthorityStats::get(&guard.stats().check_failures), 0);
    assert_eq!(host.events(), Vec::<String>::new());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn running_in_off_mode_returns_immediately() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&["gone"], true)?;
    let guard = guard(AuthorityMode::Off, broker, &host)?;
    let cancel = CancellationToken::new();
    tokio::time::timeout(Duration::from_secs(1), guard.run(cancel)).await?;
    assert!(host.events().is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn running_checks_on_a_timer_until_cancelled() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.set_no_responders(true);
    let host = FakeHost::new(&["a"], true)?;
    let guard = guard(AuthorityMode::Warn, broker, &host)?;
    let stats = guard.stats();
    let cancel = CancellationToken::new();
    let task = tokio::spawn(guard.run(cancel.clone()));

    tokio::time::sleep(Duration::from_secs(40)).await;
    // The first tick fires at once, then every 15 s: ticks at 0, 15 and 30 s.
    assert_eq!(AuthorityStats::get(&stats.no_responders), 3);

    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(1), task).await??;
    let after = AuthorityStats::get(&stats.no_responders);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(AuthorityStats::get(&stats.no_responders), after);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_reclaimed_during_the_grace_is_thawed_and_not_purged() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&["l1"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    disown_for_long_enough(&mut guard).await;
    advance(30).await;
    broker.owns(&["l1"]);
    guard.tick().await;
    advance(120).await;
    guard.tick().await;

    assert_eq!(host.events(), vec!["freeze l1", "thaw l1"]);
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn contradictory_broker_authority_cannot_thaw_a_frozen_assignment() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&["l1"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;
    disown_for_long_enough(&mut guard).await;
    broker.owns(&["l1"]);
    broker.assigns_elsewhere(&["l1"]);
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze l1"]);
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn failed_checks_freeze_every_lease_only_after_the_partition_window() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.owns(&["a", "b"]);
    let host = FakeHost::new(&["a", "b"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    guard.tick().await;
    broker.set_down(true);
    advance(15).await;
    guard.tick().await;
    advance(15).await;
    guard.tick().await;
    assert_eq!(host.events(), Vec::<String>::new(), "30s of silence is tolerated");

    advance(15).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze a", "freeze b"]);
    let stats = guard.stats();
    assert_eq!(AuthorityStats::get(&stats.partitioned), 2);
    assert_eq!(AuthorityStats::get(&stats.unowned), 0);
    assert_eq!(AuthorityStats::get(&stats.check_failures), 3);

    advance(60).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze a", "freeze b", "purge a", "purge b"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn the_broker_coming_back_thaws_a_partition_freeze() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.owns(&["a"]);
    let host = FakeHost::new(&["a"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    guard.tick().await;
    broker.set_down(true);
    advance(50).await;
    guard.tick().await;
    broker.set_down(false);
    advance(15).await;
    guard.tick().await;

    assert_eq!(host.events(), vec!["freeze a", "thaw a"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_provider_that_cannot_freeze_is_purged_after_the_grace() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&["gone"], false)?;
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    disown_for_long_enough(&mut guard).await;
    assert_eq!(host.events(), Vec::<String>::new());
    advance(60).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["purge gone"]);
    assert_eq!(AuthorityStats::get(&guard.stats().frozen), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_idle_worker_does_not_count_as_partitioned_when_a_lease_arrives() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&[], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    guard.tick().await;
    broker.set_down(true);
    advance(600).await;
    guard.tick().await;
    locked(&host.held).push(targeted("new", "w1")?);
    advance(15).await;
    guard.tick().await;

    assert_eq!(host.events(), Vec::<String>::new());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_young_lease_is_never_frozen_even_if_the_broker_does_not_know_it() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&["fresh"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    for _ in 0..2 {
        guard.tick().await;
        advance(14).await;
    }
    guard.tick().await;
    assert_eq!(host.events(), Vec::<String>::new(), "29 s old");
    assert_eq!(AuthorityStats::get(&guard.stats().unowned), 0);

    // Registered by the broker in time: never suspected afterwards.
    broker.owns(&["fresh"]);
    advance(60).await;
    guard.tick().await;
    assert_eq!(host.events(), Vec::<String>::new());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn one_miss_is_not_enough_and_an_owned_answer_resets_the_count() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.assigns_elsewhere(&["l1"]);
    let host = FakeHost::new(&["l1"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    guard.tick().await;
    advance(30).await;
    guard.tick().await; // first strike, the lease is old enough
    broker.owns(&["l1"]);
    advance(15).await;
    guard.tick().await; // owned again: the strike is forgotten
    broker.owns(&[]);
    advance(15).await;
    guard.tick().await; // first strike again
    assert_eq!(host.events(), Vec::<String>::new());

    advance(15).await;
    guard.tick().await; // second consecutive miss
    assert_eq!(host.events(), vec!["freeze l1"]);
    assert_eq!(AuthorityStats::get(&guard.stats().unowned), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_assigned_elsewhere_is_judged_like_an_unknown_one() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.assigns_elsewhere(&["moved"]);
    let host = FakeHost::new(&["moved", "unknown"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    guard.tick().await;
    advance(29).await;
    guard.tick().await;
    assert_eq!(host.events(), Vec::<String>::new());
    advance(1).await;
    guard.tick().await;
    advance(15).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze moved", "freeze unknown"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn enforce_does_not_accept_missing_authority_as_a_renewal() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.set_no_responders(true);
    let host = FakeHost::new(&["a"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    guard.tick().await;
    advance(44).await;
    guard.tick().await;
    assert!(host.events().is_empty());
    advance(1).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze a"]);
    advance(60).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze a", "purge a"]);
    assert_eq!(AuthorityStats::get(&guard.stats().partitioned), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn missing_authority_does_not_postpone_an_existing_partition_purge() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.owns(&["a"]);
    let host = FakeHost::new(&["a"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    guard.tick().await;
    broker.set_down(true);
    advance(45).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze a"]);
    broker.set_no_responders(true);
    advance(60).await;
    guard.tick().await;
    assert_eq!(host.events(), vec!["freeze a", "purge a"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn warn_tolerates_old_brokers_without_authority_checks() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.set_no_responders(true);
    let host = FakeHost::new(&["a"], true)?;
    let mut guard = guard(AuthorityMode::Warn, broker.clone(), &host)?;

    for _ in 0..40 {
        guard.tick().await;
        advance(15).await;
    }
    assert_eq!(host.events(), Vec::<String>::new(), "ten minutes of old broker");
    let stats = guard.stats();
    assert_eq!(AuthorityStats::get(&stats.no_responders), 40);
    assert_eq!(AuthorityStats::get(&stats.check_failures), 0);
    assert_eq!(AuthorityStats::get(&stats.partitioned), 0);

    // Real failures still count, from a clean slate.
    broker.set_down(true);
    guard.tick().await;
    advance(45).await;
    guard.tick().await;
    assert!(host.events().is_empty());
    assert_eq!(AuthorityStats::get(&guard.stats().partitioned), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_provider_resource_is_confirmed_by_an_answer_without_an_epoch() -> TestResult {
    let broker = FakeBroker::new(None);
    broker.owns(&["p1"]);
    let held = HeldLease::Provider {
        lease_id: lease("p1"),
        provider: ProviderKind::Subprocess,
        resource_id: AuthorityResourceId::new("pid-1")?,
    };
    let host = FakeHost::with_leases(vec![held]);
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    disown_for_long_enough(&mut guard).await;
    advance(120).await;
    guard.tick().await;

    assert!(host.events().is_empty());
    assert_eq!(AuthorityStats::get(&guard.stats().unowned), 0);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_provider_resource_is_not_confirmed_by_an_answer_with_an_epoch() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.owns(&["p1"]);
    let held = HeldLease::Provider {
        lease_id: lease("p1"),
        provider: ProviderKind::Subprocess,
        resource_id: AuthorityResourceId::new("pid-1")?,
    };
    let host = FakeHost::with_leases(vec![held]);
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    disown_for_long_enough(&mut guard).await;

    assert_eq!(host.events(), vec!["freeze p1"]);
    assert_eq!(AuthorityStats::get(&guard.stats().unowned), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_targeted_lease_without_an_epoch_is_never_confirmed() -> TestResult {
    let broker = FakeBroker::new(None);
    broker.owns(&["l1"]);
    let held = HeldLease::Targeted(LeaseAttemptIdentity {
        lease_id: lease("l1"),
        provider: ProviderKind::Mock,
        target: DispatchTarget {
            worker_id: WorkerId::new("w1")?,
            attempt_id: DispatchAttemptId::new("a1")?,
            epoch: None,
        },
    });
    let host = FakeHost::with_leases(vec![held]);
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    disown_for_long_enough(&mut guard).await;

    assert_eq!(host.events(), vec!["freeze l1"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_assignment_to_another_worker_is_not_confirmed_by_the_broker() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    broker.owns(&["l1"]);
    let host = FakeHost::with_leases(vec![targeted("l1", "someone-else")?]);
    let mut guard = guard(AuthorityMode::Enforce, broker, &host)?;

    disown_for_long_enough(&mut guard).await;

    assert_eq!(host.events(), vec!["freeze l1"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_ambiguous_broker_answer_does_not_confirm_a_lease() -> TestResult {
    let answer = WorkerAuthorityResponse {
        owned: vec![
            OwnedLease {
                lease_id: lease("l1"),
                epoch: Some(LeaseEpoch::FIRST),
            },
            OwnedLease {
                lease_id: lease("l1"),
                epoch: Some(LeaseEpoch::FIRST),
            },
        ],
        not_assigned: Vec::new(),
    };
    let host = FakeHost::new(&["l1"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, Arc::new(FixedOracle(answer)), &host)?;

    disown_for_long_enough(&mut guard).await;

    assert_eq!(host.events(), vec!["freeze l1"]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn every_disowned_lease_is_frozen_whether_assigned_elsewhere_or_unknown() -> TestResult {
    let answer = WorkerAuthorityResponse {
        owned: Vec::new(),
        not_assigned: vec![lease("l1")],
    };
    let host = FakeHost::new(&["l1", "l2"], true)?;
    let mut guard = guard(AuthorityMode::Enforce, Arc::new(FixedOracle(answer)), &host)?;

    disown_for_long_enough(&mut guard).await;

    assert_eq!(host.events(), vec!["freeze l1", "freeze l2"]);
    assert_eq!(AuthorityStats::get(&guard.stats().unowned), 2);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_lease_that_could_not_be_frozen_is_forgotten_when_the_broker_reclaims_it() -> TestResult {
    let broker = FakeBroker::new(Some(LeaseEpoch::FIRST));
    let host = FakeHost::new(&["l1"], false)?;
    let mut guard = guard(AuthorityMode::Enforce, broker.clone(), &host)?;

    disown_for_long_enough(&mut guard).await;
    assert_eq!(AuthorityStats::get(&guard.stats().unowned), 1);
    broker.owns(&["l1"]);
    advance(15).await;
    guard.tick().await;
    advance(120).await;
    guard.tick().await;

    assert!(host.events().is_empty(), "nothing was frozen, thawed or purged");
    assert_eq!(AuthorityStats::get(&guard.stats().thawed), 0);
    Ok(())
}
