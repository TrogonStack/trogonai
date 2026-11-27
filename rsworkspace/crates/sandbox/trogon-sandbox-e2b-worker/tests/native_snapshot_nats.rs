//! Native snapshots of running E2B sandboxes, driven over NATS against a loopback control plane and envd.

mod test_support;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    ArtifactType, CheckpointRunningRequest, KillCommandRequest, KillCommandResponse, LeaseId, LeaseRequest,
    LeaseRequestKey, ListProcessesResponse, NativeSnapshotReceipt, NativeSnapshotRef, OciImageRef, PollCommandResponse,
    ProcessInfo, ProcessState, ProviderKind, ProviderLease, RunCommandResponse, SandboxImage, SandboxSpec,
    SandboxSpecParts, SandboxTags, SnapshotExportOutcome, SnapshotPushTarget, TenantId, UnixSignal, WorkerId,
    WorkspaceSource,
};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals};
use trogon_sandbox_e2b_worker::{E2bApi, E2bConfig, E2bExec, E2bWorkerError, E2bWorkerProvider, Envd, run};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, Provider};
use url::Url;

const SNAPSHOT_BUILD_ID: &str = "11111111-2222-4333-8444-555555555555";
const TENANT: &str = "tenant-a";
const SOURCE: &str = "lease-source";

type Boxed = Box<dyn Error + Send + Sync>;
type TestResult<T = ()> = Result<T, Boxed>;

/// A mutex that stays usable after a holder panicked.
#[derive(Default)]
struct Shared<T>(Mutex<T>);

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Default)]
struct Control {
    creates: AtomicUsize,
    snapshots: AtomicUsize,
    snapshot_response: Shared<Option<Value>>,
    snapshot_template_id: Shared<Option<String>>,
    snapshot_requests: Shared<Vec<Value>>,
    template_reads: Shared<Vec<String>>,
    snapshot_delete_attempts: Shared<Vec<String>>,
    connects: AtomicUsize,
    deleted_restores: AtomicUsize,
    deleted_snapshots: AtomicUsize,
    fail_snapshot_delete: AtomicBool,
    refuse_snapshot_delete_with_running_forks: AtomicBool,
    active_snapshot_forks: Shared<HashSet<String>>,
    active_sandboxes: Shared<HashSet<String>>,
    sandbox_delete_pending: AtomicBool,
    observe_owned_cleanup_intent: AtomicBool,
    owned_cleanup_intent_seen: AtomicBool,
    custody_context: std::sync::OnceLock<async_nats::jetstream::Context>,
    snapshot_delete_pending: AtomicBool,
    snapshot_absent: AtomicBool,
    block_template_read: AtomicBool,
    template_read_started: Notify,
    template_read_release: Notify,
    require_fresh_source: AtomicBool,
    fail_connect: AtomicBool,
    mismatch_restore: AtomicBool,
    block_snapshot: AtomicBool,
    snapshot_started: Notify,
    snapshot_release: Notify,
    kill_bindings: Shared<Vec<(u32, u64, bool)>>,
    deadline_fixture: AtomicBool,
    process_running: Shared<HashMap<String, bool>>,
    snapshot_running: AtomicBool,
    fail_restored_delete: AtomicBool,
    delete_attempts: Shared<Vec<String>>,
    guest_calls: Shared<Vec<String>>,
    /// Requests that broke a contract the fixture expects the worker to keep.
    violations: AtomicUsize,
}

impl Control {
    fn is_running(&self, sandbox: &str) -> bool {
        self.process_running.lock().get(sandbox).copied().unwrap_or(false)
    }

    fn all_stopped(&self) -> bool {
        self.process_running.lock().values().all(|running| !running)
    }

    fn is_active(&self, sandbox: &str) -> bool {
        self.active_sandboxes.lock().contains(sandbox)
    }

    fn no_active(&self) -> bool {
        self.active_sandboxes.lock().is_empty()
    }

    fn delete_attempts(&self) -> Vec<String> {
        self.delete_attempts.lock().clone()
    }

    fn snapshot_delete_attempts(&self) -> Vec<String> {
        self.snapshot_delete_attempts.lock().clone()
    }

    fn violation(&self, broken: bool) {
        self.violations.fetch_add(usize::from(broken), Ordering::SeqCst);
    }
}

