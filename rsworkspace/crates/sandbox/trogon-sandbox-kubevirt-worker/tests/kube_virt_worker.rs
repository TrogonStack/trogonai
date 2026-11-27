//! The KubeVirt worker over NATS with a scripted provider: lease dispatch, cancellation, port lookups, live
//! checkpoints, heartbeats, drain on shutdown, the authority host, and the warm pool upkeep.

mod fake_cluster;
mod guest_support;
mod support;

use std::collections::HashSet;
use std::future::pending;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_nats::Client;
use async_trait::async_trait;
use futures::StreamExt;
use guest_support::{Nats, TestResult};
use serde_json::{Value, json};
use support::{Boxed, ScriptedDiskSource, digest, ops, seed_warm_vm};
use trogon_sandbox_contract::{
    ArtifactType, LeaseId, LeaseRequest, LifecycleEvent, OciArtifactRef, ProviderCapabilityProfile, ProviderKind,
    ProviderLease, SandboxId, SnapshotReceipt, TerminalDimension, WorkerId,
};
use trogon_sandbox_kubevirt_worker::{
    KubeVirtWorker, KubeVirtWorkerError, LeaseHolding, NatsLifecycleEmitter, WarmPoolConfig, WarmPoolManager,
    WorkerAuthorityHost, WorkerProvider, profile,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityHost, AuthorityHostError, AuthorityMode, AuthorityResourceId, FreezeOutcome, HeldLease,
    LifecycleEmitter, Provider, ProviderError, PurgeOutcome, RedispatchGuard, SnapshotError, ThawOutcome,
    lifecycle_subject, subjects,
};
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError, GuestShell, TerminalHub};

use fake_cluster::{Effect, FakeCluster, Fault, Verb};

const WAIT: Duration = Duration::from_secs(20);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

async fn eventually(mut done: impl AsyncFnMut() -> bool) -> TestResult {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !done().await {
        if tokio::time::Instant::now() > deadline {
            return Err("the condition never held".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// A guest shell that holds nothing and records the leases it released.
#[derive(Default)]
struct StubShell {
    released: Mutex<Vec<LeaseId>>,
}

#[async_trait]
impl GuestShell for StubShell {
    async fn holds(&self, _lease_id: &LeaseId) -> bool {
        false
    }

    async fn open(
        &self,
        _lease_id: &LeaseId,
        _size: (TerminalDimension, TerminalDimension),
        _term: &str,
        _command: Option<String>,
    ) -> Result<GuestChannel, GuestError> {
        Err(GuestError::NoExitStatus)
    }

    async fn release(&self, lease_id: &LeaseId) {
        lock(&self.released).push(lease_id.clone());
    }
}

#[derive(Clone, Copy)]
enum Checkpoint {
    Receipt,
    Unsupported,
    Failure,
}

/// A provider with scripted behavior and counters.
struct FakeProvider {
    pool: Arc<WarmPoolManager>,
    leases: Mutex<HashSet<LeaseId>>,
    launched: AtomicU32,
    fail_launch: AtomicBool,
    fail_cancel: AtomicBool,
    hang_launch: AtomicBool,
    checkpoint: Mutex<Checkpoint>,
    purge: Mutex<PurgeOutcome>,
    held: Mutex<Vec<HeldLease>>,
}

impl FakeProvider {
    fn new(pool: Arc<WarmPoolManager>) -> Arc<Self> {
        Arc::new(Self {
            pool,
            leases: Mutex::default(),
            launched: AtomicU32::new(0),
            fail_launch: AtomicBool::new(false),
            fail_cancel: AtomicBool::new(false),
            hang_launch: AtomicBool::new(false),
            checkpoint: Mutex::new(Checkpoint::Receipt),
            purge: Mutex::new(PurgeOutcome::Purged),
            held: Mutex::default(),
        })
    }

    fn count(&self) -> u32 {
        u32::try_from(lock(&self.leases).len()).unwrap_or(u32::MAX)
    }
}

#[async_trait]
impl Provider for FakeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::KubeVirt
    }

    async fn lease(&self, _req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        if self.hang_launch.load(Ordering::SeqCst) {
            pending::<()>().await;
        }
        if self.fail_launch.load(Ordering::SeqCst) {
            return Err(ProviderError::backend("the cluster refused the VM"));
        }
        let number = self.launched.fetch_add(1, Ordering::SeqCst) + 1;
        lock(&self.leases).insert(lease_id.clone());
        let sandbox_id = SandboxId::new(format!("vm-{number}")).map_err(ProviderError::backend)?;
        Ok(ProviderLease {
            sandbox_id,
            evidence_subject: format!("kubevirt/{lease_id}"),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        })
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        if self.fail_cancel.load(Ordering::SeqCst) {
            return Err(ProviderError::backend("the VM could not be deleted"));
        }
        lock(&self.leases).remove(lease_id);
        Ok(())
    }

    async fn checkpoint_running(
        &self,
        _lease_id: &LeaseId,
        target: &OciArtifactRef,
    ) -> Result<SnapshotReceipt, ProviderError> {
        let script = *lock(&self.checkpoint);
        match script {
            Checkpoint::Receipt => Ok(SnapshotReceipt {
                digest: target.digest.clone(),
                bytes: 7,
                artifact_type: target.artifact_type,
            }),
            Checkpoint::Unsupported => Err(ProviderError::Snapshot(SnapshotError::Unsupported)),
            Checkpoint::Failure => Err(ProviderError::backend("the export failed")),
        }
    }
}

#[async_trait]
impl LeaseHolding for FakeProvider {
    async fn holds(&self, lease_id: &LeaseId) -> bool {
        lock(&self.leases).contains(lease_id)
    }
}

#[async_trait]
impl AuthorityHost for FakeProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(lock(&self.held).clone())
    }

    async fn purge(&self, _lease: &HeldLease) -> Result<PurgeOutcome, AuthorityHostError> {
        Ok(*lock(&self.purge))
    }
}

