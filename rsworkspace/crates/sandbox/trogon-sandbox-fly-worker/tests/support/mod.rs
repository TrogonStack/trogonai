//! Shared fixtures: a stateful fake of the Fly Machines API, a throwaway NATS server, and request builders.

#![allow(dead_code)]

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_nats::Client;
use base64::Engine as _;
use serde_json::{Value, json};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_contract::{
    DriveId, DriveName, LeaseRequest, MountPath, ProviderCleanupRequest, ProviderDispatchPayload, TenantId,
};
use trogon_sandbox_fly_worker::{
    FlyApi, FlyApiToken, FlyAppName, FlyConfig, FlyConfigParts, FlyEndpoint, FlyPreviewNetwork, FlyProvider, FlyRegion,
    FlyVolumeBinding, FlyVolumeId, FlyWorker, FlyWorkerError,
};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::any};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type Boxed<T> = Result<T, Box<dyn std::error::Error>>;

pub const IMAGE: &str = "docker.io/library/ubuntu:24.04";
pub const APP: &str = "test-app";
pub const TOKEN: &str = "test-token";
pub const CAPTURE_REPLY: &str = "7\n2\nAP8=\n\n3\nZXJy\n\n";

/// One request the fake received.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub body: Value,
}

/// The fake's data and the failures it can be told to produce.
#[derive(Default)]
pub struct State {
    pub machines: Vec<Value>,
    pub volumes: Vec<Value>,
    pub calls: Vec<Call>,
    pub created: usize,
    pub volumes_created: usize,
    /// The state a freshly created Machine reports instead of `started`.
    pub boot_state: Option<String>,
    pub boot_fails: bool,
    pub capture_reply: Option<String>,
    pub read_exit_code: i32,
    pub read_reply: Option<String>,
    pub read_always_full: bool,
    pub mktemp_reply: Option<String>,
    /// Exec commands containing a needle exit with the given code.
    pub exec_exit: Vec<(String, i32)>,
    /// Exec commands containing a needle fail with HTTP 500.
    pub exec_http_fail: Vec<String>,
    pub exec_sleep: Option<Duration>,
    pub create_response_lost: bool,
    pub create_response_empty: bool,
    pub create_delay: Option<Duration>,
    pub create_status: Option<u16>,
    pub list_fails: bool,
    pub suspend_stops: bool,
    pub suspend_ignored: bool,
    pub start_fails: bool,
    pub start_ignored: bool,
    pub delete_failures: usize,
    pub volume_create_fails: bool,
    pub volume_create_lost: bool,
    pub volume_state: Option<String>,
    pub volume_delete_failures: usize,
    pub operator_volume: Option<Value>,
    pub snapshot_polls_before_ready: usize,
    pub snapshot_never_ready: bool,
    pub snapshot_status: Option<String>,
    pub snapshot_without_id: bool,
    pub snapshot_create_fails: bool,
    pub retention_days: Option<u64>,
}

struct Fake(Arc<Mutex<State>>);

fn reply(value: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(value)
}

fn status(code: u16) -> ResponseTemplate {
    ResponseTemplate::new(code).set_body_json(json!({}))
}

fn exec_reply(code: i32, stdout: &str) -> ResponseTemplate {
    reply(json!({"exit_code": code, "exit_signal": 0, "stdout": stdout, "stderr": ""}))
}

fn guard(state: &Arc<Mutex<State>>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn find<'a>(items: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    items.iter_mut().find(|item| item["id"] == id)
}

