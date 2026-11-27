//! Shared fixtures: an in-process fake of the Kubernetes API (REST objects, watches and exec over WebSocket)
//! and request builders. The fake listens on a loopback port, so both the library and the binary can use it.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_nats::jetstream;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::State as AxumState;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use futures::SinkExt;
use futures::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{Message, handshake::derive_accept_key, protocol::Role};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ArtifactType, LeaseId, LeaseRequest, OciArtifactRef};
use trogon_sandbox_k8s_worker::{DigestCache, K8sWorkerProvider, WarmPool};
use trogon_sandbox_oci::{
    Annotations, DockerConfigSource, Layer, OciClient, OciClientConfig, RegistryHost, build_push_target,
};
use trogon_sandbox_providers::{K8sMicroVmConfig, K8sMicroVmProvider};

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type Boxed<T> = Result<T, Box<dyn std::error::Error>>;

pub const NAMESPACE: &str = "fixture";
pub const IMAGE: &str = "fixture-agent:latest";
/// When the fake created every pod, and what its clock reads: the pods are thirty seconds old.
pub const CREATED_AT: &str = "2026-01-01T00:00:00Z";
const CLOCK: &str = "Thu, 01 Jan 2026 00:00:30 GMT";

/// One request the fake received.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: Value,
}

/// What an exec whose command contains `needle` does.
#[derive(Clone, Debug)]
pub struct ExecScript {
    pub needle: String,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: i32,
    /// The exec stays open until the cluster is released.
    pub hold: bool,
    /// The exec ends once the client closes its input.
    pub until_stdin_closes: bool,
    /// Finishing this exec lets every held exec finish.
    pub releases: bool,
    /// The upgrade is refused with this HTTP status.
    pub refuse: Option<u16>,
}

impl ExecScript {
    pub fn new(needle: &str) -> Self {
        Self {
            needle: needle.into(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
            hold: false,
            until_stdin_closes: false,
            releases: false,
            refuse: None,
        }
    }

    pub fn stdout(mut self, bytes: &[u8]) -> Self {
        self.stdout = bytes.to_vec();
        self
    }

    pub fn stderr(mut self, bytes: &[u8]) -> Self {
        self.stderr = bytes.to_vec();
        self
    }

    pub fn exit(mut self, code: i32) -> Self {
        self.exit_code = code;
        self
    }

    pub fn hold(mut self) -> Self {
        self.hold = true;
        self
    }

    pub fn until_stdin_closes(mut self) -> Self {
        self.until_stdin_closes = true;
        self
    }

    pub fn releases(mut self) -> Self {
        self.releases = true;
        self
    }

    pub fn refuse(mut self, status: u16) -> Self {
        self.refuse = Some(status);
        self
    }
}

/// The fake's data and the failures it can be told to produce.
#[derive(Default)]
pub struct State {
    pub objects: BTreeMap<(String, String), Value>,
    pub calls: Vec<Call>,
    pub execs: Vec<String>,
    pub stdin: Vec<u8>,
    pub scripts: Vec<ExecScript>,
    pub next_uid: u64,
    pub fail_deletes: usize,
    pub fail_creates: Vec<String>,
    pub fail_reads: Vec<String>,
    /// Collections whose reads are refused as forbidden, which a client does not retry.
    pub forbid_reads: Vec<String>,
    pub omit_job_uid: bool,
    pub acknowledge_without_removing: bool,
    pub pod_phase: Option<String>,
    /// The apiserver sends no `Date` header.
    pub no_clock: bool,
    pub jobs_created: usize,
    /// Held execs wait on this; releasing swaps in a fresh one.
    pub release: CancellationToken,
}

pub struct Cluster {
    endpoint: String,
    state: Arc<Mutex<State>>,
    stop: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl Drop for Cluster {
    fn drop(&mut self) {
        self.release();
        self.stop.cancel();
    }
}

fn status(code: u16, reason: &str) -> Value {
    json!({"apiVersion": "v1", "kind": "Status", "status": "Failure", "reason": reason, "code": code})
}

fn success() -> Value {
    json!({"apiVersion": "v1", "kind": "Status", "status": "Success", "code": 200})
}

fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part.split_once('=').unwrap_or((part, ""));
            (decode(name), decode(value))
        })
        .collect()
}

