//! Shared fixtures: a stateful fake of the Proxmox VE API, a throwaway NATS server, and request builders.

#![allow(dead_code)]

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_nats::Client;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Map, Value, json};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_contract::LeaseRequest;
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode};
use trogon_sandbox_proxmox_worker::{
    AntiSpoof, ProxmoxApi, ProxmoxConfig, ProxmoxConfigParts, ProxmoxProvider, ProxmoxWorker, ProxmoxWorkerError,
    ProxyNetwork, VmId,
};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type Boxed<T> = Result<T, Box<dyn std::error::Error>>;

pub const IMAGE: &str = "docker.io/library/ubuntu:24.04";
pub const NODE: &str = "test-node";
pub const TOKEN_ID: &str = "test@pve!worker";
pub const CAPTURE_MARKER: &str = "trogon-sandbox-capture";
pub const VM: u32 = 201;

/// One request the fake received.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: Method,
    pub path: String,
    pub body: Value,
}

impl Call {
    pub fn argv(&self) -> Vec<String> {
        self.body["command"]
            .as_array()
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default()
    }
}

/// The fake's data and the failures it can be told to produce.
#[derive(Default)]
pub struct Host {
    pub calls: Vec<Call>,
    pub clone_name: Option<String>,
    pub fail_start: bool,
    pub foreign_owner: bool,
    pub task_running: bool,
    pub firewall_disabled: bool,
    pub host_firewall_disabled: bool,
    pub file_data: Vec<u8>,
    pub next_id: u32,
    pub next_id_reply: Option<Value>,
    pub extra_vms: HashMap<u32, String>,
    pub fail_destroy: bool,
    /// Once a delete task is polled, the journal at this path is moved aside and replaced by a directory, so the
    /// worker cannot record the destroy it just confirmed.
    pub destroy_bookkeeping_fault: Option<PathBuf>,
    /// Every task stops with a failed exit status.
    pub failed_tasks: bool,
    pub snapshots: Vec<String>,
    pub binary_command: bool,
    pub capture_reply: Option<String>,
    pub file_missing: bool,
    pub absent_config_is_500: bool,
    pub config_unreadable: bool,
    pub hide_cluster_vm: bool,
    pub config_reports_missing: bool,
    pub config_forbidden: bool,
    pub absence_query_forbidden: bool,
    pub absence_query_reply: Option<Value>,
    pub template_extra: Map<String, Value>,
    pub weak_nic_filters: bool,
    pub applied_net0: Option<String>,
}

fn guard(state: &Arc<Mutex<Host>>) -> MutexGuard<'_, Host> {
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn data(value: Value) -> Response {
    axum::Json(json!({ "data": value })).into_response()
}

fn status(code: StatusCode) -> Response {
    code.into_response()
}

fn failure(message: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(json!({"message": message, "data": null})),
    )
        .into_response()
}

fn missing_message(vm: Option<u32>) -> String {
    format!(
        "Configuration file 'nodes/{NODE}/qemu-server/{}.conf' does not exist\n",
        vm.unwrap_or_default()
    )
}

fn query_value(query: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

async fn fake(State(state): State<Arc<Mutex<Host>>>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().unwrap_or("").to_owned();
    let vm = path.split('/').nth(6).and_then(|segment| segment.parse::<u32>().ok());
    let Ok(bytes) = axum::body::to_bytes(request.into_body(), 65536).await else {
        return status(StatusCode::BAD_REQUEST);
    };
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(body) => body,
            Err(_) => return status(StatusCode::BAD_REQUEST),
        }
    };
    let mut host = guard(&state);
    host.calls.push(Call {
        method: method.clone(),
        path: path.clone(),
        body: body.clone(),
    });
    if method == Method::PUT
        && path.ends_with("/config")
        && let Some(net0) = body["net0"].as_str()
    {
        host.applied_net0 = Some(net0.to_owned());
    }
    let request = Incoming {
        method,
        path,
        query,
        vm,
        body,
    };
    cluster(&mut host, &request)
        .or_else(|| firewall(&host, &request))
        .or_else(|| config(&mut host, &request))
        .or_else(|| lifecycle(&mut host, &request))
        .or_else(|| guest(&mut host, &request))
        .unwrap_or_else(|| data(Value::Null))
}

