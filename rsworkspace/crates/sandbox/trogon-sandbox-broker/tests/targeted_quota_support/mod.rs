//! Shared body of the targeted quota tests: a real broker process and NATS server,
//! a synthetic worker that grants no native resource, and signed assigned dispatch.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::num::NonZeroU32;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::{Client, jetstream};
use bytes::Bytes;
use chrono::{Datelike, Utc};
use futures::StreamExt;
use tokio::sync::Mutex;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_broker::default_capability_profile::default_capability_profile;
use trogon_sandbox_broker::image_policy::{NatsKvImageAllowList, TenantImagePolicy};
use trogon_sandbox_broker::quota::{NatsKvQuotaStore, QuotaRecord, quota_kv_key_for_month};
use trogon_sandbox_contract::{
    CancelLeaseResponse, CapabilitySupport, DispatchAuth, IsolationTier, LeaseEpoch, LeaseId, LeasePersistence,
    LeaseRequest, LeaseRequestKey, LeaseResponse, ListCapabilitiesResponse, OciImageRef, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderDispatchPayload, ProviderKind, ProviderLease, SandboxId, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, TenantId, WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol,
    WorkerHeartbeat, WorkerId, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::{
    CAPABILITY_LIST_REQUEST, LEASE_REQUEST, lease_cancel, provider_dispatch, worker_cleanup, worker_dispatch,
    worker_heartbeat,
};
use trogon_sandbox_protocol::{
    AUTH_FIELD, DispatchSigner, DispatchVerifier, ENV_SIGNING_KEY, ENV_SIGNING_KEY_ID, EnforcementMode, EpochMode,
    OP_LEASE_DISPATCH,
};

/// An error that can cross a task boundary.
pub type Fault = Box<dyn Error + Send + Sync>;

const WAIT: Duration = Duration::from_secs(10);
const CPU_CAP: u32 = 3600;
const WALL_CAP: u32 = 7200;
const MEMORY_MIB: u32 = 512;
const TTL_SECONDS: u32 = 60;
const MAX_WORKER_LEASES: u32 = 4;
const SIGNING_KEY_ID: &str = "quota-proof";
const TENANT: &str = "assigned-quota-tenant";
const IMAGE: &str = "base";

/// What differs between the providers this scenario runs for.
pub struct Scenario {
    pub provider: ProviderKind,
    pub tier: IsolationTier,
    pub worker: &'static str,
    pub quota_bucket: &'static str,
    pub image_bucket: &'static str,
}

/// Kills the broker process when the test ends, however it ends.
struct Broker(Child);

impl Broker {
    /// How the broker ended, when it has.
    fn exit(&mut self) -> Option<String> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(error) => Some(error.to_string()),
        }
    }
}

impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Default)]
struct WorkerState {
    grants: Vec<(LeaseId, ProviderLease)>,
    cleanups: Vec<ProviderCleanupRequest>,
    held: HashMap<String, ProviderLease>,
}

/// A synthetic worker that verifies every signed message it receives. A mismatch ends
/// its task and is recorded as its fault, which the test reads back.
struct AssignedWorker {
    task: tokio::task::JoinHandle<()>,
    state: Arc<Mutex<WorkerState>>,
    fault: Arc<Mutex<Option<String>>>,
}

impl Drop for AssignedWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn worker_heartbeat_message(scenario: &Scenario, worker: &WorkerId, held: usize) -> WorkerHeartbeat {
    let mut profile = default_capability_profile();
    profile.provider = scenario.provider;
    profile.isolation.default_tier = scenario.tier;
    profile.isolation.supported_tiers = vec![scenario.tier];
    WorkerHeartbeat {
        worker_id: worker.clone(),
        dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
        supports_lease_epoch: CapabilitySupport::Supported,
        native_volume_bindings: Vec::new(),
        provider: scenario.provider,
        capacity: WorkerCapacity {
            max_concurrent_leases: MAX_WORKER_LEASES,
            current_leases: u32::try_from(held).unwrap_or(u32::MAX),
            capabilities: WorkerCapabilities::from(&profile),
        },
        emitted_at: SystemTime::now(),
        cached_digests: Vec::new(),
        profile,
        warm_pool_population: HashMap::new(),
    }
}