impl Respond for Fake {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let authorized = request
            .headers
            .get("authorization")
            .is_some_and(|value| value.as_bytes() == format!("Bearer {TOKEN}").as_bytes());
        if !authorized {
            return status(401);
        }
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let path = request.url.path().to_owned();
        let method = request.method.to_string();
        let mut state = guard(&self.0);
        state.calls.push(Call {
            method: method.clone(),
            path: path.clone(),
            query: request.url.query().map(str::to_owned),
            body: body.clone(),
        });
        let segments: Vec<&str> = path.split('/').collect();
        match (method.as_str(), segments.as_slice()) {
            ("GET", ["", "v1", "apps", APP, "machines"]) => {
                if state.list_fails {
                    return status(503);
                }
                reply(Value::Array(state.machines.clone()))
            }
            ("POST", ["", "v1", "apps", APP, "machines"]) => create_machine(&mut state, &body),
            ("GET", ["", "v1", "apps", APP, "machines", id]) => match find(&mut state.machines, id) {
                Some(machine) => reply(machine.clone()),
                None => status(404),
            },
            ("DELETE", ["", "v1", "apps", APP, "machines", id]) => {
                if state.delete_failures > 0 {
                    state.delete_failures -= 1;
                    return status(503);
                }
                state.machines.retain(|machine| machine["id"] != *id);
                reply(json!({"ok": true}))
            }
            ("POST", ["", "v1", "apps", APP, "machines", id, "start"]) => {
                if state.start_fails {
                    return status(503);
                }
                if !state.start_ignored
                    && let Some(machine) = find(&mut state.machines, id)
                {
                    machine["state"] = json!("started");
                }
                reply(json!({"ok": true}))
            }
            ("POST", ["", "v1", "apps", APP, "machines", id, "suspend"]) => {
                let next = if state.suspend_stops { "stopped" } else { "suspended" };
                if !state.suspend_ignored
                    && let Some(machine) = find(&mut state.machines, id)
                {
                    machine["state"] = json!(next);
                }
                reply(json!({"ok": true}))
            }
            ("POST", ["", "v1", "apps", APP, "machines", _, "exec"]) => exec(&state, &body),
            ("GET", ["", "v1", "apps", APP, "volumes", id]) => volume(&mut state, id),
            ("POST", ["", "v1", "apps", APP, "volumes"]) => create_volume(&mut state, &body),
            ("GET", ["", "v1", "apps", APP, "volumes"]) => reply(Value::Array(state.volumes.clone())),
            ("DELETE", ["", "v1", "apps", APP, "volumes", id]) => {
                if state.volume_delete_failures > 0 {
                    state.volume_delete_failures -= 1;
                    return status(503);
                }
                state.volumes.retain(|volume| volume["id"] != *id);
                reply(json!({"state": "destroyed"}))
            }
            ("POST", ["", "v1", "apps", APP, "volumes", _, "snapshots"]) => {
                if state.snapshot_create_fails {
                    return status(500);
                }
                if state.snapshot_without_id {
                    return reply(json!({"Msg": {}}));
                }
                reply(json!({"Msg": {"backup": {"id": "01BACKUP", "graph_id": "vs_snap1", "state": "prepare"}}}))
            }
            ("GET", ["", "v1", "apps", APP, "volumes", _, "snapshots"]) => snapshots(&mut state),
            _ => status(404),
        }
    }
}

fn create_machine(state: &mut State, body: &Value) -> ResponseTemplate {
    if let Some(code) = state.create_status {
        return status(code);
    }
    if state.create_response_empty {
        return status(503);
    }
    state.created += 1;
    let initial = match (&state.boot_state, state.boot_fails, body["skip_launch"] == true) {
        (Some(chosen), _, _) => chosen.clone(),
        (None, true, _) => "failed".to_owned(),
        (None, false, true) => "created".to_owned(),
        (None, false, false) => "started".to_owned(),
    };
    let machine = json!({
        "id": format!("machine{}", state.created),
        "name": body["name"],
        "state": initial,
        "config": body["config"],
    });
    state.machines.push(machine.clone());
    if state.create_response_lost {
        return status(503);
    }
    let template = reply(machine);
    match state.create_delay {
        Some(delay) => template.set_delay(delay),
        None => template,
    }
}