#[async_trait]
impl WorkerProvider for FakeProvider {
    fn profile(&self) -> ProviderCapabilityProfile {
        profile()
    }

    fn warm_pool(&self) -> Arc<WarmPoolManager> {
        self.pool.clone()
    }

    fn max_concurrent_leases(&self) -> u32 {
        3
    }

    async fn current_leases(&self) -> u32 {
        self.count()
    }
}

fn pool_over(cluster: &FakeCluster, warm: WarmPoolConfig) -> Boxed<Arc<WarmPoolManager>> {
    Ok(Arc::new(WarmPoolManager::new(
        warm,
        Arc::new(ops(cluster)?),
        Arc::new(ScriptedDiskSource::default()),
    )))
}

fn disabled() -> WarmPoolConfig {
    WarmPoolConfig {
        enabled: false,
        ..WarmPoolConfig::default()
    }
}

struct Running {
    provider: Arc<FakeProvider>,
    shell: Arc<StubShell>,
    _nats: Nats,
    client: Client,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), KubeVirtWorkerError>>>,
}

impl Running {
    async fn start() -> Boxed<Self> {
        let cluster = FakeCluster::new();
        let pool = pool_over(&cluster, disabled())?;
        Self::start_with(FakeProvider::new(pool), None).await
    }

    async fn start_with(provider: Arc<FakeProvider>, drain: Option<Duration>) -> Boxed<Self> {
        let nats = Nats::start().await;
        let client = nats.client().await?;
        let shell = Arc::new(StubShell::default());
        let terminals = Arc::new(TerminalHub::new(shell.clone(), client.clone()));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let mut worker = KubeVirtWorker::new(
            client.clone(),
            provider.clone(),
            terminals.clone(),
            WorkerId::new("worker-1")?,
            AuthorityConfig::new(AuthorityMode::Off),
        );
        if let Some(drain) = drain {
            worker = worker.with_drain_timeout(drain);
        }
        let task = tokio::spawn(async move {
            worker
                .run(async {
                    let _ = stopped.await;
                })
                .await
        });
        // The worker subscribes before it serves, so wait until its subjects have listeners.
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(Self {
            provider,
            shell,
            _nats: nats,
            client,
            stop: Some(stop),
            task: Some(task),
        })
    }

