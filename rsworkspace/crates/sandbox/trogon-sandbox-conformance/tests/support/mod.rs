//! An in-process worker for the provider subjects.
//!
//! It serves the verbs the suite sends over a real NATS server and runs commands and file verbs on the host, so the
//! suite is checked against a worker that behaves, without a container image or the network.

#![allow(dead_code, unused_imports)]

mod files;
mod process;
mod profile;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_nats::{Client, Message};
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};
use trogon_sandbox_conformance::{ExecModel, Target};
use trogon_sandbox_contract::{
    ExecutionId, FileOpRequest, LeaseError, LeaseId, NetworkPolicy, ProcessInfo, ProcessState, ProviderCleanupRequest,
    ProviderCleanupResponse, ProviderKind, ReadinessProbe, RunCommandRequest, WorkerId,
};
use trogon_sandbox_protocol::subjects::provider_token;
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, DispatchVerifier, EnforcementMode, OP_LEASE_DISPATCH, nats_transfer,
};

pub use process::LISTEN;
use process::{Proc, locked};
pub use profile::heartbeat;

/// A target that declares everything the shell worker serves.
pub fn shell_target(client: Client, kind: ProviderKind) -> Fallible<Target> {
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port();
    let mut target = Target::new(client, kind);
    target.exec = ExecModel::Shell;
    target.strict_unknown_command_exit = true;
    target.supports_pause_resume = true;
    target.graceful_stop = true;
    target.command_restart = true;
    target.named_processes = true;
    target.process_readiness = true;
    target.process_listener = Some((vec![LISTEN.to_owned(), port.to_string()], port));
    target.checkpoint_running_unsupported = true;
    target.native_passthrough = true;
    target.redispatch_idempotent = true;
    target.file_ops = true;
    target.file_search = true;
    target.file_archive = true;
    target.denied_connect_fails = true;
    Ok(target)
}

/// The keys of a signing broker and the worker that verifies it.
pub fn keys() -> Fallible<(Arc<DispatchSigner>, Arc<DispatchVerifier>)> {
    let signer = DispatchSigner::new("k1", [7; 32]);
    let verifier = DispatchVerifier::new(EnforcementMode::Require).with_key("k1", &signer.public_key_b64())?;
    Ok((Arc::new(signer), Arc::new(verifier)))
}