fn exec(state: &State, body: &Value) -> ResponseTemplate {
    let command: Vec<&str> = body["command"]
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let has = |needle: &str| command.iter().any(|part| part.contains(needle));
    if state.exec_http_fail.iter().any(|needle| has(needle)) {
        return status(500);
    }
    if let Some((_, code)) = state.exec_exit.iter().find(|(needle, _)| has(needle)) {
        return exec_reply(*code, "");
    }
    let template = if has("trogon-sandbox-capture") {
        exec_reply(0, state.capture_reply.as_deref().unwrap_or(CAPTURE_REPLY))
    } else if has("trogon-sandbox-read") {
        if state.read_exit_code != 0 {
            exec_reply(state.read_exit_code, "")
        } else if state.read_always_full {
            exec_reply(0, &base64_of(&[7u8; 24 * 1024]))
        } else {
            exec_reply(0, state.read_reply.as_deref().unwrap_or("AQI="))
        }
    } else if has("mktemp --") {
        exec_reply(0, state.mktemp_reply.as_deref().unwrap_or("/tmp/file.trogon.123\n"))
    } else {
        exec_reply(0, "")
    };
    match state.exec_sleep {
        Some(delay) => template.set_delay(delay),
        None => template,
    }
}

fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn volume(state: &mut State, id: &str) -> ResponseTemplate {
    if id == "vol_test" {
        return reply(
            state
                .operator_volume
                .clone()
                .unwrap_or_else(|| json!({"id": "vol_test", "region": "iad", "attached_machine_id": null})),
        );
    }
    match find(&mut state.volumes, id) {
        Some(volume) => reply(volume.clone()),
        None => status(404),
    }
}

fn create_volume(state: &mut State, body: &Value) -> ResponseTemplate {
    if state.volume_create_fails {
        return status(503);
    }
    let created = state.volume_state.clone().unwrap_or_else(|| "created".to_owned());
    let volume = json!({
        "id": format!("vol_w{}", state.volumes_created),
        "name": body["name"],
        "region": body["region"],
        "size_gb": body["size_gb"],
        "snapshot_id": body["snapshot_id"],
        "state": created,
        "attached_machine_id": null,
    });
    state.volumes_created += 1;
    state.volumes.push(volume.clone());
    if state.volume_create_lost {
        return status(503);
    }
    reply(volume)
}

fn snapshots(state: &mut State) -> ResponseTemplate {
    if state.snapshot_never_ready {
        return reply(json!([]));
    }
    if state.snapshot_polls_before_ready > 0 {
        state.snapshot_polls_before_ready -= 1;
        return reply(json!([]));
    }
    let mut entry = json!({"id": "vs_snap1", "size": 4096, "retention_days": state.retention_days});
    if let Some(chosen) = &state.snapshot_status {
        entry["status"] = json!(chosen);
    }
    reply(json!([entry]))
}

/// A running fake of one app's Machines API.
pub struct Fly {
    server: MockServer,
    state: Arc<Mutex<State>>,
}

impl Fly {
    pub async fn start() -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(Fake(state.clone()))
            .mount(&server)
            .await;
        Self { server, state }
    }

    pub fn url(&self) -> String {
        self.server.uri()
    }

    /// Reads or changes the fake's data and failure settings.
    pub fn with<R>(&self, change: impl FnOnce(&mut State) -> R) -> R {
        change(&mut guard(&self.state))
    }

    pub fn calls(&self) -> Vec<Call> {
        self.with(|state| state.calls.clone())
    }

    /// How many requests used `method` on a path ending in `suffix`.
    pub fn count(&self, method: &str, suffix: &str) -> usize {
        self.with(|state| {
            state
                .calls
                .iter()
                .filter(|call| call.method == method && call.path.ends_with(suffix))
                .count()
        })
    }

    pub fn machines(&self) -> Vec<Value> {
        self.with(|state| state.machines.clone())
    }

    pub fn volumes(&self) -> Vec<Value> {
        self.with(|state| state.volumes.clone())
    }

    pub fn api(&self) -> Boxed<FlyApi> {
        Ok(FlyApi::new(
            FlyEndpoint::parse(&self.url())?,
            FlyAppName::new(APP)?,
            FlyApiToken::new(TOKEN)?,
        )?)
    }
}