fn selected(object: &Value, selector: &str) -> bool {
    selector.split(',').filter(|term| !term.is_empty()).all(|term| {
        let labels = &object["metadata"]["labels"];
        if let Some((key, value)) = term.split_once("!=") {
            return labels[key].as_str() != Some(value);
        }
        if let Some((key, value)) = term.split_once('=') {
            return labels[key].as_str() == Some(value.trim_start_matches('='));
        }
        if let Some(key) = term.strip_prefix('!') {
            return labels.get(key).is_none();
        }
        labels.get(term).is_some()
    })
}

impl State {
    fn insert(&mut self, collection: &str, mut object: Value) -> Value {
        self.next_uid += 1;
        object["metadata"]["uid"] = json!(format!("uid-{collection}-{}", self.next_uid));
        object["metadata"]["namespace"] = json!(NAMESPACE);
        object["metadata"]["resourceVersion"] = json!(self.next_uid.to_string());
        if collection == "pods" {
            object["metadata"]["creationTimestamp"] = json!(CREATED_AT);
        }
        let name = object["metadata"]["name"].as_str().unwrap_or_default().to_string();
        self.objects.insert((collection.into(), name), object.clone());
        object
    }

    fn insert_job(&mut self, job: Value) -> Value {
        self.jobs_created += 1;
        let created = self.insert("jobs", job);
        let name = created["metadata"]["name"].as_str().unwrap_or_default().to_string();
        let phase = self.pod_phase.clone().unwrap_or_else(|| "Running".into());
        let ready = phase == "Running";
        let pod = json!({
            "apiVersion": "v1", "kind": "Pod",
            "metadata": {"name": format!("{name}-pod"),
                "labels": {"job-name": name},
                "ownerReferences": [{"apiVersion": "batch/v1", "kind": "Job", "name": name,
                    "uid": created["metadata"]["uid"], "controller": true}]},
            "spec": {"containers": [{"name": "agent", "image": IMAGE}]},
            "status": {"phase": phase, "containerStatuses": [{"name": "agent", "ready": ready,
                "restartCount": 0, "image": IMAGE, "imageID": "fixture-image",
                "state": {"running": {}}}]}
        });
        self.insert("pods", pod);
        let mut reply = created;
        if self.omit_job_uid
            && let Some(metadata) = reply["metadata"].as_object_mut()
        {
            metadata.remove("uid");
        }
        reply
    }

    fn list(&self, collection: &str, query: &str) -> Vec<Value> {
        let all = params(query);
        let label = all
            .iter()
            .find(|(name, _)| name == "labelSelector")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let field = all
            .iter()
            .find(|(name, _)| name == "fieldSelector")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        self.objects
            .iter()
            .filter(|((kind, _), _)| kind == collection)
            .filter(|(_, object)| selected(object, &label))
            .filter(|((_, name), _)| field.strip_prefix("metadata.name=").is_none_or(|wanted| wanted == name))
            .map(|(_, object)| object.clone())
            .collect()
    }