    async fn stop(&mut self) -> TestResult {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }

    async fn ask(&self, subject: String, payload: &Value) -> Boxed<Value> {
        let reply =
            tokio::time::timeout(WAIT, self.client.request(subject, serde_json::to_vec(payload)?.into())).await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    async fn dispatch(&self, lease: &str) -> Boxed<Value> {
        self.ask(
            dispatch_subject(),
            &json!({"lease_id": lease, "request": request_json()?}),
        )
        .await
    }

    /// Whether nothing answers a request quickly.
    async fn silence(&self, subject: String, payload: Vec<u8>) -> bool {
        tokio::time::timeout(Duration::from_millis(700), self.client.request(subject, payload.into()))
            .await
            .is_err()
    }
}

fn dispatch_subject() -> String {
    subjects::provider_dispatch(ProviderKind::KubeVirt)
}

fn domain_subject(lease: &str) -> String {
    subjects::provider_get_domain(ProviderKind::KubeVirt, &LeaseId::new(lease))
}

fn checkpoint_subject(lease: &str) -> String {
    subjects::provider_checkpoint_running(ProviderKind::KubeVirt, &LeaseId::new(lease))
}

fn cancel_subject(lease: &str) -> String {
    subjects::provider_cancel(ProviderKind::KubeVirt, &LeaseId::new(lease))
}

fn request_json() -> Boxed<Value> {
    Ok(json!({
        "tenant": "test",
        "provider_pref": [serde_json::to_value(ProviderKind::KubeVirt)?],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": {"registry": "docker.io", "repository": "library/ubuntu", "tag": "24.04"},
            },
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {"KEY": "value"},
        },
        "ttl_seconds": 60,
        "idempotency_key": "test",
        "persistent": false,
    }))
}

async fn statuses(events: &mut async_nats::Subscriber, count: usize) -> Boxed<Vec<String>> {
    let mut statuses = Vec::new();
    while statuses.len() < count {
        let event = tokio::time::timeout(WAIT, events.next()).await?.ok_or("no event")?;
        let event: Value = serde_json::from_slice(&event.payload)?;
        statuses.push(event["status"].as_str().unwrap_or_default().to_owned());
    }
    Ok(statuses)
}

#[tokio::test]
async fn a_dispatched_lease_is_granted_once_and_reports_its_lifecycle() -> TestResult {
    let mut running = Running::start().await?;
    let mut events = running
        .client
        .subscribe(lifecycle_subject(&LeaseId::new("lease-1")))
        .await?;
    running.client.flush().await?;

    let reply = running.dispatch("lease-1").await?;
    assert_eq!(reply["sandbox_id"], "vm-1");
    let again = running.dispatch("lease-1").await?;
    assert_eq!(again["sandbox_id"], "vm-1");
    assert_eq!(running.provider.launched.load(Ordering::SeqCst), 1);
    assert_eq!(
        statuses(&mut events, 4).await?,
        ["started", "phase_started", "phase_completed", "completed"]
    );
    running.stop().await
}

#[tokio::test]
async fn a_failed_launch_is_reported_to_the_broker_and_the_lifecycle() -> TestResult {
    let mut running = Running::start().await?;
    let mut events = running
        .client
        .subscribe(lifecycle_subject(&LeaseId::new("lease-1")))
        .await?;
    running.client.flush().await?;
    running.provider.fail_launch.store(true, Ordering::SeqCst);

    let reply = running.dispatch("lease-1").await?;
    assert!(reply["provider_error"].is_string());
    assert_eq!(
        statuses(&mut events, 4).await?,
        ["started", "phase_started", "phase_failed", "failed"]
    );
    running.stop().await
}