async fn handle_dispatch(
    client: &Client,
    verifier: &DispatchVerifier,
    scenario: &Scenario,
    worker: &WorkerId,
    state: &Mutex<WorkerState>,
    message: async_nats::Message,
) -> Result<(), Fault> {
    let payload: ProviderDispatchPayload = verifier.decode(&message.payload, OP_LEASE_DISPATCH, Some(worker))?;
    if message.subject.as_str() != worker_dispatch(scenario.provider, worker) {
        return Err("the dispatch did not use the worker specific subject".into());
    }
    if payload.request.provider_pref != [scenario.provider] {
        return Err("the dispatch lost its provider preference".into());
    }
    if payload.request.spec.cpu_seconds().get() != CPU_CAP
        || payload.request.spec.memory_mib().get() != MEMORY_MIB
        || payload.request.ttl_seconds != TTL_SECONDS
    {
        return Err("the dispatch changed the requested resources".into());
    }
    let target = payload.target.ok_or("assigned dispatch requires its identity")?;
    if &target.worker_id != worker || target.epoch != Some(LeaseEpoch::FIRST) {
        return Err("the dispatch target is not the first epoch of this worker".into());
    }
    let signed: serde_json::Value = serde_json::from_slice(&message.payload)?;
    let auth: DispatchAuth = serde_json::from_value(signed[AUTH_FIELD].clone())?;
    if auth.worker_id.as_ref() != Some(worker)
        || auth.attempt_id.as_ref() != Some(&target.attempt_id)
        || auth.epoch != target.epoch
    {
        return Err("the dispatch signature is not bound to its target".into());
    }
    let native = ProviderLease {
        sandbox_id: SandboxId::new(format!("stub-{}", payload.lease_id.as_str()))?,
        evidence_subject: format!("sandbox.evidence.{}", payload.lease_id.as_str()),
        worker_id: Some(target.worker_id),
        attempt_id: Some(target.attempt_id),
        epoch: target.epoch,
    };
    {
        let mut state = state.lock().await;
        state.grants.push((payload.lease_id.clone(), native.clone()));
        state.held.insert(payload.lease_id.as_str().to_string(), native.clone());
    }
    let reply = message.reply.ok_or("the dispatch has no reply subject")?;
    client.publish(reply, serde_json::to_vec(&native)?.into()).await?;
    Ok(())
}

async fn handle_cleanup(
    client: &Client,
    verifier: &DispatchVerifier,
    scenario: &Scenario,
    worker: &WorkerId,
    state: &Mutex<WorkerState>,
    message: async_nats::Message,
) -> Result<(), Fault> {
    let scope = verifier
        .admit_message_scope(message.subject.as_str(), message.headers.as_ref(), &message.payload)?
        .ok_or("cleanup must be signed")?;
    let request: ProviderCleanupRequest = serde_json::from_slice(&message.payload)?;
    let identity = &request.identity;
    if identity.provider != scenario.provider || &identity.target.worker_id != worker {
        return Err("the cleanup is not for this worker".into());
    }
    let scoped = scope.scope();
    if scoped.worker_id.as_ref() != Some(worker)
        || scoped.attempt_id.as_ref() != Some(&identity.target.attempt_id)
        || scoped.epoch != identity.target.epoch
    {
        return Err("the cleanup signature is not bound to its target".into());
    }
    {
        let mut state = state.lock().await;
        let native = state
            .held
            .remove(identity.lease_id.as_str())
            .ok_or("cleanup must own a held assignment")?;
        if native.attempt_id.as_ref() != Some(&identity.target.attempt_id) || native.epoch != identity.target.epoch {
            return Err("the cleanup does not match the held assignment".into());
        }
        state.cleanups.push(request.clone());
    }
    let response = ProviderCleanupResponse::Cleaned {
        identity: request.identity,
        intent: request.intent,
    };
    let reply = message.reply.ok_or("the cleanup has no reply subject")?;
    client.publish(reply, serde_json::to_vec(&response)?.into()).await?;
    Ok(())
}

