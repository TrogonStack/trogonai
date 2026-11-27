use super::context::TestError;
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_nats::jetstream::{self, kv, stream};
use async_trait::async_trait;
use trogon_sandbox_broker::command_launch::{
    CancellationAuthorityClaim, CancellationCustodySupport, Claim, CommandCancellationAuthority, CommandLaunch,
    CommandLaunchError, CommandLaunchStore, InMemoryCommandLaunchStore, LaunchState, NatsKvCommandLaunchStore,
    fingerprint,
};

/// A failure injected into the stub store.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(
        error_type_naming,
        reason = "the test double mirrors the production authority store name"
    )
)]
struct Injected(String);

fn injected(message: &str) -> CommandLaunchError {
    CommandLaunchError::backend(Injected(message.to_owned()))
}

#[derive(Default)]
struct FailingStore {
    inner: InMemoryCommandLaunchStore,
    fail_write: AtomicBool,
    fail_read: AtomicBool,
    missing_read: AtomicBool,
    unsupported: AtomicBool,
    writes: AtomicUsize,
    reads: AtomicUsize,
    reassign_after_new: tokio::sync::Mutex<Option<(Arc<dyn ActiveLeaseStore>, LeaseRecord)>>,
    reassignments: AtomicUsize,
}

#[async_trait]
impl CommandLaunchStore for FailingStore {
    fn cancellation_custody_support(&self) -> CancellationCustodySupport {
        if self.unsupported.load(Ordering::SeqCst) {
            CancellationCustodySupport::Unsupported
        } else {
            CancellationCustodySupport::Retained
        }
    }

    async fn claim(&self, lease: &LeaseId, key: &LaunchKey, fingerprint: &str) -> Result<Claim, CommandLaunchError> {
        self.inner.claim(lease, key, fingerprint).await
    }

    async fn retain_cancellation_authority(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        authority: &CommandCancellationAuthority,
    ) -> Result<CancellationAuthorityClaim, CommandLaunchError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        if self.fail_write.load(Ordering::SeqCst) {
            return Err(injected("injected retained-authority persistence failure"));
        }
        let claim = self.inner.retain_cancellation_authority(lease, key, authority).await?;
        if claim == CancellationAuthorityClaim::New {
            let reassignment = self.reassign_after_new.lock().await.take();
            if let Some((active, record)) = reassignment {
                active
                    .put(&record)
                    .await
                    .map_err(|error| injected(&error.to_string()))?;
                self.reassignments.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(claim)
    }

    async fn cancellation_authority(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
    ) -> Result<Option<CommandCancellationAuthority>, CommandLaunchError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if self.fail_read.load(Ordering::SeqCst) {
            return Err(injected("injected retained-authority read failure"));
        }
        if self.missing_read.load(Ordering::SeqCst) {
            return Ok(None);
        }
        self.inner.cancellation_authority(lease, key).await
    }

    async fn finish(&self, lease: &LeaseId, key: &LaunchKey, launch: &CommandLaunch) -> Result<(), CommandLaunchError> {
        self.inner.finish(lease, key, launch).await
    }

    async fn forget(&self, lease: &LeaseId, key: &LaunchKey) -> Result<(), CommandLaunchError> {
        self.inner.forget(lease, key).await
    }

    async fn replace(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        expected: &CommandLaunch,
        next: &CommandLaunch,
    ) -> Result<bool, CommandLaunchError> {
        self.inner.replace(lease, key, expected, next).await
    }

    async fn forget_lease(&self, lease: &LeaseId) -> Result<(), CommandLaunchError> {
        self.inner.forget_lease(lease).await
    }
}

fn key(value: &str) -> TestResult<LaunchKey> {
    Ok(LaunchKey::new(value)?)
}