#[tokio::test]
async fn a_lease_id_that_is_no_sandbox_id_skips_the_placeholder_events() -> TestResult {
    let mut running = Running::start().await?;
    running.provider.fail_launch.store(true, Ordering::SeqCst);
    let reply = running.dispatch("bad\u{1}lease").await?;
    assert!(reply["provider_error"].is_string());
    running.provider.fail_launch.store(false, Ordering::SeqCst);
    let granted = running.dispatch("bad\u{1}lease").await?;
    assert!(granted["sandbox_id"].is_string());
    running.stop().await
}

#[tokio::test]
async fn a_dispatch_that_cannot_be_decoded_is_rejected() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running.ask(dispatch_subject(), &json!({"unexpected": true})).await?;
    assert!(reply["decode_error"].is_string());
    running.client.publish(dispatch_subject(), "{}".into()).await?;
    assert_eq!(running.provider.launched.load(Ordering::SeqCst), 0);
    running.stop().await
}

#[tokio::test]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    let payload = serde_json::to_vec(&json!({"lease_id": "lease-1", "request": request_json()?}))?;
    running.client.publish(dispatch_subject(), payload.into()).await?;
    eventually(async || running.provider.count() == 1).await?;
    running.stop().await
}

#[tokio::test]
async fn a_cancel_ends_the_lease_and_a_failed_cancel_keeps_it() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch("lease-1").await?;
    running.dispatch("lease-2").await?;

    running.provider.fail_cancel.store(true, Ordering::SeqCst);
    running.client.publish(cancel_subject("lease-2"), "".into()).await?;
    running.client.flush().await?;
    eventually(async || lock(&running.shell.released).contains(&LeaseId::new("lease-2"))).await?;
    assert_eq!(running.provider.count(), 2);
    assert_eq!(running.dispatch("lease-2").await?["sandbox_id"], "vm-2");

    running.provider.fail_cancel.store(false, Ordering::SeqCst);
    running.client.publish(cancel_subject("lease-1"), "".into()).await?;
    running.client.publish(cancel_subject("unknown"), "".into()).await?;
    eventually(async || running.provider.count() == 1).await?;

    let regranted = running.dispatch("lease-1").await?;
    assert_eq!(regranted["sandbox_id"], "vm-3");
    running.stop().await
}

#[tokio::test]
async fn a_held_lease_is_told_it_has_no_ports_and_a_stranger_gets_no_answer() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch("lease-1").await?;
    let reply = running
        .ask(domain_subject("lease-1"), &json!({"lease_id": "lease-1", "port": 8080}))
        .await?;
    assert!(reply.to_string().to_lowercase().contains("unsupported"));
    assert!(
        running
            .silence(
                domain_subject("other"),
                serde_json::to_vec(&json!({"lease_id": "other", "port": 8080}))?
            )
            .await
    );
    assert!(running.silence(domain_subject("lease-1"), b"not json".to_vec()).await);
    running.client.publish(domain_subject("lease-1"), "".into()).await?;
    running.client.flush().await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    running.stop().await
}