async fn serve_worker(
    client: Client,
    verifier: DispatchVerifier,
    scenario: &Scenario,
    worker: WorkerId,
    state: Arc<Mutex<WorkerState>>,
    mut dispatch: async_nats::Subscriber,
    mut cleanup: async_nats::Subscriber,
) -> Result<(), Fault> {
    let mut heartbeat = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let held = state.lock().await.held.len();
                let message = worker_heartbeat_message(scenario, &worker, held);
                client
                    .publish(
                        worker_heartbeat(scenario.provider, &worker),
                        serde_json::to_vec(&message)?.into(),
                    )
                    .await?;
            }
            message = dispatch.next() => {
                let message = message.ok_or("the dispatch subscription closed")?;
                handle_dispatch(&client, &verifier, scenario, &worker, &state, message).await?;
            }
            message = cleanup.next() => {
                let message = message.ok_or("the cleanup subscription closed")?;
                handle_cleanup(&client, &verifier, scenario, &worker, &state, message).await?;
            }
        }
    }
}

impl AssignedWorker {
    async fn start(client: Client, public: &str, scenario: &'static Scenario) -> Result<Self, Fault> {
        let verifier = DispatchVerifier::new(EnforcementMode::Require)
            .with_key(SIGNING_KEY_ID, public)?
            .with_epoch_mode(EpochMode::Require);
        let worker = WorkerId::new(scenario.worker)?;
        let dispatch = client.subscribe(worker_dispatch(scenario.provider, &worker)).await?;
        let cleanup = client.subscribe(worker_cleanup(scenario.provider, &worker)).await?;
        client.flush().await?;
        let state = Arc::new(Mutex::new(WorkerState::default()));
        let fault = Arc::new(Mutex::new(None));
        let task = {
            let state = state.clone();
            let fault = fault.clone();
            tokio::spawn(async move {
                let outcome = serve_worker(client, verifier, scenario, worker, state, dispatch, cleanup).await;
                if let Err(error) = outcome {
                    *fault.lock().await = Some(error.to_string());
                }
            })
        };
        Ok(Self { task, state, fault })
    }

    /// The reason the worker stopped, once it has.
    async fn fault(&self) -> Option<String> {
        self.fault.lock().await.clone()
    }
}

async fn quota_record(quota: &NatsKvQuotaStore, tenant: &TenantId) -> Result<Option<QuotaRecord>, Fault> {
    let now = Utc::now();
    let bytes = quota
        .kv
        .get(quota_kv_key_for_month(tenant, now.year(), now.month()))
        .await?;
    match bytes {
        Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        None => Ok(None),
    }
}

fn quota_diagnostic(record: Option<&QuotaRecord>) -> String {
    match record {
        None => "record absent".to_string(),
        Some(record) => format!(
            "cpu {}/{} wall {}/{} active {}/{} pending {}",
            record.cpu_seconds_used,
            record.cap_cpu_seconds,
            record.wall_seconds_used,
            record.cap_wall_seconds,
            record.active_leases,
            record.active_lease_cap,
            record.pending_attempt.is_some(),
        ),
    }
}

fn usage(record: &QuotaRecord) -> (u32, u32, u32) {
    (record.cpu_seconds_used, record.wall_seconds_used, record.active_leases)
}

async fn cancel(client: &Client, id: &LeaseId) -> Result<CancelLeaseResponse, Fault> {
    let message = client
        .send_request(
            lease_cancel(id),
            async_nats::Request::new().payload(Bytes::new()).timeout(Some(WAIT)),
        )
        .await?;
    Ok(serde_json::from_slice(&message.payload)?)
}