type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Exec {
    Shell,
    JsEval,
    None,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cleanup {
    Confirm,
    Fail,
}

/// What a grant says about the worker and attempt a dispatch targeted.
#[derive(Clone)]
pub enum Echo {
    /// The target, as sent.
    Faithful,
    /// Another worker than the one targeted.
    Worker(WorkerId),
    /// Another attempt than the one targeted.
    Attempt,
    /// Nothing.
    Silent,
}

#[derive(Clone)]
pub struct Config {
    pub kind: ProviderKind,
    pub exec: Exec,
    /// Requests a cancelled lease still answers before it refuses.
    pub lag: u32,
    /// Refuses requests that are not signed for this verifier.
    pub verifier: Option<Arc<DispatchVerifier>>,
    pub cleanup: Cleanup,
    /// Worker named in a grant when the dispatch carries no target.
    pub worker_id: Option<WorkerId>,
    pub echo: Echo,
    /// Evidence subject a grant carries.
    pub evidence: String,
    /// The image has no `nc`, only a Python 3 socket probe.
    pub python_probe: bool,
}

impl Config {
    pub fn new(kind: ProviderKind, exec: Exec) -> Self {
        Self {
            kind,
            exec,
            lag: 0,
            verifier: None,
            cleanup: Cleanup::Confirm,
            worker_id: None,
            echo: Echo::Faithful,
            evidence: "fake.evidence".to_owned(),
            python_probe: false,
        }
    }
}

struct Entry {
    id: ExecutionId,
    proc: Arc<Proc>,
    name: Option<trogon_sandbox_contract::ProcessName>,
    readiness: Option<ReadinessProbe>,
    request: RunCommandRequest,
}

#[derive(Default)]
struct Inner {
    paused: bool,
    lag: Option<u32>,
    deny_network: bool,
    globals: HashMap<String, String>,
    entries: Vec<Arc<Entry>>,
}

struct Lease {
    sandbox_id: String,
    inner: Mutex<Inner>,
}

pub struct FakeWorker {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct State {
    config: Config,
    client: Client,
    leases: Mutex<HashMap<String, Arc<Lease>>>,
}

pub async fn spawn(client: Client, config: Config) -> Fallible<FakeWorker> {
    let subject = format!("sandbox.provider.{}.>", provider_token(config.kind));
    let mut subscription = client.subscribe(subject).await?;
    client.flush().await?;
    let state = Arc::new(State {
        config,
        client,
        leases: Mutex::default(),
    });
    let task = tokio::spawn(async move {
        while let Some(message) = subscription.next().await {
            tokio::spawn(answer(state.clone(), message));
        }
    });
    Ok(FakeWorker { task })
}

async fn answer(state: Arc<State>, message: Message) {
    let Some(reply) = message.reply.clone() else {
        state.cancel_subject(message.subject.as_str());
        return;
    };
    let body = state
        .route(&message)
        .await
        .unwrap_or_else(|error| json!({ "error": error.to_string() }));
    if let Ok(bytes) = serde_json::to_vec(&body) {
        let _ = nats_transfer::respond(&state.client, reply.as_str(), Bytes::from(bytes)).await;
    }
}

pub fn denied(error: LeaseError) -> Fallible<Value> {
    let mut body = serde_json::to_value(error)?;
    body["outcome"] = json!("denied");
    Ok(body)
}

fn backend(message: &str) -> Fallible<Value> {
    denied(LeaseError::Backend {
        message: message.to_owned(),
    })
}

impl State {
    fn lease(&self, id: &str) -> Option<Arc<Lease>> {
        locked(&self.leases).get(id).cloned()
    }

    /// A lease that answers: a cancelled one serves only its lag.
    fn admit(&self, id: &str) -> Option<Arc<Lease>> {
        let lease = self.lease(id)?;
        let served = {
            let mut inner = locked(&lease.inner);
            match inner.lag {
                None => true,
                Some(0) => false,
                Some(remaining) => {
                    inner.lag = Some(remaining - 1);
                    true
                }
            }
        };
        served.then_some(lease)
    }

    fn cancel_subject(&self, subject: &str) {
        if let Some(id) = subject.rsplit('.').next() {
            self.release(id);
        }
    }

    fn release(&self, id: &str) {
        let Some(lease) = self.lease(id) else {
            return;
        };
        let mut inner = locked(&lease.inner);
        inner.lag = Some(self.config.lag);
        for entry in &inner.entries {
            entry.proc.terminate();
        }
    }

    async fn route(&self, message: &Message) -> Fallible<Value> {
        let tokens: Vec<&str> = message.subject.as_str().split('.').collect();
        let signed = message
            .headers
            .as_ref()
            .is_some_and(|headers| headers.get(AUTH_HEADER).is_some());
        let payload = nats_transfer::receive(&self.client, message).await?;
        match tokens.as_slice() {
            [_, _, _, "lease"] => self.dispatch(&payload, None),
            [_, _, _, "worker", worker, "lease"] => self.dispatch(&payload, Some(WorkerId::new(*worker)?)),
            [_, _, _, "worker", _, "cleanup"] => self.cleanup(&payload),
            [_, _, _, verb, lease] => {
                if self.config.verifier.is_some() && !signed {
                    return denied(LeaseError::Unauthorized {
                        reason: "request is not signed".to_owned(),
                    });
                }
                let request: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
                self.verb(verb, lease, request).await
            }
            _ => backend("unknown subject"),
        }
    }

    fn cleanup(&self, payload: &[u8]) -> Fallible<Value> {
        let request: ProviderCleanupRequest = serde_json::from_slice(payload)?;
        if self.config.cleanup == Cleanup::Confirm {
            self.release(request.identity.lease_id.as_str());
        }
        let response = match self.config.cleanup {
            Cleanup::Confirm => ProviderCleanupResponse::Cleaned {
                identity: request.identity,
                intent: request.intent,
            },
            Cleanup::Fail => ProviderCleanupResponse::Failed {
                identity: request.identity,
                message: "the worker could not clean up".to_owned(),
            },
        };
        Ok(serde_json::to_value(response)?)
    }

    fn dispatch(&self, payload: &[u8], own: Option<WorkerId>) -> Fallible<Value> {
        let request: Value = match &self.config.verifier {
            Some(verifier) => match verifier.decode(payload, OP_LEASE_DISPATCH, own.as_ref()) {
                Ok(request) => request,
                Err(_) => return Ok(json!({ "error": "dispatch is not signed" })),
            },
            None => serde_json::from_slice(payload)?,
        };
        let id = request["lease_id"].as_str().unwrap_or_default().to_owned();
        let sandbox_id = format!("fake-{id}");
        let deny_network =
            request["request"]["spec"]["network_policy"] == serde_json::to_value(NetworkPolicy::DenyAll)?;
        locked(&self.leases).entry(id).or_insert_with(|| {
            Arc::new(Lease {
                sandbox_id: sandbox_id.clone(),
                inner: Mutex::new(Inner {
                    deny_network,
                    ..Inner::default()
                }),
            })
        });
        let target = &request["target"];
        let mut grant = json!({ "sandbox_id": sandbox_id, "evidence_subject": self.config.evidence });
        if target.is_object() {
            match &self.config.echo {
                Echo::Faithful => {
                    grant["worker_id"] = target["worker_id"].clone();
                    grant["attempt_id"] = target["attempt_id"].clone();
                }
                Echo::Worker(other) => {
                    grant["worker_id"] = json!(other);
                    grant["attempt_id"] = target["attempt_id"].clone();
                }
                Echo::Attempt => {
                    grant["worker_id"] = target["worker_id"].clone();
                    grant["attempt_id"] = json!("another-attempt");
                }
                Echo::Silent => {}
            }
            if !target["epoch"].is_null() {
                grant["epoch"] = target["epoch"].clone();
            }
        } else if let Some(worker) = &self.config.worker_id {
            grant["worker_id"] = json!(worker);
        }
        if request["request"]["native"].is_object() {
            grant["native_error"] = json!("unknown native option conformance_unknown_option");
        }
        Ok(grant)
    }

    async fn verb(&self, verb: &str, id: &str, request: Value) -> Fallible<Value> {
        let lease_id = LeaseId::new(id);
        let not_found = LeaseError::LeaseNotFound { lease_id };
        let Some(lease) = self.admit(id) else {
            return denied(not_found);
        };
        if verb == "checkpoint_running" {
            return Ok(
                json!({ "outcome": "err", "kind": "unsupported", "message": "the fake worker cannot checkpoint" }),
            );
        }
        match self.config.exec {
            Exec::None => backend("this worker runs no commands"),
            Exec::JsEval => js_verb(&lease, verb, &request),
            Exec::Shell => self.shell_verb(&lease, verb, request).await,
        }
    }

    async fn shell_verb(&self, lease: &Arc<Lease>, verb: &str, request: Value) -> Fallible<Value> {
        match verb {
            "pause" | "resume" => {
                locked(&lease.inner).paused = verb == "pause";
                Ok(json!({ "outcome": "done" }))
            }
            "write_files" => {
                for file in request["files"].as_array().into_iter().flatten() {
                    let content: Vec<u8> = serde_json::from_value(file["content"].clone())?;
                    files::write(file["path"].as_str().unwrap_or_default(), &content)?;
                }
                Ok(json!({ "outcome": "written" }))
            }
            "read_file" => Ok(match std::fs::read(request["path"].as_str().unwrap_or_default()) {
                Ok(data) => json!({ "outcome": "content", "data": data }),
                Err(_) => json!({ "outcome": "not_found" }),
            }),
            "file_op" => {
                let request: FileOpRequest = serde_json::from_value(request)?;
                Ok(serde_json::to_value(files::serve(request.op)?)?)
            }
            "run_command" => self.run_command(lease, serde_json::from_value(request)?).await,
            "poll_command" => poll(lease, &request),
            "kill_command" => kill(lease, &request).await,
            "stop_command" => stop(lease, &request).await,
            "restart_command" => self.restart(lease, &request).await,
            "list_processes" => list(lease),
            "wait_ready" => wait_ready(lease, &request).await,
            _ => backend("unsupported verb"),
        }
    }

    async fn run_command(&self, lease: &Arc<Lease>, request: RunCommandRequest) -> Fallible<Value> {
        let (paused, deny_network) = {
            let inner = locked(&lease.inner);
            (inner.paused, inner.deny_network)
        };
        if paused {
            return denied(LeaseError::LeaseNotRunning {
                lease_id: request.lease_id,
            });
        }
        let command = request.command.join(" ");
        if command.contains("command -v nc") {
            let tool: &[u8] = if self.config.python_probe {
                b"PYTHON3\n"
            } else {
                b"NC\n"
            };
            return Ok(completed(0, tool, b"", false));
        }
        if command.contains("nc -z -w 3") || command.contains("connection=socket.socket") {
            let marker: &[u8] = if deny_network { b"DENIED\n" } else { b"CONNECTED\n" };
            return Ok(completed(0, marker, b"", false));
        }
        let detached = matches!(
            &request.execution,
            trogon_sandbox_contract::CommandExecution::Detached { .. }
        );
        if detached {
            return Ok(started(&*start(lease, request)?));
        }
        let wait = Duration::from_secs(u64::from(request.timeout_seconds));
        let proc = start(lease, request)?.proc.clone();
        match tokio::time::timeout(wait, proc.finished()).await {
            Ok(()) => {
                let (stdout, stderr) = proc.output();
                Ok(completed(proc.exit_code().unwrap_or(1), &stdout, &stderr, false))
            }
            Err(_) => {
                proc.terminate();
                proc.finished().await;
                let (stdout, stderr) = proc.output();
                Ok(completed(-1, &stdout, &stderr, true))
            }
        }
    }

    async fn restart(&self, lease: &Arc<Lease>, request: &Value) -> Fallible<Value> {
        let Some(old) = find(lease, request) else {
            return Ok(json!({ "outcome": "not_found" }));
        };
        old.proc.terminate();
        old.proc.finished().await;
        let fresh = start(lease, old.request.clone())?;
        Ok(json!({ "outcome": "restarted", "execution_id": fresh.id }))
    }
}

fn completed(exit_code: i32, stdout: &[u8], stderr: &[u8], timed_out: bool) -> Value {
    let mut body = json!({ "outcome": "completed", "exit_code": exit_code, "stdout": stdout, "stderr": stderr });
    if timed_out {
        body["timed_out"] = json!(true);
    }
    body
}

fn started(entry: &Entry) -> Value {
    json!({ "outcome": "started", "execution_id": entry.id })
}

/// Starts the request's command, or returns the running process of the name it asks for.
fn start(lease: &Arc<Lease>, request: RunCommandRequest) -> Fallible<Arc<Entry>> {
    let (name, readiness) = match &request.execution {
        trogon_sandbox_contract::CommandExecution::Detached {
            process: Some(process), ..
        } => (process.name.clone(), process.readiness.clone()),
        _ => (None, None),
    };
    let mut inner = locked(&lease.inner);
    if let Some(name) = &name
        && let Some(existing) = inner
            .entries
            .iter()
            .find(|entry| entry.name.as_ref() == Some(name) && entry.proc.exit_code().is_none())
    {
        return Ok(existing.clone());
    }
    let proc = process::start(&request.command, request.cwd.as_deref(), request.env.as_map());
    let entry = Arc::new(Entry {
        id: ExecutionId::new(uuid::Uuid::now_v7().to_string()),
        proc,
        name,
        readiness,
        request,
    });
    inner.entries.push(entry.clone());
    Ok(entry)
}

fn find(lease: &Arc<Lease>, request: &Value) -> Option<Arc<Entry>> {
    let wanted = request["execution_id"].as_str()?;
    locked(&lease.inner)
        .entries
        .iter()
        .find(|entry| entry.id.as_str() == wanted)
        .cloned()
}

fn poll(lease: &Arc<Lease>, request: &Value) -> Fallible<Value> {
    let Some(entry) = find(lease, request) else {
        return Ok(json!({ "outcome": "not_found" }));
    };
    Ok(match entry.proc.exit_code() {
        None => json!({ "outcome": "running" }),
        Some(code) => {
            let (stdout, stderr) = entry.proc.output();
            completed(code, &stdout, &stderr, false)
        }
    })
}

async fn kill(lease: &Arc<Lease>, request: &Value) -> Fallible<Value> {
    let Some(entry) = find(lease, request) else {
        return Ok(json!({ "outcome": "not_found" }));
    };
    if entry.proc.exit_code().is_some() {
        return Ok(json!({ "outcome": "already_completed" }));
    }
    entry.proc.terminate();
    Ok(json!({ "outcome": "signaled" }))
}

async fn stop(lease: &Arc<Lease>, request: &Value) -> Fallible<Value> {
    let Some(entry) = find(lease, request) else {
        return Ok(json!({ "outcome": "not_found" }));
    };
    if entry.proc.exit_code().is_some() {
        return Ok(json!({ "outcome": "already_completed" }));
    }
    entry.proc.terminate();
    entry.proc.finished().await;
    Ok(json!({ "outcome": "stopped", "signal": "term" }))
}

fn list(lease: &Arc<Lease>) -> Fallible<Value> {
    let processes: Vec<ProcessInfo> = locked(&lease.inner)
        .entries
        .iter()
        .filter(|entry| entry.name.is_some() || entry.readiness.is_some())
        .map(|entry| ProcessInfo {
            execution_id: entry.id.clone(),
            name: entry.name.clone(),
            readiness: entry.readiness.clone(),
            state: entry
                .proc
                .exit_code()
                .map_or(ProcessState::Running, |exit_code| ProcessState::Exited { exit_code }),
        })
        .collect();
    Ok(json!({ "outcome": "processes", "processes": processes }))
}

async fn wait_ready(lease: &Arc<Lease>, request: &Value) -> Fallible<Value> {
    let Some(entry) = find(lease, request) else {
        return Ok(json!({ "outcome": "not_found" }));
    };
    let Some(probe) = entry.readiness.clone() else {
        return Ok(json!({ "outcome": "no_probe" }));
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(request["timeout_seconds"].as_u64().unwrap_or(1));
    loop {
        let ready = match &probe {
            ReadinessProbe::OutputContains { needle } => {
                String::from_utf8_lossy(&locked(&entry.proc.stdout)).contains(needle.as_str())
            }
            ReadinessProbe::TcpPort { port } => tokio::net::TcpStream::connect(("127.0.0.1", port.get())).await.is_ok(),
        };
        if ready {
            return Ok(json!({ "outcome": "ready" }));
        }
        if let Some(exit_code) = entry.proc.exit_code() {
            return Ok(json!({ "outcome": "failed", "exit_code": exit_code }));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(json!({ "outcome": "not_ready" }));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The JS runtime: it evaluates a small subset and refuses everything else with a typed error.
fn js_verb(lease: &Arc<Lease>, verb: &str, request: &Value) -> Fallible<Value> {
    if verb != "run_command" {
        return backend("a JS runtime has no such verb");
    }
    let command: Vec<&str> = request["command"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let plain = request["detached"] != json!(true)
        && request["cwd"].is_null()
        && request["env"].as_object().is_none_or(|env| env.is_empty());
    let (true, ["eval", source]) = (plain, command.as_slice()) else {
        return backend("a JS runtime evaluates one source at a time");
    };
    let mut inner = locked(&lease.inner);
    if let Some(key) = source
        .strip_prefix("globalThis[")
        .and_then(|rest| rest.split(']').next())
    {
        if source.contains("= \"kept\"") {
            inner.globals.insert(key.to_owned(), "kept".to_owned());
            return Ok(completed(0, b"", b"", false));
        }
        let value = inner
            .globals
            .get(key)
            .cloned()
            .unwrap_or_else(|| "undefined".to_owned());
        return Ok(completed(0, format!("{value}\n").as_bytes(), b"", false));
    }
    if let Some(key) = source
        .strip_prefix("console.log(globalThis[")
        .and_then(|rest| rest.split(']').next())
    {
        let value = inner
            .globals
            .get(key)
            .cloned()
            .unwrap_or_else(|| "undefined".to_owned());
        return Ok(completed(0, format!("{value}\n").as_bytes(), b"", false));
    }
    if source.starts_with("throw new Error(") {
        return Ok(completed(1, b"", source.as_bytes(), false));
    }
    if source.starts_with("for (;;)") {
        return Ok(completed(-1, b"", b"", true));
    }
    Ok(completed(0, b"hello\n", b"", false))
}