async fn fixture(store: Arc<dyn CommandLaunchStore>) -> TestResult<(Harness, LeaseRecord)> {
    let harness = Harness::with_launch_store(Some(store)).await?;
    let worker = WorkerId::new("authority-store-owner")?;
    harness.heartbeat(&worker, true).await?;
    let pending = harness.begin_lease(&worker, "authority-store-lease").await?;
    let (_, record) = harness.finish_lease(pending).await?;
    Ok((harness, record))
}

async fn attempt_start(harness: &Harness, record: &LeaseRecord, key: &str) -> TestResult<(RunCommandResponse, bool)> {
    attempt_start_mode(harness, record, key, true).await
}

async fn attempt_start_mode(
    harness: &Harness,
    record: &LeaseRecord,
    key: &str,
    strong: bool,
) -> TestResult<(RunCommandResponse, bool)> {
    let mut runs = harness
        .client
        .subscribe(subjects::provider_run_command(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let mut readiness = if strong {
        Some(
            harness
                .subscribe_readiness(&Harness::readiness_binding(record)?)
                .await?,
        )
    } else {
        None
    };
    let mut task = {
        let core = harness.core.clone();
        let mut request = command(&record.lease_id, key)?;
        request.require_launch_cancellation = strong.into();
        tokio::spawn(async move { core.handle_run_command(request).await })
    };
    let binding = Harness::readiness_binding(record)?;
    let result = tokio::time::timeout(WAIT, async {
        loop {
            tokio::select! {
                message = async {
                    match readiness.as_mut() {
                        Some(subscription) => subscription.next().await,
                        None => None,
                    }
                }, if readiness.is_some() => {
                    let message = message.context("readiness subscription ended")?;
                    harness.reply_readiness(message, &binding).await?;
                    if let Some(mut subscription) = readiness.take() {
                        subscription.unsubscribe().await?;
                    }
                    harness.client.flush().await?;
                }
                response = &mut task => break Ok::<_, TestError>((response?, false)),
                message = runs.next() => {
                    let message = message.context("the run subscription ended")?;
                    harness.verify_owned_verb(&message, record)?;
                    let response: RunCommandResponse = serde_json::from_value(json!({
                        "outcome":"started", "execution_id":"synthetic-attempted-execution", "origin":"created",
                    }))?;
                    harness
                        .client
                        .publish(
                            message.reply.context("the run carries a reply subject")?,
                            serde_json::to_vec(&response)?.into(),
                        )
                        .await?;
                    break Ok((task.await?, true))
                }
            }
        }
    })
    .await
    .context("the attempt settles")??;
    no_message(&mut runs, "one logical attempt must not retry provider dispatch").await;
    Ok(result)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_authority_persistence_refuses_before_any_strong_command_dispatch() -> TestResult {
    let store = Arc::new(FailingStore::default());
    store.fail_write.store(true, Ordering::SeqCst);
    let (harness, record) = fixture(store.clone()).await?;
    let (response, delivered) = attempt_start(&harness, &record, "failed-persistence").await?;
    eprintln!(
        "broker failed authority persistence receipt: write_attempts={} delivered={delivered} response={response:?} worker_is_stub=true native_acceptance=false",
        store.writes.load(Ordering::SeqCst)
    );
    assert!(
        !delivered,
        "failed durable authority persistence must refuse before any command effect"
    );
    assert!(matches!(response, RunCommandResponse::Denied(_)));
    assert_eq!(store.writes.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_authority_read_keeps_old_cancellation_unsettled_without_dispatch() -> TestResult {
    let store = Arc::new(FailingStore::default());
    let (harness, record) = fixture(store.clone()).await?;
    super::retained_authority::start(&harness, &record, "owned-launch", true).await?;
    store.reads.store(0, Ordering::SeqCst);
    harness.heartbeat(assigned_worker(&record)?, false).await?;
    store.fail_read.store(true, Ordering::SeqCst);
    let mut cancels = harness
        .client
        .subscribe("sandbox.provider.*.cancel_command_launch.*")
        .await?;
    harness.client.flush().await?;
    let response = harness
        .core
        .handle_cancel_command_launch(cancellation(&record.lease_id, "owned-launch")?)
        .await;
    no_message(&mut cancels, "failed authority read must not dispatch cancellation").await;
    eprintln!(
        "broker failed authority read receipt: read_attempts={} response={response:?} provider_delivery=0 worker_is_stub=true native_acceptance=false",
        store.reads.load(Ordering::SeqCst)
    );
    assert!(
        matches!(response, CancelCommandLaunchResponse::Pending { .. }),
        "unavailable retained authority is unsettled, not proof of unsupported cleanup: {response:?}"
    );
    assert_eq!(store.reads.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_custody_record_cannot_invent_retained_cancellation_authority() -> TestResult {
    let store = Arc::new(FailingStore::default());
    let (harness, record) = fixture(store.clone()).await?;
    super::retained_authority::start(&harness, &record, "owned-launch", true).await?;
    harness.heartbeat(assigned_worker(&record)?, false).await?;
    store.missing_read.store(true, Ordering::SeqCst);
    let mut cancels = harness
        .client
        .subscribe("sandbox.provider.*.cancel_command_launch.*")
        .await?;
    harness.client.flush().await?;
    let response = harness
        .core
        .handle_cancel_command_launch(cancellation(&record.lease_id, "owned-launch")?)
        .await;
    no_message(&mut cancels, "missing custody must not invent a retained route").await;
    assert_eq!(response, CancelCommandLaunchResponse::Unsupported);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reassignment_after_new_custody_reservation_refuses_before_the_first_effect() -> TestResult {
    let store = Arc::new(FailingStore::default());
    let (harness, record) = fixture(store.clone()).await?;
    let mut replacement = record.clone();
    let second = LeaseEpoch::new(2)?;
    replacement.worker_id = Some(WorkerId::new("replacement-owner")?);
    let attempt = replacement
        .dispatch_attempt
        .as_mut()
        .context("the owning record names its attempt")?;
    attempt.id = trogon_sandbox_contract::DispatchAttemptId::new("replacement-attempt")?;
    attempt.epoch = Some(second);
    replacement.epoch = Some(second);
    replacement.sandbox_id = "replacement-native-receipt".into();
    *store.reassign_after_new.lock().await = Some((harness.active.clone(), replacement.clone()));
    let (response, delivered) = attempt_start(&harness, &record, "reassignment-race").await?;
    let actual = harness
        .active
        .get(&record.lease_id)
        .await?
        .context("the lease stays stored")?;
    eprintln!(
        "broker reserve/send reassignment race: new_reservations={} actual_reassignments={} delivered={delivered} response={response:?} worker_is_stub=true native_acceptance=false",
        store.writes.load(Ordering::SeqCst),
        store.reassignments.load(Ordering::SeqCst)
    );
    assert_eq!(
        store.reassignments.load(Ordering::SeqCst),
        1,
        "the actual store boundary must install the newer assignment before the first strong send"
    );
    assert_eq!(actual.worker_id, replacement.worker_id);
    assert_eq!(actual.dispatch_attempt, replacement.dispatch_attempt);
    assert_eq!(actual.epoch, replacement.epoch);
    assert_eq!(actual.sandbox_id, replacement.sandbox_id);
    assert!(
        !delivered,
        "a reservation must not be dispatched after its exact assigned owner changes"
    );
    assert!(matches!(response, RunCommandResponse::Denied(_)));
    Ok(())
}

async fn retained_after_reply_removal(expired: bool) -> TestResult {
    retained_after_reply_removal_with_mode(expired, true).await
}

async fn retained_after_reply_removal_with_mode(expired: bool, strong_duplicate: bool) -> TestResult {
    let store = Arc::new(InMemoryCommandLaunchStore::with_max_age(if expired {
        Duration::from_millis(80)
    } else {
        Duration::from_secs(3600)
    }));
    let (harness, record) = fixture(store.clone()).await?;
    let started = super::retained_authority::start(&harness, &record, "owned-launch", true).await?;
    let RunCommandResponse::Started(started) = started else {
        return Err(format!("the owned launch must start: {started:?}").into());
    };
    if expired {
        tokio::time::sleep(Duration::from_millis(200)).await;
    } else {
        store.forget_lease(&record.lease_id).await?;
    }
    harness.heartbeat(assigned_worker(&record)?, false).await?;
    let mut cancels = harness
        .client
        .subscribe(subjects::provider_cancel_command_launch(PROVIDER, &record.lease_id))
        .await?;
    harness.client.flush().await?;
    let expected = CancelCommandLaunchResponse::Canceled {
        execution_id: Some(started.execution_id),
    };
    let (canceled, routed) = harness
        .cancel(
            cancellation(&record.lease_id, "owned-launch")?,
            &record,
            &mut cancels,
            &expected,
        )
        .await?;
    harness.heartbeat(assigned_worker(&record)?, true).await?;
    let (duplicate, duplicate_delivered) =
        attempt_start_mode(&harness, &record, "owned-launch", strong_duplicate).await?;
    eprintln!(
        "broker retained custody after reply removal: expired={expired} strong_duplicate={strong_duplicate} old_cancel_routed={routed} old_cancel={canceled:?} fresh_admission_restored=true duplicate_delivered={duplicate_delivered} duplicate={duplicate:?} worker_is_stub=true native_acceptance=false"
    );
    assert!(
        !duplicate_delivered,
        "a retained custody reservation must refuse another effect after the reply disappears"
    );
    assert!(matches!(duplicate, RunCommandResponse::Denied(_)));
    assert_eq!(
        canceled, expected,
        "reply retention must not govern exact old cancellation authority"
    );
    assert!(routed);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_deletion_preserves_cancellation_and_refuses_duplicate_effects() -> TestResult {
    retained_after_reply_removal(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_expiry_preserves_cancellation_and_refuses_duplicate_effects() -> TestResult {
    retained_after_reply_removal(true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_deletion_refuses_downgraded_ordinary_launch_key_reuse() -> TestResult {
    retained_after_reply_removal_with_mode(false, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_expiry_refuses_downgraded_ordinary_launch_key_reuse() -> TestResult {
    retained_after_reply_removal_with_mode(true, false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_custody_refuses_strong_launches_and_preserves_ordinary_execution() -> TestResult {
    let store = Arc::new(FailingStore::default());
    store.unsupported.store(true, Ordering::SeqCst);
    let (harness, record) = fixture(store.clone()).await?;
    let (strong, strong_delivered) = attempt_start(&harness, &record, "unsupported-strong").await?;
    let (ordinary, ordinary_delivered) = attempt_start_mode(&harness, &record, "ordinary-control", false).await?;
    assert!(
        ordinary_delivered && matches!(ordinary, RunCommandResponse::Started(_)),
        "ordinary unsupported-store behavior must remain available"
    );
    eprintln!(
        "broker custody support boundary: ordinary_delivered={ordinary_delivered} strong_delivered={strong_delivered} strong={strong:?} retain_attempts={} worker_is_stub=true native_acceptance=false",
        store.writes.load(Ordering::SeqCst)
    );
    assert!(
        !strong_delivered && matches!(strong, RunCommandResponse::Denied(_)),
        "an Unsupported store must never mint strong custody"
    );
    assert_eq!(store.writes.load(Ordering::SeqCst), 0);
    Ok(())
}

async fn retain(
    store: &dyn CommandLaunchStore,
    record: &LeaseRecord,
    launch_key: &str,
) -> TestResult<CommandCancellationAuthority> {
    let launch_key = key(launch_key)?;
    let fingerprint = fingerprint(&command(&record.lease_id, launch_key.as_str())?)?;
    assert_eq!(
        store.claim(&record.lease_id, &launch_key, &fingerprint).await?,
        Claim::Claimed
    );
    let authority = CommandCancellationAuthority::for_launch(record, &launch_key, &fingerprint)
        .context("the assigned record yields cancellation authority")?;
    assert_eq!(
        store
            .retain_cancellation_authority(&record.lease_id, &launch_key, &authority)
            .await?,
        CancellationAuthorityClaim::New
    );
    Ok(authority)
}

fn launch_fingerprint(record: &LeaseRecord, launch_key: &str) -> TestResult<String> {
    Ok(fingerprint(&command(&record.lease_id, launch_key)?)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_jetstream_reconstruction_keeps_custody_after_reply_cleanup() -> TestResult {
    let (harness, record) = fixture(Arc::new(InMemoryCommandLaunchStore::new())).await?;
    let js = jetstream::new(harness.client.clone());
    let store = NatsKvCommandLaunchStore::new(js.clone(), "retained-authority", 1).await?;
    let authority = retain(&store, &record, "first-launch").await?;
    let fresh = harness.server.client().await;
    let reopened = NatsKvCommandLaunchStore::new(jetstream::new(fresh), "retained-authority", 1).await?;
    let first = key("first-launch")?;
    assert_eq!(
        reopened.cancellation_authority(&record.lease_id, &first).await?,
        Some(authority.clone())
    );
    assert!(authority.matches_record(&record));
    let done = CommandLaunch::new(
        launch_fingerprint(&record, "first-launch")?,
        LaunchState::Done {
            response: serde_json::from_value(
                json!({"outcome":"started", "execution_id":"synthetic-retained", "origin":"created"}),
            )?,
        },
    );
    reopened.finish(&record.lease_id, &first, &done).await?;
    assert!(
        reopened
            .replace(&record.lease_id, &first, &done, &done.retired())
            .await?
    );
    assert_eq!(
        reopened.cancellation_authority(&record.lease_id, &first).await?,
        Some(authority.clone())
    );
    reopened.forget(&record.lease_id, &first).await?;
    assert_eq!(
        reopened
            .claim(&record.lease_id, &first, &launch_fingerprint(&record, "first-launch")?)
            .await?,
        Claim::Claimed
    );
    assert_eq!(
        reopened.cancellation_authority(&record.lease_id, &first).await?,
        Some(authority.clone())
    );
    assert_eq!(
        reopened
            .retain_cancellation_authority(&record.lease_id, &first, &authority)
            .await?,
        CancellationAuthorityClaim::Existing(authority)
    );
    for launch_key in ["second-launch", "third-launch"] {
        retain(&reopened, &record, launch_key).await?;
    }
    reopened.forget_lease(&record.lease_id).await?;
    for launch_key in ["first-launch", "second-launch", "third-launch"] {
        let launch_key_id = key(launch_key)?;
        assert_eq!(
            reopened
                .claim(
                    &record.lease_id,
                    &launch_key_id,
                    &launch_fingerprint(&record, launch_key)?
                )
                .await?,
            Claim::Claimed
        );
        let authority = reopened
            .cancellation_authority(&record.lease_id, &launch_key_id)
            .await?
            .context("custody survives the lease cleanup")?;
        assert!(authority.matches_record(&record));
        assert_eq!(
            reopened
                .retain_cancellation_authority(&record.lease_id, &launch_key_id, &authority)
                .await?,
            CancellationAuthorityClaim::Existing(authority)
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_stores_keep_custody_when_reply_records_expire() -> TestResult {
    let (harness, record) = fixture(Arc::new(InMemoryCommandLaunchStore::new())).await?;
    let memory = InMemoryCommandLaunchStore::with_max_age(Duration::from_millis(80));
    retain(&memory, &record, "expiring-launch").await?;
    let js = jetstream::new(harness.client.clone());
    let durable = NatsKvCommandLaunchStore::new(js.clone(), "expiring-authority", 1).await?;
    js.create_or_update_key_value(kv::Config {
        bucket: "expiring-authority".into(),
        history: 1,
        max_age: Duration::from_millis(200),
        ..Default::default()
    })
    .await?;
    retain(&durable, &record, "expiring-launch").await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let expiring = key("expiring-launch")?;
    for store in [&memory as &dyn CommandLaunchStore, &durable as &dyn CommandLaunchStore] {
        let authority = store
            .cancellation_authority(&record.lease_id, &expiring)
            .await?
            .context("custody outlives the reply record")?;
        assert_eq!(
            store
                .claim(
                    &record.lease_id,
                    &expiring,
                    &launch_fingerprint(&record, "expiring-launch")?
                )
                .await?,
            Claim::Claimed
        );
        assert_eq!(
            store
                .retain_cancellation_authority(&record.lease_id, &expiring, &authority)
                .await?,
            CancellationAuthorityClaim::Existing(authority)
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn durable_legacy_malformed_oversized_or_mismatched_entries_cannot_fabricate_authority() -> TestResult {
    let (harness, record) = fixture(Arc::new(InMemoryCommandLaunchStore::new())).await?;
    let js = jetstream::new(harness.client.clone());
    let store = NatsKvCommandLaunchStore::new(js.clone(), "authority-corruption", 1).await?;
    let authority = retain(&store, &record, "owned-launch").await?;
    let owned = key("owned-launch")?;
    let raw = js.get_key_value("authority-corruption-custody").await?;
    let mut keys = raw.keys().await?;
    let mut authority_key = None;
    while let Some(entry) = keys.next().await {
        let entry = entry?;
        if entry.ends_with(".owned-launch") {
            authority_key = Some(entry);
        }
    }
    let authority_key = authority_key.context("typed authority must have its separate namespace")?;
    raw.put(&authority_key, "{".into()).await?;
    assert!(store.cancellation_authority(&record.lease_id, &owned).await.is_err());
    raw.put(&authority_key, serde_json::to_vec(&authority)?.into()).await?;
    assert!(raw.put(&authority_key, vec![b'x'; 16 * 1024 + 1].into()).await.is_err());
    assert_eq!(
        store.cancellation_authority(&record.lease_id, &owned).await?,
        Some(authority.clone())
    );
    let mut mismatched = serde_json::to_value(&authority)?;
    mismatched["launch_key"] = "foreign-launch".into();
    raw.put(&authority_key, serde_json::to_vec(&mismatched)?.into()).await?;
    assert!(store.cancellation_authority(&record.lease_id, &owned).await.is_err());
    let mut unknown = serde_json::to_value(&authority)?;
    unknown["version"] = 2.into();
    raw.put(&authority_key, serde_json::to_vec(&unknown)?.into()).await?;
    assert!(store.cancellation_authority(&record.lease_id, &owned).await.is_err());
    raw.purge(&authority_key).await?;
    assert!(store.cancellation_authority(&record.lease_id, &owned).await.is_err());
    Ok(())
}

/// A live custody stream policy an operator could change to something unsafe.
#[derive(Clone, Copy, Debug)]
enum UnsafePolicy {
    Age,
    Eviction,
}

impl UnsafePolicy {
    const ALL: [Self; 2] = [Self::Age, Self::Eviction];

    fn apply(self, config: &mut stream::Config) {
        match self {
            Self::Age => config.max_age = Duration::from_secs(3600),
            Self::Eviction => {
                config.discard = stream::DiscardPolicy::Old;
                config.discard_new_per_subject = false;
            }
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Age => "age",
            Self::Eviction => "eviction",
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cached_custody_handles_refuse_live_unsafe_policy_without_rewriting_it() -> TestResult {
    let (harness, record) = fixture(Arc::new(InMemoryCommandLaunchStore::new())).await?;
    let js = jetstream::new(harness.client.clone());
    let store = NatsKvCommandLaunchStore::new(js.clone(), "live-authority-policy", 1).await?;
    let authority = retain(&store, &record, "owned-launch").await?;
    let owned = key("owned-launch")?;
    let raw = js.get_key_value("live-authority-policy-custody").await?;
    let mut native = raw.stream.clone();
    let original = native.info().await?.config.clone();
    for policy in UnsafePolicy::ALL {
        let mut changed = original.clone();
        policy.apply(&mut changed);
        js.update_stream(changed.clone()).await?;
        let mut accepted = js.get_stream(&changed.name).await?;
        assert_eq!(
            serde_json::to_value(&accepted.info().await?.config)?,
            serde_json::to_value(&changed)?
        );
        assert!(store.cancellation_authority(&record.lease_id, &owned).await.is_err());
        let new_name = format!("new-{}", policy.name());
        let new_key = key(&new_name)?;
        let fp = launch_fingerprint(&record, &new_name)?;
        assert_eq!(store.claim(&record.lease_id, &new_key, &fp).await?, Claim::Claimed);
        let new = CommandCancellationAuthority::for_launch(&record, &new_key, &fp)
            .context("the assigned record yields cancellation authority")?;
        assert!(
            store
                .retain_cancellation_authority(&record.lease_id, &new_key, &new)
                .await
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(&accepted.info().await?.config)?,
            serde_json::to_value(&changed)?,
            "a cached custody operation must not rewrite unsafe live policy"
        );
        js.update_stream(original.clone()).await?;
        assert_eq!(
            store.cancellation_authority(&record.lease_id, &owned).await?,
            Some(authority.clone())
        );
        assert_eq!(store.cancellation_authority(&record.lease_id, &new_key).await?, None);
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_unsafe_custody_bucket_blocks_strong_operations_without_blocking_reply_storage() -> TestResult {
    let (harness, record) = fixture(Arc::new(InMemoryCommandLaunchStore::new())).await?;
    let js = jetstream::new(harness.client.clone());
    let raw = js
        .create_key_value(kv::Config {
            bucket: "unsafe-authority-custody".into(),
            history: 1,
            max_age: Duration::from_secs(3600),
            max_bytes: 64 * 1024 * 1024,
            max_value_size: 16 * 1024,
            ..Default::default()
        })
        .await?;
    let store = NatsKvCommandLaunchStore::new(js.clone(), "unsafe-authority", 1).await?;
    let owned = key("owned-launch")?;
    let fp = launch_fingerprint(&record, "owned-launch")?;
    assert_eq!(store.claim(&record.lease_id, &owned, &fp).await?, Claim::Claimed);
    let authority = CommandCancellationAuthority::for_launch(&record, &owned, &fp)
        .context("the assigned record yields cancellation authority")?;
    assert!(
        store
            .retain_cancellation_authority(&record.lease_id, &owned, &authority)
            .await
            .is_err()
    );
    assert!(store.cancellation_authority(&record.lease_id, &owned).await.is_err());
    let mut native = raw.stream.clone();
    assert_eq!(native.info().await?.config.max_age, Duration::from_secs(3600));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordinary_custody_lookup_does_not_provision_a_missing_namespace() -> TestResult {
    let (harness, record) = fixture(Arc::new(InMemoryCommandLaunchStore::new())).await?;
    let js = jetstream::new(harness.client.clone());
    let store = NatsKvCommandLaunchStore::new(js.clone(), "noncreating-authority", 1).await?;
    assert_eq!(
        store
            .cancellation_authority(&record.lease_id, &key("ordinary-key")?)
            .await?,
        None
    );
    let Err(error) = js.get_stream("KV_noncreating-authority-custody").await else {
        return Err("a lookup must not create the custody stream".into());
    };
    assert!(
        matches!(error.kind(), jetstream::context::GetStreamErrorKind::JetStream(ref server)
        if server.error_code() == jetstream::ErrorCode::STREAM_NOT_FOUND)
    );
    Ok(())
}