struct Incoming {
    method: Method,
    path: String,
    query: String,
    vm: Option<u32>,
    body: Value,
}

fn cluster(host: &mut Host, request: &Incoming) -> Option<Response> {
    let path = &request.path;
    if path.ends_with("/cluster/resources") && request.method == Method::GET {
        let mut vms = Vec::new();
        if host.clone_name.is_some() && !host.hide_cluster_vm {
            vms.push(json!({"vmid": VM}));
        }
        vms.extend(host.extra_vms.keys().map(|vm| json!({"vmid": vm})));
        return Some(data(json!(vms)));
    }
    if !path.ends_with("/cluster/nextid") {
        return None;
    }
    if request.query.is_empty() {
        return Some(data(
            host.next_id_reply
                .clone()
                .unwrap_or_else(|| json!(host.next_id.max(VM))),
        ));
    }
    let candidate = query_value(&request.query, "vmid").and_then(|value| value.parse::<u32>().ok())?;
    if host.absence_query_forbidden {
        return Some(status(StatusCode::FORBIDDEN));
    }
    if let Some(reply) = &host.absence_query_reply {
        return Some(data(reply.clone()));
    }
    if (candidate == VM && host.clone_name.is_some()) || host.extra_vms.contains_key(&candidate) {
        return Some(status(StatusCode::BAD_REQUEST));
    }
    Some(data(json!(candidate)))
}

fn firewall(host: &Host, request: &Incoming) -> Option<Response> {
    let path = &request.path;
    let get = request.method == Method::GET;
    if path.ends_with("/cluster/firewall/options") {
        return Some(data(json!({"enable": u8::from(!host.firewall_disabled)})));
    }
    if path.ends_with(&format!("/{NODE}/firewall/options")) {
        return Some(data(json!({"enable": u8::from(!host.host_firewall_disabled)})));
    }
    if path.ends_with("/services/pve-firewall/state") {
        return Some(data(json!({"state": "running"})));
    }
    if path.ends_with("/firewall/rules") && get {
        return Some(data(
            json!([{"pos": 0, "action": "ACCEPT"}, {"pos": 1, "action": "ACCEPT"}]),
        ));
    }
    if path.contains("/qemu/") && path.ends_with("/firewall/options") && get {
        return Some(data(
            json!({"enable": 1, "macfilter": u8::from(!host.weak_nic_filters), "ipfilter": 1}),
        ));
    }
    if path.ends_with("/firewall/ipset/ipfilter-net0") && get {
        return Some(data(json!([{"cidr": "10.9.0.2/32"}])));
    }
    if path.ends_with("/firewall/ipset") && get {
        return Some(data(json!([])));
    }
    None
}

fn config(host: &mut Host, request: &Incoming) -> Option<Response> {
    if request.path.ends_with("/100/config") {
        let mut template = json!({"template": 1, "net0": "virtio,bridge=vmbr0", "net1": "virtio,bridge=vmbr1"});
        if let Some(fields) = template.as_object_mut() {
            fields.extend(host.template_extra.clone());
        }
        return Some(data(template));
    }
    if !(request.path.ends_with("/config") && request.method == Method::GET) {
        return None;
    }
    if host.config_forbidden {
        return Some(status(StatusCode::FORBIDDEN));
    }
    if host.config_unreadable {
        return Some(failure("cluster configuration is not readable"));
    }
    if host.config_reports_missing {
        return Some(failure(&missing_message(request.vm)));
    }
    let name = if request.vm == Some(VM) {
        host.clone_name.as_ref()
    } else {
        request.vm.and_then(|vm| host.extra_vms.get(&vm))
    };
    Some(match name {
        Some(name) => data(json!({
            "name": if host.foreign_owner { "foreign-vm" } else { name },
            "net0": host.applied_net0.clone().unwrap_or_else(|| "virtio,bridge=vmbr0".into()),
            "net1": "virtio,bridge=vmbr1",
        })),
        None if host.absent_config_is_500 => failure(&missing_message(request.vm)),
        None => status(StatusCode::NOT_FOUND),
    })
}