    fn serve(&mut self, method: &str, path: &str, query: &str, body: Value) -> (u16, Value) {
        self.calls.push(Call {
            method: method.into(),
            path: path.into(),
            query: query.into(),
            body: body.clone(),
        });
        let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
        let Some(index) = parts.iter().position(|part| *part == "namespaces") else {
            return (404, status(404, "NotFound"));
        };
        if parts.get(index + 1) != Some(&NAMESPACE) {
            return (404, status(404, "NotFound"));
        }
        let collection = parts.get(index + 2).copied().unwrap_or("");
        let name = parts.get(index + 3).copied();
        if method == "GET" && self.forbid_reads.iter().any(|needle| path.contains(needle.as_str())) {
            return (403, status(403, "Forbidden"));
        }
        if method == "GET" && self.fail_reads.iter().any(|needle| path.contains(needle.as_str())) {
            return (503, status(503, "Unavailable"));
        }
        match (method, name) {
            ("POST", None) => {
                if self.fail_creates.iter().any(|needle| needle == collection) {
                    return (503, status(503, "Unavailable"));
                }
                let mut body = body;
                if body["metadata"]["name"].is_null()
                    && let Some(prefix) = body["metadata"]["generateName"].as_str().map(str::to_string)
                {
                    body["metadata"]["name"] = json!(format!("{prefix}{}", self.next_uid + 1));
                }
                let name = body["metadata"]["name"].as_str().unwrap_or_default().to_string();
                if self.objects.contains_key(&(collection.into(), name)) {
                    return (409, status(409, "AlreadyExists"));
                }
                let created = if collection == "jobs" {
                    self.insert_job(body)
                } else {
                    self.insert(collection, body)
                };
                (201, created)
            }
            ("GET", None) => {
                let items = self.list(collection, query);
                if params(query)
                    .iter()
                    .any(|(name, value)| name == "watch" && (value == "true" || value == "1"))
                {
                    let events: Vec<Value> = items
                        .into_iter()
                        .map(|object| json!({"type": "ADDED", "object": object}))
                        .collect();
                    return (200, Value::Array(events));
                }
                let (api_version, kind) = match collection {
                    "jobs" => ("batch/v1", "JobList"),
                    "pods" => ("v1", "PodList"),
                    "secrets" => ("v1", "SecretList"),
                    "configmaps" => ("v1", "ConfigMapList"),
                    _ => ("networking.k8s.io/v1", "NetworkPolicyList"),
                };
                (
                    200,
                    json!({"apiVersion": api_version, "kind": kind, "metadata": {"resourceVersion": "1"}, "items": items}),
                )
            }
            ("GET", Some(name)) => self
                .objects
                .get(&(collection.into(), name.into()))
                .cloned()
                .map_or((404, status(404, "NotFound")), |object| (200, object)),
            ("PATCH" | "PUT", Some(name)) => {
                let key = (collection.to_string(), name.to_string());
                let Some(current) = self.objects.get_mut(&key) else {
                    return (404, status(404, "NotFound"));
                };
                if body["metadata"]["uid"]
                    .as_str()
                    .is_some_and(|uid| current["metadata"]["uid"] != uid)
                    || body["metadata"]["resourceVersion"]
                        .as_str()
                        .is_some_and(|revision| current["metadata"]["resourceVersion"] != revision)
                {
                    return (409, status(409, "Conflict"));
                }
                if method == "PUT" {
                    *current = body;
                } else {
                    if let Some(fields) = body["metadata"].as_object() {
                        for (field, value) in fields {
                            current["metadata"][field] = value.clone();
                        }
                    }
                    for section in ["spec", "data", "stringData", "status"] {
                        if let Some(value) = body.get(section) {
                            current[section] = value.clone();
                        }
                    }
                }
                self.next_uid += 1;
                current["metadata"]["resourceVersion"] = json!(self.next_uid.to_string());
                (200, current.clone())
            }
            ("DELETE", Some(name)) => {
                if self.fail_deletes > 0 {
                    self.fail_deletes -= 1;
                    return (503, status(503, "Unavailable"));
                }
                let key = (collection.to_string(), name.to_string());
                let Some(current) = self.objects.get(&key) else {
                    return (404, status(404, "NotFound"));
                };
                if body["preconditions"]["uid"]
                    .as_str()
                    .is_some_and(|uid| current["metadata"]["uid"] != uid)
                    || body["preconditions"]["resourceVersion"]
                        .as_str()
                        .is_some_and(|revision| current["metadata"]["resourceVersion"] != revision)
                {
                    return (409, status(409, "Conflict"));
                }
                if self.acknowledge_without_removing {
                    return (200, success());
                }
                let Some(removed) = self.objects.remove(&key) else {
                    return (404, status(404, "NotFound"));
                };
                let uid = removed["metadata"]["uid"].clone();
                if collection == "jobs" {
                    self.objects.retain(|(kind, _), object| {
                        kind != "pods"
                            || !object["metadata"]["ownerReferences"]
                                .as_array()
                                .is_some_and(|owners| owners.iter().any(|owner| owner["uid"] == uid))
                    });
                }
                (200, success())
            }
            _ => (405, status(405, "MethodNotAllowed")),
        }
    }
}