/// The settings every test starts from.
pub fn parts(dir: &Path) -> Boxed<FlyConfigParts> {
    Ok(FlyConfigParts {
        image: IMAGE.to_owned(),
        region: FlyRegion::new("iad")?,
        state_dir: dir.join("state"),
        boot_timeout: Duration::from_secs(2),
        max_leases: 2,
        memory_mib: std::num::NonZeroU32::new(512).ok_or("memory")?,
        cpus: std::num::NonZeroU32::MIN,
        warm_pool_size: 0,
        volume: None,
        workspace_volume: None,
        egress_proxy: None,
        public_host: None,
        preview_network: FlyPreviewNetwork::Public,
    })
}

pub fn volume_binding() -> Boxed<FlyVolumeBinding> {
    Ok(FlyVolumeBinding::new(
        FlyVolumeId::new("vol_test")?,
        DriveId {
            tenant: TenantId::new("tenant-a")?,
            name: DriveName::new("data")?,
        },
        MountPath::new("/data")?,
    )?)
}

/// A fake, a state directory and a provider talking to the one through the other.
pub struct Env {
    pub fly: Fly,
    pub dir: tempfile::TempDir,
    pub provider: Arc<FlyProvider>,
}

impl Env {
    pub async fn start() -> Boxed<Self> {
        Self::with_parts(|_| Ok(())).await
    }

    pub async fn with_parts(change: impl FnOnce(&mut FlyConfigParts) -> TestResult) -> Boxed<Self> {
        let fly = Fly::start().await;
        let dir = tempfile::tempdir()?;
        let mut settings = parts(dir.path())?;
        change(&mut settings)?;
        let provider = Arc::new(FlyProvider::new(fly.api()?, FlyConfig::new(settings)?)?);
        Ok(Self { fly, dir, provider })
    }

    /// A second provider over the same fake and state directory, as after a restart.
    pub fn restarted(self) -> Boxed<Self> {
        let Self { fly, dir, provider } = self;
        drop(provider);
        let provider = Arc::new(FlyProvider::new(fly.api()?, FlyConfig::new(parts(dir.path())?)?)?);
        Ok(Self { fly, dir, provider })
    }
}

pub fn request_json() -> Value {
    json!({
        "tenant": "test",
        "provider_pref": ["fly"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": {"registry": "docker.io", "repository": "library/ubuntu", "tag": "24.04"},
            },
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
            "env": {},
        },
        "ttl_seconds": 60,
        "idempotency_key": "test",
        "persistent": false,
    })
}

pub fn request() -> Boxed<LeaseRequest> {
    Ok(serde_json::from_value(request_json())?)
}

/// A request changed through `change` before it is parsed.
pub fn request_with(change: impl FnOnce(&mut Value)) -> Boxed<LeaseRequest> {
    let mut wire = request_json();
    change(&mut wire);
    Ok(serde_json::from_value(wire)?)
}

pub fn volume_request_json() -> Value {
    let mut wire = request_json();
    wire["tenant"] = json!("tenant-a");
    wire["spec"]["drives"] = json!([{
        "drive_id": {"tenant": "tenant-a", "name": "data"},
        "mount_path": "/data",
        "mode": "read_write",
    }]);
    wire
}

/// A throwaway NATS server running in a container.
pub struct Nats {
    inner: CoreTestServer,
}

impl Nats {
    pub async fn start() -> Self {
        Self {
            inner: CoreTestServer::start().await,
        }
    }

    pub fn url(&self) -> String {
        format!("nats://{}", self.inner.address())
    }