#[tokio::test]
async fn a_checkpoint_is_exported_or_refused_for_the_right_reason() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch("lease-1").await?;
    let target = json!({
        "registry": "zot.local",
        "repository": "snapshots/disk",
        "artifact_type": serde_json::to_value(ArtifactType::KubevirtDiskV1)?,
    });
    let ask = |extra: Value| {
        let mut request = json!({"lease_id": "lease-1", "target": target});
        if let (Some(request), Some(extra)) = (request.as_object_mut(), extra.as_object()) {
            request.extend(extra.clone());
        }
        request
    };

    let exported = running.ask(checkpoint_subject("lease-1"), &ask(json!({}))).await?;
    assert_eq!(exported["outcome"], "ok");
    assert_eq!(exported["bytes"], 7);
    let native = running
        .ask(checkpoint_subject("lease-1"), &ask(json!({"native_name": "snap"})))
        .await?;
    assert!(native.to_string().contains("registry"));

    *lock(&running.provider.checkpoint) = Checkpoint::Unsupported;
    let unsupported = running.ask(checkpoint_subject("lease-1"), &ask(json!({}))).await?;
    assert!(unsupported.to_string().contains("unsupported"));
    *lock(&running.provider.checkpoint) = Checkpoint::Failure;
    let failed = running.ask(checkpoint_subject("lease-1"), &ask(json!({}))).await?;
    assert!(failed.to_string().contains("transient"));

    let stranger = json!({"lease_id": "other", "target": target});
    assert!(
        running
            .silence(checkpoint_subject("other"), serde_json::to_vec(&stranger)?)
            .await
    );
    assert!(
        running
            .silence(checkpoint_subject("lease-1"), b"not json".to_vec())
            .await
    );
    running.client.publish(checkpoint_subject("lease-1"), "".into()).await?;
    running.client.flush().await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    running.stop().await
}

#[tokio::test]
async fn the_worker_announces_itself_while_it_serves() -> TestResult {
    let nats = Nats::start().await;
    let watcher = nats.client().await?;
    let mut heartbeats = watcher
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::KubeVirt))
        .await?;
    watcher.flush().await?;
    let cluster = FakeCluster::new();
    let pool = pool_over(&cluster, WarmPoolConfig::default())?;
    let provider = FakeProvider::new(pool);
    let terminals = Arc::new(TerminalHub::new(Arc::new(StubShell::default()), watcher.clone()));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let worker = KubeVirtWorker::new(
        nats.client().await?,
        provider,
        terminals,
        WorkerId::new("worker-1")?,
        AuthorityConfig::new(AuthorityMode::Off),
    );
    let task = tokio::spawn(async move {
        worker
            .run(async {
                let _ = stopped.await;
            })
            .await
    });
    let beat = tokio::time::timeout(WAIT, heartbeats.next())
        .await?
        .ok_or("no heartbeat")?;
    let beat: Value = serde_json::from_slice(&beat.payload)?;
    assert_eq!(beat["worker_id"], "worker-1");
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 3);
    assert!(beat["warm_pool_population"].is_null());
    let _ = stop.send(());
    task.await??;
    Ok(())
}

#[tokio::test]
async fn shutdown_abandons_requests_that_outlast_the_drain_timeout() -> TestResult {
    let cluster = FakeCluster::new();
    let provider = FakeProvider::new(pool_over(&cluster, disabled())?);
    provider.hang_launch.store(true, Ordering::SeqCst);
    let mut running = Running::start_with(provider, Some(Duration::from_millis(200))).await?;
    let payload = serde_json::to_vec(&json!({"lease_id": "lease-1", "request": request_json()?}))?;
    running.client.publish(dispatch_subject(), payload.into()).await?;
    running.client.flush().await?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    running.stop().await
}

#[tokio::test]
async fn the_warm_pool_is_reconciled_and_idle_vms_are_evicted() -> TestResult {
    let cluster = FakeCluster::new();
    seed_warm_vm(&cluster, "warm-1", &digest('a')?, Some(512))?;
    let warm = WarmPoolConfig {
        max_idle_seconds: 0,
        ..WarmPoolConfig::default()
    };
    let provider = FakeProvider::new(pool_over(&cluster, warm)?);
    let mut running = Running::start_with(provider, None).await?;
    eventually(async || cluster.get("virtualmachines", "warm-1").is_none()).await?;
    running.stop().await
}