struct Head {
    method: String,
    path: String,
    query: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

async fn read_request(stream: &mut TcpStream) -> Option<Head> {
    let mut bytes = Vec::new();
    let end = loop {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header = String::from_utf8(bytes[..end].to_vec()).ok()?;
    let mut lines = header.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < end + length {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    let body = if length == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&bytes[end..end + length]).unwrap_or(Value::Null)
    };
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    Some(Head {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers,
        body,
    })
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        404 => "Not Found",
        409 => "Conflict",
        501 => "Not Implemented",
        _ => "Service Unavailable",
    }
}

async fn reply(stream: &mut TcpStream, code: u16, body: Value, clock: bool) {
    let bytes = match &body {
        Value::Array(events) => events
            .iter()
            .flat_map(|event| {
                let mut line = serde_json::to_vec(event).unwrap_or_default();
                line.push(b'\n');
                line
            })
            .collect(),
        other => serde_json::to_vec(other).unwrap_or_default(),
    };
    let date = if clock {
        format!("Date: {CLOCK}\r\n")
    } else {
        String::new()
    };
    let headers = format!(
        "HTTP/1.1 {code} {}\r\n{date}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason(code),
        bytes.len()
    );
    let _ = stream.write_all(headers.as_bytes()).await;
    let _ = stream.write_all(&bytes).await;
}

async fn serve_connection(mut stream: TcpStream, state: Arc<Mutex<State>>) {
    let Some(head) = read_request(&mut stream).await else {
        return;
    };
    if head.path.ends_with("/exec") {
        exec(stream, head, state).await;
        return;
    }
    let (code, body, clock) = {
        let mut state = lock(&state);
        let (code, body) = state.serve(&head.method, &head.path, &head.query, head.body);
        (code, body, !state.no_clock)
    };
    reply(&mut stream, code, body, clock).await;
}