fn request_spec() -> Result<SandboxSpec, Fault> {
    Ok(SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(IMAGE),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(CPU_CAP).ok_or("zero cpu")?,
        memory_mib: NonZeroU32::new(MEMORY_MIB).ok_or("zero memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })?)
}

fn lease_request(scenario: &Scenario, tenant: &TenantId) -> Result<LeaseRequest, Fault> {
    Ok(LeaseRequest {
        tenant: tenant.clone(),
        provider_pref: vec![scenario.provider],
        spec: request_spec()?,
        ttl_seconds: TTL_SECONDS,
        idempotency_key: LeaseRequestKey::new("finite-cap-assigned-create")?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: Default::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: LeasePersistence::Ephemeral,
        native: None,
        region: None,
        fallback_regions: Vec::new(),
        operation_id: None,
    })
}

fn spawn_broker(scenario: &Scenario, nats_url: &str, signing_seed: &str) -> Result<Broker, Fault> {
    let child = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-broker"))
        .env_clear()
        .env("NATS_URL", nats_url)
        .env("SANDBOX_AUTHZ_ENFORCE", "false")
        .env("SANDBOX_ENFORCE_IMAGE_ALLOWLIST", "true")
        .env("SANDBOX_ENFORCE_WORKER_REGISTRY", "true")
        .env(ENV_SIGNING_KEY, signing_seed)
        .env(ENV_SIGNING_KEY_ID, SIGNING_KEY_ID)
        .env("SANDBOX_QUOTA_BACKEND", "nats-kv")
        .env("SANDBOX_QUOTA_BUCKET", scenario.quota_bucket)
        .env("SANDBOX_IMAGE_ALLOWLIST_BUCKET", scenario.image_bucket)
        .env("SANDBOX_IDEMPOTENCY_BACKEND", "nats-kv")
        .env("SANDBOX_ENTITY_MODE", "off")
        .env("SANDBOX_DEFAULT_CPU_CAP", CPU_CAP.to_string())
        .env("SANDBOX_DEFAULT_WALL_CAP", WALL_CAP.to_string())
        .env("SANDBOX_JETSTREAM_REPLICAS", "1")
        .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(Broker(child))
}

/// Waits until the broker lists the worker's provider with the worker's isolation tier.
async fn wait_for_worker_profile(
    client: &Client,
    worker: &AssignedWorker,
    broker: &mut Broker,
    scenario: &Scenario,
) -> Result<(), Fault> {
    let waiting = async {
        loop {
            if let Ok(message) = client
                .send_request(
                    CAPABILITY_LIST_REQUEST,
                    async_nats::Request::new()
                        .payload(Bytes::new())
                        .timeout(Some(Duration::from_millis(300))),
                )
                .await
                && let Ok(response) = serde_json::from_slice::<ListCapabilitiesResponse>(&message.payload)
                && response.profiles.iter().any(|profile| {
                    profile.provider == scenario.provider
                        && profile.default_isolation_tier == scenario.tier
                        && profile.supported_isolation_tiers == [scenario.tier]
                })
            {
                return Ok(());
            }
            if let Some(fault) = worker.fault().await {
                return Err::<(), Fault>(format!("the synthetic worker stopped: {fault}").into());
            }
            if let Some(exit) = broker.exit() {
                return Err(format!("the broker exited: {exit}").into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(WAIT, waiting)
        .await
        .map_err(|_| "the broker never observed the synthetic worker heartbeat")?
}

/// Waits until the tenant's usage returns to zero.
async fn wait_for_refund(quota: &NatsKvQuotaStore, tenant: &TenantId) -> Result<(), Fault> {
    let waiting = async {
        loop {
            let record = quota_record(quota, tenant).await?.ok_or("the quota record vanished")?;
            if usage(&record) == (0, 0, 0) {
                return Ok::<(), Fault>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::time::timeout(WAIT, waiting)
        .await
        .map_err(|_| "exact assigned release must refund cpu, wall and active once")?
}

async fn run_inner(scenario: &'static Scenario) -> Result<(), Fault> {
    let server = JetStreamTestServer::start().await;
    let nats_url = format!("nats://{}", server.address());
    let client = server.client().await;
    let tenant = TenantId::new(TENANT)?;
    let quota = NatsKvQuotaStore::new(jetstream::new(client.clone()), scenario.quota_bucket).await?;
    let images = NatsKvImageAllowList::new(jetstream::new(client.clone()), scenario.image_bucket).await?;
    trogon_sandbox_broker::tenant_record::put(
        &images.kv,
        &tenant,
        serde_json::to_vec(&TenantImagePolicy::from_patterns(vec![IMAGE.to_string()]))?.into(),
    )
    .await?;
    let (seed, public) = DispatchSigner::generate_keypair_b64();
    let worker = AssignedWorker::start(client.clone(), &public, scenario).await?;
    let mut legacy_dispatch = client.subscribe(provider_dispatch(scenario.provider)).await?;
    client.flush().await?;
    let mut broker = spawn_broker(scenario, &nats_url, &seed)?;
    wait_for_worker_profile(&client, &worker, &mut broker, scenario).await?;
    if quota_record(&quota, &tenant).await?.is_some() {
        return Err("the tenant must start without usage".into());
    }

    let request = serde_json::to_vec(&lease_request(scenario, &tenant)?)?;
    let message = tokio::select! {
        biased;
        unexpected = legacy_dispatch.next() => {
            let unexpected = unexpected.ok_or("the legacy dispatch observer closed")?;
            let record = quota_record(&quota, &tenant).await?;
            return Err(format!(
                "an accepted assigned heartbeat must use worker specific dispatch; the broker used legacy subject {}, quota {}",
                unexpected.subject,
                quota_diagnostic(record.as_ref()),
            ).into());
        }
        message = client.send_request(
            LEASE_REQUEST,
            async_nats::Request::new().payload(request.into()).timeout(Some(WAIT)),
        ) => message?,
    };
    let response: LeaseResponse = serde_json::from_slice(&message.payload)?;
    let held = quota_record(&quota, &tenant).await?;
    let LeaseResponse::Granted(grant) = response else {
        return Err(format!(
            "one assigned lease at the exact finite cpu cap must grant without a second reservation, got {response:?}, quota {}, worker fault {:?}",
            quota_diagnostic(held.as_ref()),
            worker.fault().await,
        )
        .into());
    };
    assert_eq!(grant.provider, scenario.provider);
    assert!(grant.created.is_created());
    {
        let state = worker.state.lock().await;
        assert_eq!(state.grants.len(), 1);
        let (lease_id, native) = state.grants.first().ok_or("the worker saw no dispatch")?;
        assert_eq!(lease_id, &grant.lease_id);
        assert_eq!(native.sandbox_id.as_str(), grant.sandbox_id);
        assert_eq!(native.worker_id.as_ref().map(WorkerId::as_str), Some(scenario.worker));
        assert!(native.attempt_id.is_some());
        assert_eq!(native.epoch, Some(LeaseEpoch::FIRST));
    }
    let held = held.ok_or("successful quota admission must persist its finite record")?;
    assert_eq!(usage(&held), (CPU_CAP, TTL_SECONDS, 1));
    assert_eq!((held.cap_cpu_seconds, held.cap_wall_seconds), (CPU_CAP, WALL_CAP));
    assert!(held.pending_attempt.is_none());

    assert_eq!(cancel(&client, &grant.lease_id).await?, CancelLeaseResponse::Cancelled);
    wait_for_refund(&quota, &tenant).await?;
    let released = quota_record(&quota, &tenant)
        .await?
        .ok_or("the quota record vanished")?;
    assert_eq!(usage(&released), (0, 0, 0));
    assert_eq!(
        (released.cap_cpu_seconds, released.cap_wall_seconds),
        (CPU_CAP, WALL_CAP)
    );
    let attempt = {
        let state = worker.state.lock().await;
        assert_eq!(
            state.cleanups.len(),
            1,
            "the exact assigned cleanup is not re-executed after release"
        );
        assert!(state.held.is_empty());
        let (_, native) = state.grants.first().ok_or("the worker saw no dispatch")?;
        native.attempt_id.clone().ok_or("the grant carries no attempt")?
    };
    let receipt = quota
        .kv
        .get(format!("attempt.{}", attempt.as_str()))
        .await?
        .ok_or("the released attempt left no receipt")?;
    let receipt: serde_json::Value = serde_json::from_slice(&receipt)?;
    assert_eq!(receipt["phase"], "Released");
    assert_eq!(receipt["reservation"]["attempt_id"], attempt.as_str());
    assert_eq!(receipt["reservation"]["cpu_seconds"], CPU_CAP);
    assert_eq!(receipt["reservation"]["wall_seconds"], TTL_SECONDS);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), legacy_dispatch.next())
            .await
            .is_err(),
        "the legacy provider route must never receive an assigned request"
    );
    if let Some(fault) = worker.fault().await {
        return Err(format!("the synthetic worker stopped: {fault}").into());
    }
    Ok(())
}

/// Runs one lease through quota admission, assigned dispatch, release and refund.
pub async fn run(scenario: &'static Scenario) -> Result<(), Box<dyn Error>> {
    run_inner(scenario).await.map_err(|error| -> Box<dyn Error> { error })
}