fn lifecycle(host: &mut Host, request: &Incoming) -> Option<Response> {
    let path = &request.path;
    if path.ends_with("/clone") {
        let id = request.body["newid"].as_u64().and_then(|id| u32::try_from(id).ok())?;
        let name = request.body["name"].as_str()?.to_owned();
        if request.body["full"] != 0 {
            return Some(status(StatusCode::BAD_REQUEST));
        }
        if id == VM {
            host.clone_name = Some(name);
        } else {
            host.extra_vms.insert(id, name);
        }
        host.next_id = id + 1;
        return Some(data(json!("UPID:node:clone:1")));
    }
    if path.contains("/tasks/") {
        if path.contains("delete")
            && !host.task_running
            && let Some(journal) = host.destroy_bookkeeping_fault.take()
            && (std::fs::rename(&journal, journal.with_extension("fault-backup")).is_err()
                || std::fs::create_dir(&journal).is_err())
        {
            return Some(status(StatusCode::INTERNAL_SERVER_ERROR));
        }
        return Some(data(if host.task_running {
            json!({"status": "running"})
        } else if host.failed_tasks {
            json!({"status": "stopped", "exitstatus": "task failed"})
        } else {
            json!({"status": "stopped", "exitstatus": "OK"})
        }));
    }
    if request.method == Method::POST && path.ends_with("/snapshot") {
        let name = request.body["snapname"].as_str()?.to_owned();
        if request.body["vmstate"] != 1 {
            return Some(status(StatusCode::BAD_REQUEST));
        }
        host.snapshots.push(name);
        return Some(data(json!("UPID:node:snapshot:1")));
    }
    if request.method == Method::POST && path.ends_with("/rollback") {
        let name = path.rsplit('/').nth(1)?;
        if !host.snapshots.iter().any(|snapshot| snapshot == name) {
            return Some(failure("snapshot does not exist"));
        }
        return Some(data(json!("UPID:node:rollback:1")));
    }
    if path.ends_with("/status/start") && host.fail_start {
        return Some(status(StatusCode::INTERNAL_SERVER_ERROR));
    }
    if path.ends_with("/status/start") || path.ends_with("/status/stop") {
        return Some(data(json!("UPID:node:status:1")));
    }
    let vm = request.vm?;
    if request.method == Method::DELETE && path.ends_with(&vm.to_string()) {
        if host.fail_destroy {
            return Some(status(StatusCode::SERVICE_UNAVAILABLE));
        }
        if vm == VM {
            host.clone_name = None;
        } else {
            host.extra_vms.remove(&vm);
        }
        return Some(data(json!("UPID:node:delete:1")));
    }
    None
}

fn guest(host: &mut Host, request: &Incoming) -> Option<Response> {
    let path = &request.path;
    if path.ends_with("/agent/exec") {
        return Some(if request.body["command"].is_array() {
            data(json!({"pid": 42}))
        } else {
            status(StatusCode::BAD_REQUEST)
        });
    }
    if path.ends_with("/agent/exec-status") {
        return Some(exec_status(host));
    }
    if path.ends_with("/agent/file-read") {
        if host.file_missing {
            return Some(failure("Failed to open file: No such file or directory"));
        }
        let offset = query_value(&request.query, "offset").and_then(|value| value.parse::<usize>().ok())?;
        let end = (offset + 32768).min(host.file_data.len());
        let chunk = host.file_data.get(offset..end)?;
        return Some(data(json!({
            "content": base64::engine::general_purpose::STANDARD.encode(chunk),
            "truncated": u8::from(end < host.file_data.len()),
        })));
    }
    None
}