fn token_of(headers: &HeaderMap) -> &str {
    headers
        .get("x-access-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

fn sandbox_of(token: &str) -> &str {
    let token = token.strip_prefix("fixture-").unwrap_or(token);
    token.strip_suffix("-current").unwrap_or(token)
}

async fn create(State(control): State<Arc<Control>>, Json(body): Json<Value>) -> Json<Value> {
    let ordinal = control.creates.fetch_add(1, Ordering::SeqCst);
    let template = body["templateID"].as_str().unwrap_or_default();
    let snapshot = format!("{}:{SNAPSHOT_BUILD_ID}", snapshot_template_id(&control));
    let id = if ordinal == 0 {
        control.violation(template != "base");
        "source".to_string()
    } else if body["metadata"]["fixture-role"] == "other-lease" {
        control.violation(template != "base");
        format!("other-{ordinal}")
    } else {
        control.violation(template != snapshot);
        control
            .active_snapshot_forks
            .lock()
            .insert(format!("restored-{ordinal}"));
        format!("restored-{ordinal}")
    };
    if control.deadline_fixture.load(Ordering::SeqCst) {
        let running = control.snapshot_running.load(Ordering::SeqCst);
        control.process_running.lock().insert(id.clone(), running);
    }
    control.active_sandboxes.lock().insert(id.clone());
    Json(json!({"sandboxID":id,"envdAccessToken":format!("fixture-{id}")}))
}

async fn snapshot(State(control): State<Arc<Control>>, Json(body): Json<Value>) -> Response {
    control.snapshots.fetch_add(1, Ordering::SeqCst);
    control.snapshot_absent.store(false, Ordering::SeqCst);
    control.snapshot_requests.lock().push(body);
    if control.block_snapshot.load(Ordering::SeqCst) {
        control.snapshot_started.notify_one();
        control.snapshot_release.notified().await;
    }
    if control.deadline_fixture.load(Ordering::SeqCst) {
        control
            .snapshot_running
            .store(control.is_running("source"), Ordering::SeqCst);
    }
    Json(snapshot_response(&control)).into_response()
}

fn snapshot_template_id(control: &Control) -> String {
    control
        .snapshot_template_id
        .lock()
        .clone()
        .unwrap_or_else(|| "snapshot-1".into())
}

fn snapshot_response(control: &Control) -> Value {
    control
        .snapshot_response
        .lock()
        .clone()
        .unwrap_or_else(|| json!({"snapshotID":"snapshot-1:default","names":[]}))
}

async fn snapshot_template(State(control): State<Arc<Control>>, Path(id): Path<String>) -> Response {
    control.template_reads.lock().push(id.clone());
    if control.block_template_read.load(Ordering::SeqCst) {
        control.template_read_started.notify_one();
        control.template_read_release.notified().await;
    }
    if id != snapshot_template_id(&control) || control.snapshot_absent.load(Ordering::SeqCst) {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(json!({
        "templateID":id,"buildID":SNAPSHOT_BUILD_ID,"buildCount":1,
        "builds":[{"buildID":SNAPSHOT_BUILD_ID,"status":"ready"}],
    }))
    .into_response()
}

async fn remove_snapshot(State(control): State<Arc<Control>>, Path(id): Path<String>) -> Response {
    control.snapshot_delete_attempts.lock().push(id.clone());
    let raw_id = snapshot_template_id(&control);
    if control.fail_snapshot_delete.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if control.refuse_snapshot_delete_with_running_forks.load(Ordering::SeqCst)
        && !control.active_snapshot_forks.lock().is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message":"cannot delete template because there are running sandboxes using it"})),
        )
            .into_response();
    }
    let response = snapshot_response(&control);
    let names_empty = response["names"].as_array().is_some_and(|names| names.is_empty());
    control.violation(id != raw_id || response["snapshotID"] != format!("{raw_id}:default") || !names_empty);
    control.deleted_snapshots.fetch_add(1, Ordering::SeqCst);
    let pending = control.snapshot_delete_pending.load(Ordering::SeqCst);
    control.snapshot_absent.fetch_or(!pending, Ordering::SeqCst);
    StatusCode::NO_CONTENT.into_response()
}

async fn connect(State(control): State<Arc<Control>>) -> Response {
    control.connects.fetch_add(1, Ordering::SeqCst);
    if control.fail_connect.load(Ordering::SeqCst) {
        return (StatusCode::SERVICE_UNAVAILABLE, "fixture resume failed").into_response();
    }
    control.require_fresh_source.store(true, Ordering::SeqCst);
    Json(json!({"sandboxID":"source","envdAccessToken":"fixture-source-current","trafficAccessToken":"fixture-traffic-current"})).into_response()
}

/// Whether a lease cleanup record naming `sandbox` is durable and in the cleanup phase.
async fn cleanup_intent_recorded(context: &async_nats::jetstream::Context, sandbox: &str) -> bool {
    let found = async {
        let store = context.get_key_value("sandbox-e2b-lease-cleanup").await.ok()?;
        let mut keys = store.keys().await.ok()?;
        let mut seen = false;
        while let Some(Ok(key)) = keys.next().await {
            let record = store.get(key).await.ok().flatten();
            let record = record.and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            seen |= record.is_some_and(|record| {
                record["tenant"] == TENANT
                    && record["lease_id"] == SOURCE
                    && record["sandbox_id"] == sandbox
                    && record["phase"] == "cleanup"
            });
        }
        Some(seen)
    };
    found.await.unwrap_or(false)
}

async fn remove(State(control): State<Arc<Control>>, Path(id): Path<String>) -> StatusCode {
    control.delete_attempts.lock().push(id.clone());
    if control.observe_owned_cleanup_intent.load(Ordering::SeqCst)
        && let Some(context) = control.custody_context.get()
    {
        let seen = cleanup_intent_recorded(context, &id).await;
        control.owned_cleanup_intent_seen.fetch_or(seen, Ordering::SeqCst);
    }
    let pending = control.sandbox_delete_pending.load(Ordering::SeqCst);
    let restored = id.starts_with("restored-");
    let refused = restored && control.fail_restored_delete.load(Ordering::SeqCst);
    if pending {
        return StatusCode::NO_CONTENT;
    }
    if refused {
        return StatusCode::SERVICE_UNAVAILABLE;
    }
    control
        .deleted_restores
        .fetch_add(usize::from(restored), Ordering::SeqCst);
    control.active_snapshot_forks.lock().remove(&id);
    if control.deadline_fixture.load(Ordering::SeqCst) {
        control.process_running.lock().insert(id.clone(), false);
    }
    control.active_sandboxes.lock().remove(&id);
    StatusCode::NO_CONTENT
}

async fn sandbox_detail(State(control): State<Arc<Control>>, Path(id): Path<String>) -> Response {
    match control.is_active(&id) {
        true => Json(json!({"state":"running"})).into_response(),
        false => StatusCode::NOT_FOUND.into_response(),
    }
}

fn stream(stdout: &[u8]) -> Response {
    stream_result(stdout, 0)
}

fn stream_result(stdout: &[u8], exit_code: i32) -> Response {
    let mut bytes = Vec::new();
    for (flag, value) in [
        (0u8, json!({"event":{"start":{"pid":1}}})),
        (
            0,
            json!({"event":{"data":{"stdout":base64::engine::general_purpose::STANDARD.encode(stdout)}}}),
        ),
        (0, json!({"event":{"end":{"exited":true,"exitCode":exit_code}}})),
        (2, json!({})),
    ] {
        let value = serde_json::to_vec(&value).unwrap_or_default();
        bytes.push(flag);
        bytes.extend_from_slice(&u32::try_from(value.len()).unwrap_or_default().to_be_bytes());
        bytes.extend(value);
    }
    ([("content-type", "application/connect+json")], bytes).into_response()
}

async fn process(State(control): State<Arc<Control>>, headers: HeaderMap, body: Bytes) -> Response {
    let token = token_of(&headers);
    control.violation(control.require_fresh_source.load(Ordering::SeqCst) && token == "fixture-source");
    let command: Value = body
        .get(5..)
        .and_then(|frame| serde_json::from_slice(frame).ok())
        .unwrap_or_default();
    let argv: Vec<String> = serde_json::from_value(command["process"]["args"].clone()).unwrap_or_default();
    let sandbox_id = sandbox_of(token);
    control.guest_calls.lock().push(sandbox_id.into());
    if let [.., signal, pid, started] = argv.as_slice()
        && argv.len() >= 4
        && signal == UnixSignal::Kill.as_str()
    {
        if control.deadline_fixture.load(Ordering::SeqCst) {
            control.process_running.lock().insert(sandbox_id.into(), false);
        }
        control.kill_bindings.lock().push((
            pid.parse().unwrap_or_default(),
            started.parse().unwrap_or_default(),
            token.starts_with("fixture-restored-"),
        ));
        return stream(b"");
    }
    let args = command["process"]["args"].to_string();
    let launches = args.contains("nohup setsid");
    let identity = launches || args.contains("/proc/");
    if control.deadline_fixture.load(Ordering::SeqCst) {
        if launches {
            control.process_running.lock().insert(sandbox_id.into(), true);
        } else if identity && !control.is_running(sandbox_id) {
            return stream_result(b"", 44);
        }
    }
    let mismatched = token.starts_with("fixture-restored-") && control.mismatch_restore.load(Ordering::SeqCst);
    match (identity, mismatched) {
        (true, true) => stream(b"123 999\n"),
        (true, false) => stream(b"123 456\n"),
        (false, _) => stream(b""),
    }
}

async fn file(
    State(control): State<Arc<Control>>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    let token = token_of(&headers);
    control.violation(control.require_fresh_source.load(Ordering::SeqCst) && token == "fixture-source");
    let exited = control.deadline_fixture.load(Ordering::SeqCst)
        && query.get("path").is_some_and(|path| path.ends_with("/exit"))
        && !control.is_running(sandbox_of(token));
    match exited {
        true => (StatusCode::OK, "137\n").into_response(),
        false => StatusCode::NOT_FOUND.into_response(),
    }
}

fn lease_request_with(tenant: &str, image: SandboxImage) -> TestResult<LeaseRequest> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image,
        workspace: WorkspaceSource::empty(),
        env: Default::default(),
        cpu_seconds: std::num::NonZeroU32::MIN.saturating_add(3599),
        memory_mib: std::num::NonZeroU32::MIN.saturating_add(511),
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
    })?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new(tenant)?,
        provider_pref: vec![ProviderKind::E2b],
        spec,
        ttl_seconds: 600,
        idempotency_key: LeaseRequestKey::new("native-snapshot")?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: Default::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: Default::default(),
        native: None,
        region: None,
        fallback_regions: vec![],
    })
}