    pub async fn client(&self) -> Boxed<Client> {
        Ok(async_nats::connect(self.url()).await?)
    }
}

/// A worker serving a provider on a NATS server until stopped.
pub struct Running {
    pub env: Env,
    pub nats: Nats,
    pub client: Client,
    pub worker_id: trogon_sandbox_contract::WorkerId,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<Result<(), FlyWorkerError>>>,
}

impl Running {
    pub async fn start() -> Boxed<Self> {
        Self::start_with(Env::start().await?).await
    }

    pub async fn start_with(env: Env) -> Boxed<Self> {
        Self::start_draining(env, Duration::from_secs(300)).await
    }

    pub async fn start_draining(env: Env, drain: Duration) -> Boxed<Self> {
        let nats = Nats::start().await;
        let client = nats.client().await?;
        let worker_id = env.provider.dispatch_ledger().worker_id().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let worker = FlyWorker::new(
            client.clone(),
            env.provider.clone(),
            AuthorityConfig::new(AuthorityMode::Off),
        )
        .with_drain_timeout(drain);
        let task = tokio::spawn(async move {
            worker
                .run(async {
                    let _ = stopped.await;
                })
                .await
        });
        // The worker subscribes before it serves, so wait until its dispatch subject has a listener.
        tokio::time::sleep(Duration::from_millis(300)).await;
        Ok(Self {
            env,
            nats,
            client,
            worker_id,
            stop: Some(stop),
            task: Some(task),
        })
    }

    /// Asks the worker to stop and waits for it to finish.
    pub async fn stop(&mut self) -> TestResult {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }

    /// Sends one JSON request and returns the JSON reply.
    pub async fn ask(&self, subject: String, payload: &Value) -> Boxed<Value> {
        let reply = trogon_sandbox_protocol::nats_transfer::request(
            &self.client,
            &subject,
            serde_json::to_vec(payload)?.into(),
            None,
            Duration::from_secs(20),
        )
        .await?;
        Ok(serde_json::from_slice(&reply)?)
    }

    /// Dispatches a lease to this worker with a fresh attempt identity.
    pub async fn dispatch(&self, lease: &str, attempt: &str, request: Value) -> Boxed<Value> {
        let subject = trogon_sandbox_protocol::subjects::worker_dispatch(
            trogon_sandbox_contract::ProviderKind::Fly,
            &self.worker_id,
        );
        let payload = json!({
            "lease_id": lease,
            "request": request,
            "target": {"worker_id": self.worker_id.as_str(), "attempt_id": attempt},
        });
        self.ask(subject, &payload).await
    }

    /// Sends a lease operation such as `run_command` or `pause`.
    pub async fn operation(&self, verb: &str, lease: &str, mut payload: Value) -> Boxed<Value> {
        payload["lease_id"] = json!(lease);
        self.ask(format!("sandbox.provider.fly.{verb}.{lease}"), &payload).await
    }
}

pub fn target_json(env: &Env, attempt: &str) -> Value {
    json!({"worker_id": env.provider.dispatch_ledger().worker_id().as_str(), "attempt_id": attempt})
}

pub fn dispatch_payload(
    env: &Env,
    lease: &str,
    attempt: Option<&str>,
    request: Value,
    drives: Vec<Value>,
) -> Boxed<ProviderDispatchPayload> {
    let mut wire = json!({"lease_id": lease, "request": request, "resolved_drives": drives});
    if let Some(attempt) = attempt {
        wire["target"] = target_json(env, attempt);
    }
    Ok(serde_json::from_value(wire)?)
}

pub fn cleanup_request(env: &Env, lease: &str, attempt: &str, intent: &str) -> Boxed<ProviderCleanupRequest> {
    Ok(serde_json::from_value(json!({
        "identity": {"lease_id": lease, "provider": "fly", "target": target_json(env, attempt)},
        "intent": intent,
    }))?)
}