#[tokio::test]
async fn a_failed_reconcile_is_only_logged_and_a_disabled_pool_is_left_alone() -> TestResult {
    let cluster = FakeCluster::new();
    cluster.inject(Fault::new(Verb::List, "virtualmachines", Effect::Reject(500)).always());
    let provider = FakeProvider::new(pool_over(&cluster, WarmPoolConfig::default())?);
    let mut running = Running::start_with(provider, None).await?;
    cluster.wait_for_requests(Verb::List, "virtualmachines", 1).await;
    running.stop().await?;

    let quiet = FakeCluster::new();
    let provider = FakeProvider::new(pool_over(&quiet, disabled())?);
    let mut running = Running::start_with(provider, None).await?;
    assert!(quiet.requests().is_empty());
    running.stop().await
}

fn terminals_over(shell: &Arc<StubShell>, client: Client) -> Arc<TerminalHub> {
    Arc::new(TerminalHub::new(shell.clone(), client))
}

#[tokio::test]
async fn the_authority_host_passes_through_and_cleans_up_what_it_purges() -> TestResult {
    let nats = Nats::start().await;
    let cluster = FakeCluster::new();
    let provider = FakeProvider::new(pool_over(&cluster, disabled())?);
    let lease_id = LeaseId::new("lease-1");
    let held = HeldLease::Provider {
        lease_id: lease_id.clone(),
        provider: ProviderKind::KubeVirt,
        resource_id: AuthorityResourceId::new("vm-1")?,
    };
    lock(&provider.held).push(held.clone());
    let shell = Arc::new(StubShell::default());
    let grants = Arc::new(RedispatchGuard::new());
    let host = WorkerAuthorityHost::new(
        provider.clone(),
        terminals_over(&shell, nats.client().await?),
        grants.clone(),
    );

    assert_eq!(host.held_leases().await?, vec![held.clone()]);
    assert_eq!(host.freeze(&held).await?, FreezeOutcome::Unsupported);
    assert_eq!(host.thaw(&held).await?, ThawOutcome::NoLongerOwned);

    let launches = AtomicU32::new(0);
    let launch = || async {
        launches.fetch_add(1, Ordering::SeqCst);
        provider
            .lease(
                &serde_json::from_value(request_json().map_err(|_| ProviderError::backend("bad request"))?)
                    .map_err(ProviderError::backend)?,
                &lease_id,
            )
            .await
    };
    grants.get_or_launch(&lease_id, launch).await?;

    *lock(&provider.purge) = PurgeOutcome::Stale;
    assert_eq!(host.purge(&held).await?, PurgeOutcome::Stale);
    assert!(lock(&shell.released).is_empty());
    grants.get_or_launch(&lease_id, launch).await?;
    assert_eq!(launches.load(Ordering::SeqCst), 1);

    for outcome in [PurgeOutcome::Purged, PurgeOutcome::Settled] {
        *lock(&provider.purge) = outcome;
        assert_eq!(host.purge(&held).await?, outcome);
    }
    assert_eq!(lock(&shell.released).len(), 2);
    grants.get_or_launch(&lease_id, launch).await?;
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn an_emitter_whose_client_is_gone_only_warns() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let emitter = NatsLifecycleEmitter::new(client.clone(), &LeaseId::new("lease-1"));
    client.drain().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    emitter
        .emit(LifecycleEvent::Cancelled {
            sandbox_id: SandboxId::new("vm-1")?,
        })
        .await;
    Ok(())
}

#[tokio::test]
async fn a_worker_whose_client_is_gone_cannot_subscribe() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let cluster = FakeCluster::new();
    let provider = FakeProvider::new(pool_over(&cluster, disabled())?);
    let shell = Arc::new(StubShell::default());
    let terminals = terminals_over(&shell, client.clone());
    client.drain().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let worker = KubeVirtWorker::new(
        client,
        provider,
        terminals,
        WorkerId::new("worker-1")?,
        AuthorityConfig::new(AuthorityMode::Off),
    );
    let outcome = worker.run(pending()).await;
    assert!(outcome.is_err());
    Ok(())
}