fn lease_request(tenant: &str) -> TestResult<LeaseRequest> {
    let image = SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified("base"),
        pull_secret: None,
    };
    lease_request_with(tenant, image)
}

/// A request the fixture answers with a sandbox other than the snapshot source.
fn sibling_request(tenant: &str) -> TestResult<LeaseRequest> {
    let mut request = lease_request(tenant)?;
    request.tags = serde_json::from_value(json!({"fixture-role":"other-lease"}))?;
    Ok(request)
}

/// The receipt of a native capture, or an error when the outcome was anything else.
fn native(outcome: &SnapshotExportOutcome) -> TestResult<NativeSnapshotReceipt> {
    Ok(serde_json::from_value(serde_json::to_value(outcome)?)?)
}

/// The execution id of a response that started a detached command.
fn started(response: &RunCommandResponse) -> TestResult<trogon_sandbox_contract::ExecutionId> {
    let wire = serde_json::to_value(response)?;
    Ok(trogon_sandbox_contract::ExecutionId::new(
        wire["execution_id"].as_str().ok_or("the command did not start")?,
    ))
}

fn checkpoint_request(name: &str) -> CheckpointRunningRequest {
    CheckpointRunningRequest {
        lease_id: LeaseId::new(SOURCE),
        target: SnapshotPushTarget {
            registry: "unused.invalid".into(),
            repository: "snapshots".into(),
            artifact_type: ArtifactType::SandboxRootfsV1,
        },
        native_name: Some(name.into()),
    }
}