fn exec_status(host: &Host) -> Response {
    let last = host
        .calls
        .iter()
        .rev()
        .find(|call| call.path.ends_with("/agent/exec"))
        .map(Call::argv)
        .unwrap_or_default();
    let capture = last.iter().any(|arg| arg == CAPTURE_MARKER);
    let temporary = last.iter().any(|arg| arg.contains("mktemp"));
    let stdout = if capture {
        let output: &[u8] = if host.binary_command {
            &[255, 128, 65, 0]
        } else {
            b"guest-output\n"
        };
        host.capture_reply.clone().unwrap_or_else(|| {
            format!(
                "0\n{}\n{}\n\n0\n\n\n",
                output.len(),
                base64::engine::general_purpose::STANDARD.encode(output)
            )
        })
    } else if temporary {
        "/workspace/temporary-file\n".to_owned()
    } else {
        String::new()
    };
    data(json!({
        "exited": 1,
        "exitcode": 0,
        "out-data": stdout,
        "err-data": "",
        "out-truncated": 0,
        "err-truncated": 0,
    }))
}

/// A running fake of one node's Proxmox API.
pub struct Fake {
    state: Arc<Mutex<Host>>,
    address: std::net::SocketAddr,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fake {
    pub async fn start() -> Boxed<Self> {
        Self::start_with(Host::default()).await
    }

    pub async fn start_with(host: Host) -> Boxed<Self> {
        let state = Arc::new(Mutex::new(host));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let router = Router::new().fallback(fake).with_state(state.clone());
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Ok(Self { state, address, server })
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    /// Reads or changes the fake's data and failure settings.
    pub fn with<R>(&self, change: impl FnOnce(&mut Host) -> R) -> R {
        change(&mut guard(&self.state))
    }

    pub fn calls(&self) -> Vec<Call> {
        self.with(|host| host.calls.clone())
    }

    /// How many requests used `method` on a path ending in `suffix`.
    pub fn count(&self, method: &Method, suffix: &str) -> usize {
        self.with(|host| {
            host.calls
                .iter()
                .filter(|call| call.method == *method && call.path.ends_with(suffix))
                .count()
        })
    }

    pub fn clone_exists(&self) -> bool {
        self.with(|host| host.clone_name.is_some())
    }

    pub fn api(&self, task_timeout: Duration) -> Boxed<ProxmoxApi> {
        // The secret is made up for each run so no credential-looking literal is committed.
        let secret = uuid::Uuid::new_v4().to_string();
        Ok(ProxmoxApi::new(
            self.url().parse()?,
            NODE.to_owned(),
            TOKEN_ID,
            &secret,
            task_timeout,
        )?)
    }
}

pub fn network() -> Boxed<ProxyNetwork> {
    Ok(ProxyNetwork {
        bridge: "vmbr9".into(),
        addresses: vec![
            ipnet::Ipv4Net::new(Ipv4Addr::new(10, 9, 0, 2), 24)?,
            ipnet::Ipv4Net::new(Ipv4Addr::new(10, 9, 0, 3), 24)?,
        ],
        gateway: Ipv4Addr::new(10, 9, 0, 1),
        bind_address: Ipv4Addr::LOCALHOST,
        advertised_address: Ipv4Addr::new(10, 9, 0, 1),
        antispoof: AntiSpoof::Require,
    })
}

pub fn parts(state_dir: PathBuf, network: Option<ProxyNetwork>) -> Boxed<ProxmoxConfigParts> {
    Ok(ProxmoxConfigParts {
        template: VmId::try_from(100)?,
        image: IMAGE.to_owned(),
        state_dir,
        boot_timeout: Duration::from_millis(100),
        max_leases: 2,
        network,
    })
}

/// A fake, a state directory and a provider talking to the one through the other.
pub struct Env {
    pub fake: Fake,
    pub dir: tempfile::TempDir,
    pub provider: Arc<ProxmoxProvider>,
    pub task_timeout: Duration,
}

impl Env {
    pub async fn start() -> Boxed<Self> {
        Self::build(Host::default(), Duration::from_secs(1), None).await
    }

    pub async fn with_host(host: Host) -> Boxed<Self> {
        Self::build(host, Duration::from_secs(1), None).await
    }

    pub async fn with_task_timeout(host: Host, task_timeout: Duration) -> Boxed<Self> {
        Self::build(host, task_timeout, None).await
    }

    pub async fn with_network(host: Host, network: ProxyNetwork) -> Boxed<Self> {
        Self::build(host, Duration::from_secs(1), Some(network)).await
    }

    async fn build(host: Host, task_timeout: Duration, network: Option<ProxyNetwork>) -> Boxed<Self> {
        let fake = Fake::start_with(host).await?;
        let dir = tempfile::tempdir()?;
        let config = ProxmoxConfig::new(parts(dir.path().join("state"), network)?)?;
        let provider = Arc::new(ProxmoxProvider::new(fake.api(task_timeout)?, config)?);
        Ok(Self {
            fake,
            dir,
            provider,
            task_timeout,
        })
    }

    pub fn state_dir(&self) -> PathBuf {
        self.dir.path().join("state")
    }

    pub fn journal(&self, lease: &str) -> PathBuf {
        self.state_dir().join(format!("{lease}.json"))
    }

    /// A second provider over the same fake and state directory, as after a restart.
    pub fn restarted(self, task_timeout: Duration) -> Boxed<Self> {
        let Self {
            fake, dir, provider, ..
        } = self;
        drop(provider);
        let config = ProxmoxConfig::new(parts(dir.path().join("state"), None)?)?;
        let provider = Arc::new(ProxmoxProvider::new(fake.api(task_timeout)?, config)?);
        Ok(Self {
            fake,
            dir,
            provider,
            task_timeout,
        })
    }
}

pub fn request_json() -> Value {
    json!({
        "tenant": "test",
        "provider_pref": ["proxmox"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": {"registry": "docker.io", "repository": "library/ubuntu", "tag": "24.04"},
            },
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 1024,
            "vcpu_millis": 1500,
            "network_policy": "deny_all",
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

/// `length` bytes counting up and wrapping at 256.
pub fn pattern(length: usize) -> Vec<u8> {
    (0..length).map(|index| (index % 256) as u8).collect()
}

/// The capture a guest that passes every check of the spike reports: exit 7 with one line on each stream.
pub fn spike_capture() -> String {
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    format!(
        "7\n13\n{}\n\n13\n{}\n\n",
        encode(b"spike-stdout\n"),
        encode(b"spike-stderr\n")
    )
}

/// A fake host whose guest passes every check of the spike.
pub fn spike_host() -> Host {
    Host {
        capture_reply: Some(spike_capture()),
        file_data: pattern(512 * 1024),
        ..Default::default()
    }
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
    task: Option<tokio::task::JoinHandle<Result<(), ProxmoxWorkerError>>>,
}

impl Running {
    pub async fn start() -> Boxed<Self> {
        Self::start_with(Env::start().await?).await
    }

    pub async fn start_with(env: Env) -> Boxed<Self> {
        Self::serve(env, Nats::start().await, AuthorityConfig::new(AuthorityMode::Off)).await
    }

    /// Serves `env` on an already running NATS server with the given authority check.
    pub async fn serve(env: Env, nats: Nats, authority: AuthorityConfig) -> Boxed<Self> {
        let client = nats.client().await?;
        let worker_id = env.provider.dispatch_ledger().worker_id().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let worker = ProxmoxWorker::new(client.clone(), env.provider.clone(), authority)
            .with_drain_timeout(Duration::from_secs(30));
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
            trogon_sandbox_contract::ProviderKind::Proxmox,
            &self.worker_id,
        );
        let payload = json!({
            "lease_id": lease,
            "request": request,
            "target": {"worker_id": self.worker_id.as_str(), "attempt_id": attempt},
        });
        self.ask(subject, &payload).await
    }

    /// Sends a lease operation such as `run_command` or `write_files`.
    pub async fn operation(&self, verb: &str, lease: &str, mut payload: Value) -> Boxed<Value> {
        payload["lease_id"] = json!(lease);
        self.ask(format!("sandbox.provider.proxmox.{verb}.{lease}"), &payload)
            .await
    }
}