async fn exec(mut stream: TcpStream, head: Head, state: Arc<Mutex<State>>) {
    let command = params(&head.query)
        .into_iter()
        .filter(|(name, _)| name == "command")
        .map(|(_, value)| value)
        .collect::<Vec<_>>()
        .join(" ");
    let takes_stdin = params(&head.query)
        .iter()
        .any(|(name, value)| name == "stdin" && value == "true");
    let (script, release) = {
        let mut state = lock(&state);
        state.execs.push(command.clone());
        let script = state
            .scripts
            .iter()
            .find(|script| command.contains(&script.needle))
            .cloned();
        (script, state.release.clone())
    };
    let script = script.unwrap_or_else(|| ExecScript::new(""));
    if let Some(code) = script.refuse {
        reply(&mut stream, code, status(code, "Refused"), false).await;
        return;
    }
    let Some(key) = head.headers.get("sec-websocket-key") else {
        reply(&mut stream, 400, status(400, "BadRequest"), false).await;
        return;
    };
    let accept = derive_accept_key(key.as_bytes());
    // Version 5 lets the client end its input without closing the whole socket.
    let protocol = "v5.channel.k8s.io";
    let handshake = format!(
        "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: {protocol}\r\n\r\n"
    );
    if stream.write_all(handshake.as_bytes()).await.is_err() {
        return;
    }
    let socket = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
    let (mut sink, mut incoming) = socket.split();
    let received = state.clone();
    let arrived = Arc::new(tokio::sync::Notify::new());
    let signal = arrived.clone();
    let mut reader = tokio::spawn(async move {
        while let Some(Ok(message)) = incoming.next().await {
            match message {
                Message::Binary(frame) if frame.first() == Some(&0) => {
                    lock(&received).stdin.extend_from_slice(&frame[1..]);
                    signal.notify_one();
                }
                Message::Binary(frame) if frame.first() == Some(&255) => {
                    signal.notify_one();
                    break;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });
    if script.hold {
        release.cancelled().await;
    } else {
        // Input sent with the command is read before the command finishes.
        let wait = if takes_stdin {
            Duration::from_secs(3)
        } else {
            Duration::from_millis(1)
        };
        let _ = tokio::time::timeout(wait, arrived.notified()).await;
    }
    for (channel, bytes) in [(1u8, &script.stdout), (2u8, &script.stderr)] {
        if bytes.is_empty() {
            continue;
        }
        let mut frame = vec![channel];
        frame.extend_from_slice(bytes);
        let _ = sink.send(Message::Binary(frame.into())).await;
    }
    if script.until_stdin_closes {
        let _ = (&mut reader).await;
    }
    let outcome = if script.exit_code == 0 {
        json!({"status": "Success"})
    } else {
        json!({"status": "Failure", "reason": "NonZeroExitCode",
            "details": {"causes": [{"reason": "ExitCode", "message": script.exit_code.to_string()}]}})
    };
    let mut frame = vec![3u8];
    frame.extend_from_slice(&serde_json::to_vec(&outcome).unwrap_or_default());
    let _ = sink.send(Message::Binary(frame.into())).await;
    if script.releases {
        release_all(&state);
    }
    let _ = sink.close().await;
    // Input the client sends alongside the command is still recorded after the command ends.
    let _ = tokio::time::timeout(Duration::from_millis(500), &mut reader).await;
}

fn release_all(state: &Mutex<State>) {
    let mut state = lock(state);
    state.release.cancel();
    state.release = CancellationToken::new();
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Cluster {
    pub async fn start() -> Boxed<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let state = Arc::new(Mutex::new(State::default()));
        let stop = CancellationToken::new();
        let task = tokio::spawn({
            let state = state.clone();
            let stop = stop.clone();
            async move {
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        () = stop.cancelled() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            connections.spawn(serve_connection(stream, state.clone()));
                        }
                    }
                }
                connections.abort_all();
            }
        });
        Ok(Self {
            endpoint,
            state,
            stop,
            task: Some(task),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Lets every held exec finish.
    pub fn release(&self) {
        release_all(&self.state);
    }

    pub fn script(&self, script: ExecScript) {
        self.state().scripts.push(script);
    }

    pub fn client(&self) -> Boxed<kube::Client> {
        let mut config = kube::Config::new(self.endpoint.parse()?);
        config.default_namespace = NAMESPACE.into();
        config.default_retry = false;
        Ok(kube::Client::try_from(config)?)
    }

    pub fn provider(&self) -> Boxed<Arc<K8sMicroVmProvider>> {
        self.provider_with(K8sMicroVmConfig::new(NAMESPACE))
    }

    pub fn provider_with(&self, config: K8sMicroVmConfig) -> Boxed<Arc<K8sMicroVmProvider>> {
        Ok(Arc::new(K8sMicroVmProvider::with_client(self.client()?, config)))
    }

    /// Waits until the input the cluster received equals `expected`.
    pub async fn stdin_is(&self, expected: &[u8]) -> bool {
        for _ in 0..200 {
            if self.state().stdin == expected {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    pub fn names(&self, collection: &str) -> Vec<String> {
        self.state()
            .objects
            .keys()
            .filter(|(kind, _)| kind == collection)
            .map(|(_, name)| name.clone())
            .collect()
    }

    pub fn object(&self, collection: &str, name: &str) -> Option<Value> {
        self.state()
            .objects
            .get(&(collection.to_string(), name.to_string()))
            .cloned()
    }

    pub fn count_calls(&self, method: &str, needle: &str) -> usize {
        self.state()
            .calls
            .iter()
            .filter(|call| call.method == method && call.path.contains(needle))
            .count()
    }

    pub async fn shutdown(mut self) {
        self.release();
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }
}

/// A lease request for the fixture image with an open network.
pub fn lease_request() -> Boxed<LeaseRequest> {
    lease_request_with(json!({}))
}

/// The base request with `overrides` merged into its spec.
pub fn lease_request_with(overrides: Value) -> Boxed<LeaseRequest> {
    let mut request = json!({
        "tenant": "tenant-a",
        "provider_pref": ["k8s_micro_vm"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "registry.example", "repository": "sessions/python", "tag": "1.0"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 5,
            "memory_mib": 512,
            "env": {},
            "mounts": [],
            "network_policy": "allow_all",
            "boot_mode": "entrypoint_default",
        },
        "ttl_seconds": 60,
        "idempotency_key": "key-1",
        "persistent": true,
    });
    if let (Some(spec), Some(extra)) = (request["spec"].as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            spec.insert(key.clone(), value.clone());
        }
    }
    Ok(serde_json::from_value(request)?)
}

/// The pieces a warm pool is built from.
pub struct Parts {
    pub inner: Arc<K8sMicroVmProvider>,
    pub oci: Arc<OciClient>,
    pub digests: Arc<DigestCache>,
    pub registry: String,
}

/// A fake cluster, a fake registry, a NATS server with JetStream, and a provider wired to all three.
pub struct World {
    pub cluster: Cluster,
    pub registry: Registry,
    pub nats: JetStreamTestServer,
    pub provider: Arc<K8sWorkerProvider>,
}

impl World {
    pub async fn start() -> Boxed<Self> {
        Self::with(K8sMicroVmConfig::new(NAMESPACE), |_| None).await
    }

    pub async fn with(
        config: K8sMicroVmConfig,
        warm_pool: impl FnOnce(&Parts) -> Option<Arc<WarmPool>>,
    ) -> Boxed<Self> {
        Self::build(config, false, warm_pool).await
    }

    /// A world whose provider keeps each lease request in the cluster so rollback survives a restart.
    pub async fn persisting(config: K8sMicroVmConfig) -> Boxed<Self> {
        Self::build(config, true, |_| None).await
    }

    /// A provider for the same cluster, as if the worker had restarted.
    pub async fn restarted(&self, config: K8sMicroVmConfig, persist: bool) -> Boxed<Arc<K8sWorkerProvider>> {
        Ok(Arc::new(
            K8sWorkerProvider::new(
                self.cluster.provider_with(config)?,
                IMAGE.into(),
                jetstream::new(self.nats.client().await),
                Arc::new(self.registry.client()?),
                None,
                Arc::new(DigestCache::new(8)),
                None,
            )
            .with_persisted_rollback_state(persist),
        ))
    }

    async fn build(
        config: K8sMicroVmConfig,
        persist: bool,
        warm_pool: impl FnOnce(&Parts) -> Option<Arc<WarmPool>>,
    ) -> Boxed<Self> {
        let cluster = Cluster::start().await?;
        let registry = Registry::start().await?;
        let nats = JetStreamTestServer::start().await;
        let parts = Parts {
            inner: cluster.provider_with(config)?,
            oci: Arc::new(registry.client()?),
            digests: Arc::new(DigestCache::new(8)),
            registry: registry.address.clone(),
        };
        let pool = warm_pool(&parts);
        let provider = Arc::new(
            K8sWorkerProvider::new(
                parts.inner,
                IMAGE.into(),
                jetstream::new(nats.client().await),
                parts.oci,
                None,
                parts.digests,
                pool,
            )
            .with_persisted_rollback_state(persist),
        );
        Ok(Self {
            cluster,
            registry,
            nats,
            provider,
        })
    }
}

/// Pushes `content` to the registry as a root filesystem artifact and returns the reference that names it.
pub async fn seed_snapshot(registry: &Registry, repository: &str, content: &[u8]) -> Boxed<OciArtifactRef> {
    let layers = vec![Layer {
        data: content.to_vec(),
        media_type: "application/vnd.oci.image.layer.v1.tar".to_string(),
        annotations: Annotations::default(),
    }];
    let target = build_push_target(
        &registry.address,
        repository,
        ArtifactType::SandboxRootfsV1,
        &layers,
        &Annotations::default(),
    )?;
    registry
        .client()?
        .push_artifact(&target, layers, Annotations::default())
        .await?;
    Ok(target)
}

#[derive(Default)]
struct RegistryState {
    blobs: BTreeMap<String, Vec<u8>>,
    manifests: BTreeMap<(String, String), Vec<u8>>,
    uploads: BTreeMap<String, Vec<u8>>,
    next_upload: u64,
}

/// An in-memory registry that speaks enough of the distribution API for pushing and pulling artifacts.
pub struct Registry {
    pub address: String,
    state: Arc<Mutex<RegistryState>>,
    task: JoinHandle<()>,
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Registry {
    pub async fn start() -> Boxed<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?.to_string();
        let state: Arc<Mutex<RegistryState>> = Arc::default();
        let app = Router::new().fallback(registry_handle).with_state(state.clone());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { address, state, task })
    }

    pub fn client(&self) -> Boxed<OciClient> {
        Ok(OciClient::new(OciClientConfig {
            insecure_registries: vec![RegistryHost::new(&self.address)?],
            docker_config: DockerConfigSource::Disabled,
            ..Default::default()
        }))
    }

    /// Stops serving, so later requests are refused.
    pub fn shutdown(&self) {
        self.task.abort();
    }

    pub fn manifest_count(&self) -> usize {
        self.lock().manifests.len()
    }

    fn lock(&self) -> MutexGuard<'_, RegistryState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn digest_of(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

fn registry_reply(status: StatusCode, location: Option<&str>, body: Option<(&str, Vec<u8>)>) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    if let Some(location) = location
        && let Ok(value) = header::HeaderValue::from_str(location)
    {
        response.headers_mut().insert(header::LOCATION, value);
    }
    if let Some((content_type, bytes)) = body {
        if let Ok(value) = header::HeaderValue::from_str(content_type) {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        if let Ok(value) = header::HeaderValue::from_str(&digest_of(&bytes)) {
            response.headers_mut().insert("docker-content-digest", value);
        }
        *response.body_mut() = Body::from(bytes);
    }
    response
}

async fn registry_handle(AxumState(state): AxumState<Arc<Mutex<RegistryState>>>, request: Request<Body>) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let query = request.uri().query().unwrap_or_default().to_string();
    let body = to_bytes(request.into_body(), usize::MAX)
        .await
        .map(|bytes| bytes.to_vec())
        .unwrap_or_default();
    let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
    registry_route(&mut state, &method, &path, &query, body)
}

fn registry_route(state: &mut RegistryState, method: &Method, path: &str, query: &str, body: Vec<u8>) -> Response {
    if path == "/v2/" || path == "/v2" {
        return registry_reply(StatusCode::OK, None, None);
    }
    if let Some(id) = path.strip_prefix("/upload/") {
        return registry_upload(state, method, id, query, body);
    }
    let Some(rest) = path.strip_prefix("/v2/") else {
        return registry_reply(StatusCode::NOT_FOUND, None, None);
    };
    let segments: Vec<&str> = rest.split('/').collect();
    if let Some(position) = segments.windows(2).position(|pair| pair == ["blobs", "uploads"]) {
        let id = state.next_upload;
        state.next_upload += 1;
        state.uploads.insert(id.to_string(), Vec::new());
        return registry_reply(
            StatusCode::ACCEPTED,
            Some(&format!("/upload/{id}?repo={}", segments[..position].join("/"))),
            None,
        );
    }
    let Some(position) = segments
        .iter()
        .rposition(|segment| ["manifests", "blobs"].contains(segment))
    else {
        return registry_reply(StatusCode::NOT_FOUND, None, None);
    };
    if position + 1 >= segments.len() {
        return registry_reply(StatusCode::NOT_FOUND, None, None);
    }
    let repository = segments[..position].join("/");
    let selector = segments[position + 1..].join("/");
    if segments[position] == "blobs" {
        return match state.blobs.get(&selector) {
            Some(bytes) if method == Method::HEAD => {
                let mut reply = registry_reply(StatusCode::OK, None, None);
                if let Ok(value) = header::HeaderValue::from_str(&bytes.len().to_string()) {
                    reply.headers_mut().insert(header::CONTENT_LENGTH, value);
                }
                reply
            }
            Some(bytes) => registry_reply(StatusCode::OK, None, Some(("application/octet-stream", bytes.clone()))),
            None => registry_reply(StatusCode::NOT_FOUND, None, None),
        };
    }
    let key = (repository, selector);
    if method == Method::PUT {
        state.manifests.insert((key.0.clone(), digest_of(&body)), body.clone());
        state.manifests.insert(key, body);
        return registry_reply(StatusCode::CREATED, Some("/manifest"), None);
    }
    match state.manifests.get(&key) {
        Some(bytes) => registry_reply(
            StatusCode::OK,
            None,
            Some(("application/vnd.oci.image.manifest.v1+json", bytes.clone())),
        ),
        None => registry_reply(StatusCode::NOT_FOUND, None, None),
    }
}

fn registry_upload(state: &mut RegistryState, method: &Method, id: &str, query: &str, body: Vec<u8>) -> Response {
    let Some(buffer) = state.uploads.get_mut(id) else {
        return registry_reply(StatusCode::NOT_FOUND, None, None);
    };
    buffer.extend_from_slice(&body);
    if method == Method::PATCH {
        return registry_reply(StatusCode::ACCEPTED, Some(&format!("/upload/{id}")), None);
    }
    let data = std::mem::take(buffer);
    state.uploads.remove(id);
    let actual = digest_of(&data);
    let claimed = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find_map(|(name, value)| (name == "digest").then(|| value.replace("%3A", ":")));
    if claimed.as_deref() != Some(actual.as_str()) {
        return registry_reply(StatusCode::BAD_REQUEST, None, None);
    }
    state.blobs.insert(actual, data);
    registry_reply(StatusCode::CREATED, Some("/blob"), None)
}

pub fn lease_id(name: &str) -> LeaseId {
    LeaseId::from(name.to_string())
}

/// How long a request waits for an answer that never comes.
pub const SILENCE: Duration = Duration::from_millis(700);

/// A running worker on top of a [`World`], with a NATS client to talk to it.
pub struct Running {
    pub world: World,
    pub client: async_nats::Client,
    pub worker: trogon_sandbox_contract::WorkerId,
    cancel: CancellationToken,
    task: Option<JoinHandle<Result<(), trogon_sandbox_k8s_worker::WorkerError>>>,
}

impl Running {
    pub async fn start() -> Boxed<Self> {
        Self::on(World::start().await?).await
    }

    pub async fn on(world: World) -> Boxed<Self> {
        let client = world.nats.client().await;
        let worker = trogon_sandbox_contract::WorkerId::new("worker-under-test")?;
        let cancel = CancellationToken::new();
        let task = tokio::spawn(trogon_sandbox_k8s_worker::run_worker(
            client.clone(),
            world.provider.clone(),
            worker.clone(),
            cancel.clone(),
        ));
        // The worker subscribes before it serves, so wait until its subjects have listeners.
        tokio::time::sleep(Duration::from_millis(500)).await;
        Ok(Self {
            world,
            client,
            worker,
            cancel,
            task: Some(task),
        })
    }

    /// Stops the worker and waits for it to finish.
    pub async fn stop(&mut self) -> TestResult {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.await??;
        }
        Ok(())
    }

    /// Sends one JSON request, which may span several bus messages, and returns the JSON reply.
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

    /// Dispatches a lease to the worker.
    pub async fn dispatch(&self, lease: &str, request: Value) -> Boxed<Value> {
        let subject =
            trogon_sandbox_protocol::subjects::provider_dispatch(trogon_sandbox_contract::ProviderKind::K8sMicroVm);
        self.ask(subject, &json!({"lease_id": lease, "request": request})).await
    }

    /// Dispatches a lease built from the fixture request and checks that it was granted.
    pub async fn grant(&self, lease: &str) -> TestResult {
        let reply = self.dispatch(lease, serde_json::to_value(lease_request()?)?).await?;
        assert!(
            reply.get("provider_error").is_none() && reply.get("decode_error").is_none(),
            "not granted: {reply}"
        );
        Ok(())
    }

    /// Sends a verb for `lease`, with the lease filled in.
    pub async fn verb(
        &self,
        verb: trogon_sandbox_k8s_worker::K8sVerb,
        lease: &str,
        mut payload: Value,
    ) -> Boxed<Value> {
        payload["lease_id"] = json!(lease);
        self.ask(verb_subject(verb, lease), &payload).await
    }

    /// Asserts that a request gets no answer.
    pub async fn silent(&self, subject: String, payload: Vec<u8>) -> TestResult {
        let outcome = tokio::time::timeout(SILENCE, self.client.request(subject.clone(), payload.into())).await;
        assert!(!matches!(outcome, Ok(Ok(_))), "unexpected reply on {subject}");
        Ok(())
    }

    /// Publishes without waiting for an answer.
    pub async fn publish(&self, subject: String, payload: &Value) -> TestResult {
        self.client
            .publish(subject, serde_json::to_vec(payload)?.into())
            .await?;
        self.client.flush().await?;
        Ok(())
    }
}

/// The subject that carries `verb` for `lease`.
pub fn verb_subject(verb: trogon_sandbox_k8s_worker::K8sVerb, lease: &str) -> String {
    format!("{}{lease}", verb.subject().trim_end_matches('>'))
}

/// Waits for `check` to hold.
pub async fn until(mut check: impl FnMut() -> bool) -> TestResult {
    for _ in 0..100 {
        if check() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("condition not reached".into())
}