/// A worker serving a provider until dropped.
struct Served {
    cancel: CancellationToken,
    worker: tokio::task::JoinHandle<Result<(), E2bWorkerError>>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.worker.abort();
    }
}

/// Starts a worker named `name` for `provider` and waits until it is subscribed.
async fn serve(
    client: &async_nats::Client,
    config: &E2bConfig,
    provider: Option<Arc<E2bWorkerProvider>>,
    name: &str,
) -> TestResult<(Arc<E2bWorkerProvider>, Served)> {
    let envd = Envd::new(Duration::from_secs(5))?;
    let provider = match provider {
        Some(provider) => provider,
        None => Arc::new(E2bWorkerProvider::new(
            E2bApi::new(config.clone(), Duration::from_secs(5))?,
            "base",
            Arc::new(E2bExec::new(envd.clone())),
        )),
    };
    let cancel = CancellationToken::new();
    let terminals = Arc::new(E2bTerminals::new(
        Arc::new(E2bGuestShell::new(provider.clone(), envd)),
        client.clone(),
    ));
    let worker_id = WorkerId::new(name)?;
    let (run_client, run_provider, run_cancel) = (client.clone(), provider.clone(), cancel.clone());
    let worker = tokio::spawn(async move {
        run(
            run_client.clone(),
            async_nats::jetstream::new(run_client),
            run_provider,
            &worker_id,
            run_cancel,
            terminals,
            AuthorityConfig::new(AuthorityMode::Off),
        )
        .await
    });
    let served = Served { cancel, worker };
    tokio::time::timeout(Duration::from_secs(10), provider.wait_ready()).await?;
    Ok((provider, served))
}

struct Rig {
    _nats: test_support::LocalNats,
    client: async_nats::Client,
    provider: Arc<E2bWorkerProvider>,
    config: E2bConfig,
    control: Arc<Control>,
    _worker: Option<Served>,
    http: tokio::task::JoinHandle<()>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.http.abort();
    }
}

impl Rig {
    async fn new() -> TestResult<Self> {
        Self::with_manifest_storage(true).await
    }

    /// Without manifest storage the worker is not started, so the provider has no durable custody.
    async fn with_manifest_storage(configure: bool) -> TestResult<Self> {
        let nats = test_support::LocalNats::start().await;
        let client = nats.client.clone();
        let control = Arc::new(Control::default());
        control
            .custody_context
            .set(async_nats::jetstream::new(client.clone()))
            .map_err(|_| "the custody context is set once")?;
        let router = Router::new()
            .route("/v2/sandboxes", post(create))
            .route("/sandboxes/source/snapshots", post(snapshot))
            .route("/v2/sandboxes/source/connect", post(connect))
            .route("/sandboxes/{id}", get(sandbox_detail).delete(remove))
            .route("/templates/{*id}", get(snapshot_template).delete(remove_snapshot))
            .route("/process.Process/Start", post(process))
            .route("/files", get(file))
            .with_state(control.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = Url::parse(&format!("http://{}", listener.local_addr()?))?;
        let http = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let config = E2bConfig::new(endpoint.clone(), "e2b.app", "fixture-key")?.with_envd_url(endpoint)?;
        let envd = Envd::new(Duration::from_secs(5))?;
        let provider = Arc::new(E2bWorkerProvider::new(
            E2bApi::new(config.clone(), Duration::from_secs(5))?,
            "base",
            Arc::new(E2bExec::new(envd)),
        ));
        let worker = match configure {
            true => Some(
                serve(&client, &config, Some(provider.clone()), "worker-native")
                    .await?
                    .1,
            ),
            false => None,
        };
        provider.lease(&lease_request(TENANT)?, &LeaseId::new(SOURCE)).await?;
        Ok(Self {
            _nats: nats,
            client,
            provider,
            config,
            control,
            _worker: worker,
            http,
        })
    }

    /// Stops the worker and starts another named `name` over a fresh provider.
    async fn restart(&mut self, name: &str) -> TestResult<(Arc<E2bWorkerProvider>, Served)> {
        self._worker = None;
        serve(&self.client, &self.config, None, name).await
    }

    fn api(&self) -> TestResult<E2bApi> {
        Ok(E2bApi::new(self.config.clone(), Duration::from_secs(5))?)
    }

    async fn call<T: DeserializeOwned>(&self, verb: &str, lease: &str, body: Value) -> TestResult<T> {
        let subject = format!("sandbox.provider.e2b.{verb}.{lease}");
        let reply = self.client.request(subject, serde_json::to_vec(&body)?.into()).await?;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    async fn launch(&self) -> TestResult<RunCommandResponse> {
        self.call(
            "run_command",
            SOURCE,
            json!({"lease_id":SOURCE,"command":["server"],"detached":true}),
        )
        .await
    }

    async fn capture(&self) -> TestResult<SnapshotExportOutcome> {
        let request = serde_json::to_value(checkpoint_request("snapshot-name"))?;
        self.call("checkpoint_running", SOURCE, request).await
    }

    async fn restore(&self, tenant: &str, lease: &str, reference: NativeSnapshotRef) -> TestResult<ProviderLease> {
        let request = lease_request_with(tenant, SandboxImage::NativeSnapshot { reference })?;
        Ok(self.provider.lease(&request, &LeaseId::new(lease)).await?)
    }

    async fn processes(&self, lease: &str) -> TestResult<Vec<ProcessInfo>> {
        let reply: Value = self.call("list_processes", lease, json!({"lease_id":lease})).await?;
        let reply: ListProcessesResponse = serde_json::from_value(reply)?;
        Ok(serde_json::from_value(
            serde_json::to_value(reply)?["processes"].clone(),
        )?)
    }

    async fn cancel(&self, lease: &str) -> TestResult {
        Ok(self.provider.cancel(&LeaseId::new(lease)).await?)
    }
}

#[tokio::test]
async fn native_memory_capture_preserves_unnamed_execution_identity_across_independent_forks() -> TestResult {
    let rig = Rig::new().await?;
    let execution_id = started(&rig.launch().await?)?;
    let receipt = native(&rig.capture().await?)?;
    assert_eq!(receipt.reference.provider, ProviderKind::E2b);
    assert!(receipt.reference.id.as_str().len() == 26);
    assert_eq!(rig.control.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(rig.control.connects.load(Ordering::SeqCst), 1);
    let source = rig.provider.sandbox_for_lease(&LeaseId::new(SOURCE)).await;
    let traffic = source.ok_or("the source lease is held")?.traffic_token;
    assert_eq!(traffic.as_deref(), Some("fixture-traffic-current"));
    let poll: PollCommandResponse = rig
        .call(
            "poll_command",
            SOURCE,
            json!({"lease_id":SOURCE,"execution_id":execution_id}),
        )
        .await?;
    assert!(matches!(poll, PollCommandResponse::Running));
    rig.restore(TENANT, "lease-fork-1", receipt.reference.clone()).await?;
    rig.restore(TENANT, "lease-fork-2", receipt.reference).await?;
    for lease in [SOURCE, "lease-fork-1", "lease-fork-2"] {
        let processes = rig.processes(lease).await?;
        assert_eq!(processes.len(), 1);
        assert_eq!(processes[0].execution_id, execution_id);
        assert_eq!(processes[0].state, ProcessState::Running);
        assert!(processes[0].name.is_none());
    }
    rig.cancel(SOURCE).await?;
    assert_eq!(rig.processes("lease-fork-1").await?[0].execution_id, execution_id);
    let poll: PollCommandResponse = rig
        .call(
            "poll_command",
            "lease-fork-2",
            json!({"lease_id":"lease-fork-2","execution_id":execution_id}),
        )
        .await?;
    assert!(matches!(poll, PollCommandResponse::Running));
    assert_eq!(rig.control.violations.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn snapshot_metadata_is_tenant_owned_and_unknown_or_foreign_references_do_not_create_sandboxes() -> TestResult {
    let rig = Rig::new().await?;
    let receipt = native(&rig.capture().await?)?;
    let wrong_tenant = rig.restore("tenant-b", "wrong-tenant", receipt.reference.clone()).await;
    assert!(wrong_tenant.is_err());
    let unknown = NativeSnapshotRef {
        id: trogon_sandbox_contract::NativeSnapshotId::new("unknown")?,
        ..receipt.reference.clone()
    };
    assert!(rig.restore(TENANT, "unknown", unknown).await.is_err());
    let foreign = NativeSnapshotRef {
        provider: ProviderKind::Modal,
        ..receipt.reference
    };
    assert!(rig.restore(TENANT, "foreign", foreign).await.is_err());
    assert_eq!(rig.control.creates.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn restored_process_start_identity_must_match_the_host_capture_before_adoption() -> TestResult {
    let rig = Rig::new().await?;
    started(&rig.launch().await?)?;
    let receipt = native(&rig.capture().await?)?;
    rig.control.mismatch_restore.store(true, Ordering::SeqCst);
    assert!(rig.restore(TENANT, "mismatched", receipt.reference).await.is_err());
    assert_eq!(rig.control.deleted_restores.load(Ordering::SeqCst), 1);
    assert!(
        rig.provider
            .sandbox_for_lease(&LeaseId::new("mismatched"))
            .await
            .is_none()
    );
    assert_eq!(rig.provider.active_lease_count().await, 1);
    Ok(())
}

#[tokio::test]
async fn capture_does_not_report_success_when_the_original_sandbox_cannot_be_resumed() -> TestResult {
    let rig = Rig::new().await?;
    rig.control.fail_connect.store(true, Ordering::SeqCst);
    assert!(matches!(rig.capture().await?, SnapshotExportOutcome::Err(_)));
    assert_eq!(rig.control.snapshots.load(Ordering::SeqCst), 1);
    assert_eq!(rig.control.connects.load(Ordering::SeqCst), 1);
    assert_eq!(rig.control.deleted_snapshots.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_new_worker_restores_only_durable_host_owned_snapshot_process_metadata() -> TestResult {
    let mut rig = Rig::new().await?;
    let execution_id = started(&rig.launch().await?)?;
    let receipt = native(&rig.capture().await?)?;
    let (provider, _restarted) = rig.restart("worker-native-restarted").await?;
    let request = lease_request_with(
        TENANT,
        SandboxImage::NativeSnapshot {
            reference: receipt.reference,
        },
    )?;
    provider.lease(&request, &LeaseId::new("lease-after-restart")).await?;
    let processes = rig.processes("lease-after-restart").await?;
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].execution_id, execution_id);
    assert_eq!(processes[0].state, ProcessState::Running);
    Ok(())
}

async fn assert_snapshot_isolation(other_tenant: &str) -> TestResult {
    let rig = Rig::new().await?;
    let initial = started(&rig.launch().await?)?;
    let other_request = sibling_request(other_tenant)?;
    rig.provider.lease(&other_request, &LeaseId::new("lease-other")).await?;
    rig.control.block_snapshot.store(true, Ordering::SeqCst);
    let client = rig.client.clone();
    let capture = tokio::spawn(async move {
        let request = serde_json::to_vec(&checkpoint_request("isolated"))?;
        let reply = client
            .request(
                format!("sandbox.provider.e2b.checkpoint_running.{SOURCE}"),
                request.into(),
            )
            .await?;
        Ok::<_, Boxed>(serde_json::from_slice::<SnapshotExportOutcome>(&reply.payload)?)
    });
    tokio::time::timeout(Duration::from_secs(2), rig.control.snapshot_started.notified()).await?;
    let launch = |client: async_nats::Client, lease: &'static str| {
        tokio::spawn(async move {
            let subject = format!("sandbox.provider.e2b.run_command.{lease}");
            let request = json!({"lease_id":lease,"command":["server"],"detached":true});
            let reply = client.request(subject, serde_json::to_vec(&request)?.into()).await?;
            Ok::<_, Boxed>(serde_json::from_slice::<RunCommandResponse>(&reply.payload)?)
        })
    };
    let other = launch(rig.client.clone(), "lease-other");
    let same = launch(rig.client.clone(), SOURCE);
    let other = tokio::time::timeout(Duration::from_secs(2), other).await;
    let other_completed = other.is_ok();
    let same_waited = !same.is_finished();
    rig.control.snapshot_release.notify_one();
    let outcome = capture.await??;
    let same = same.await??;
    let receipt = native(&outcome)?;
    assert!(
        other_completed,
        "a source snapshot blocked another lease's detached launch"
    );
    assert!(matches!(other???, RunCommandResponse::Started(_)));
    assert!(matches!(same, RunCommandResponse::Started(_)));
    assert!(same_waited, "source launch escaped the snapshot inventory barrier");
    rig.restore(TENANT, "lease-isolated-fork", receipt.reference).await?;
    let processes = rig.processes("lease-isolated-fork").await?;
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[0].execution_id, initial);
    Ok(())
}

#[tokio::test]
async fn a_snapshot_does_not_block_another_tenants_detached_launch() -> TestResult {
    assert_snapshot_isolation("tenant-b").await
}

#[tokio::test]
async fn a_snapshot_does_not_block_the_same_tenants_other_lease() -> TestResult {
    assert_snapshot_isolation(TENANT).await
}

#[path = "native_snapshot_nats/command_control.rs"]
mod command_control_tests;

#[path = "native_snapshot_nats/deadlines.rs"]
mod deadlines_tests;

#[path = "native_snapshot_nats/cleanup.rs"]
mod cleanup_tests;

#[path = "native_snapshot_nats/cleanup_retention.rs"]
mod cleanup_retention_tests;

#[path = "native_snapshot_nats/references.rs"]
mod references_tests;

#[path = "native_snapshot_nats/deletion.rs"]
mod deletion_tests;
