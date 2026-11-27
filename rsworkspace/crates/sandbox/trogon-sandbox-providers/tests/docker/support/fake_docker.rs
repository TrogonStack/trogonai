//! An in-process Docker daemon on a unix socket, with just enough of the Engine
//! API for the engine adapter and the local provider.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::error::Error;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

/// Container ids are `id-<name>`; a name starting with one of these makes the matching call fail.
pub const FAIL_CREATE: &str = "fail-create";
pub const FAIL_START: &str = "fail-start";
pub const FAIL_INSPECT: &str = "fail-inspect";
pub const FAIL_KILL: &str = "fail-kill";
pub const FAIL_REMOVE: &str = "fail-remove";
pub const FAIL_COMMIT: &str = "fail-commit";
pub const FAIL_EXPORT: &str = "fail-export";
pub const FAIL_CHECKPOINT: &str = "fail-checkpoint";
pub const HIDE_CHECKPOINT: &str = "hide-checkpoint";
pub const FAIL_WAIT: &str = "fail-wait";
pub const EMPTY_WAIT: &str = "empty-wait";
pub const NO_STATE: &str = "no-state";
pub const FAIL_RESTORE: &str = "fail-restore";
/// The repository name an import fails for.
pub const FAIL_IMPORT_REPO: &str = "fail-import";
/// The label key listing fails for.
pub const FAIL_LIST_LABEL: &str = "fail-list";

pub const EXPORT_BYTES: &[u8] = b"rootfs-tar-bytes";

/// Where the egress sidecar writes its readiness marker once its rules are installed.
const SIDECAR_READY_PATH: &str = "/etc/sandbox-egress-proxy-ready";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    pub target: String,
    pub body: String,
}

#[derive(Debug, Clone)]
struct Container {
    name: String,
    labels: BTreeMap<String, String>,
    status: String,
    exit_code: i64,
    ports: Vec<String>,
    config: Value,
}

#[derive(Default)]
struct State {
    containers: BTreeMap<String, Container>,
    requests: Vec<RecordedRequest>,
    checkpoints: Vec<String>,
    fail_sidecars: bool,
    fail_sidecar_rules: bool,
    fail_kills: bool,
    fail_creates: bool,
}

pub struct FakeDocker {
    socket_path: PathBuf,
    state: Arc<Mutex<State>>,
    task: JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for FakeDocker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn reply(status: StatusCode, body: impl Into<Bytes>) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body.into()));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert("content-type", http::HeaderValue::from_static("application/json"));
    response
}

fn json_reply(status: StatusCode, value: &Value) -> Response<Full<Bytes>> {
    reply(status, value.to_string())
}

fn error_reply(status: StatusCode, message: &str) -> Response<Full<Bytes>> {
    json_reply(status, &json!({ "message": message }))
}

fn no_content() -> Response<Full<Bytes>> {
    reply(StatusCode::NO_CONTENT, Bytes::new())
}

impl FakeDocker {
    pub async fn start() -> Result<Self, Box<dyn Error>> {
        let dir = tempfile::tempdir()?;
        let socket_path = dir.path().join("docker.sock");
        let listener = UnixListener::bind(&socket_path)?;
        let state = Arc::new(Mutex::new(State::default()));
        let served = Arc::clone(&state);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = Arc::clone(&served);
                tokio::spawn(async move {
                    let service = service_fn(move |request| {
                        let state = Arc::clone(&state);
                        async move { Ok::<_, Infallible>(handle(state, request).await) }
                    });
                    let _ = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Ok(Self {
            socket_path,
            state,
            task,
            _dir: dir,
        })
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket_path.clone()
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        lock(&self.state).requests.clone()
    }

    /// Requests whose method and target contain `needle`, in arrival order.
    pub fn requests_matching(&self, needle: &str) -> Vec<RecordedRequest> {
        self.requests()
            .into_iter()
            .filter(|request| format!("{} {}", request.method, request.target).contains(needle))
            .collect()
    }

    pub fn set_state(&self, container_id: &str, status: &str, exit_code: i64) {
        if let Some(container) = lock(&self.state).containers.get_mut(container_id) {
            container.status = status.to_string();
            container.exit_code = exit_code;
        }
    }

    /// Makes every `*-egress-proxy` container fail to create.
    pub fn fail_sidecars(&self) {
        lock(&self.state).fail_sidecars = true;
    }

    /// Makes every `*-egress-proxy` container exit at start, as its shell does when a firewall rule fails, without
    /// writing its readiness marker.
    pub fn fail_sidecar_rules(&self) {
        lock(&self.state).fail_sidecar_rules = true;
    }

    /// Makes every kill fail.
    pub fn fail_kills(&self) {
        lock(&self.state).fail_kills = true;
    }

    /// Makes every create fail.
    pub fn fail_creates(&self) {
        lock(&self.state).fail_creates = true;
    }

    /// Bodies of the `containers/create` requests, in arrival order, with the `name` query.
    pub fn create_requests(&self) -> Vec<(String, Value)> {
        self.requests_matching("/containers/create")
            .into_iter()
            .map(|request| {
                let name = request.target.split("name=").nth(1).unwrap_or_default().to_string();
                (name, serde_json::from_str(&request.body).unwrap_or(Value::Null))
            })
            .collect()
    }

    pub fn container_ids(&self) -> Vec<String> {
        lock(&self.state).containers.keys().cloned().collect()
    }

    pub fn labels_of(&self, container_id: &str) -> BTreeMap<String, String> {
        lock(&self.state)
            .containers
            .get(container_id)
            .map(|container| container.labels.clone())
            .unwrap_or_default()
    }

    pub fn add_container(&self, name: &str, labels: &[(&str, &str)]) {
        lock(&self.state).containers.insert(
            format!("id-{name}"),
            Container {
                name: name.to_string(),
                labels: labels
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
                status: "running".to_string(),
                exit_code: 0,
                ports: vec![],
                config: Value::Null,
            },
        );
    }
}

fn failing(name: &str, marker: &str) -> bool {
    name.starts_with(marker)
}

async fn handle(state: Arc<Mutex<State>>, request: Request<Incoming>) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let body = match body.collect().await {
        Ok(collected) => String::from_utf8_lossy(&collected.to_bytes()).into_owned(),
        Err(_) => String::new(),
    };
    let target = parts
        .uri
        .path_and_query()
        .map(|target| target.to_string())
        .unwrap_or_default();
    lock(&state).requests.push(RecordedRequest {
        method: parts.method.to_string(),
        target: target.clone(),
        body: body.clone(),
    });

    let path = parts.uri.path();
    let path = match path.strip_prefix("/v") {
        Some(rest) => rest.find('/').map_or("", |index| &rest[index..]),
        None => path,
    };
    let query: Vec<(String, String)> = serde_urlencoded::from_str(parts.uri.query().unwrap_or("")).unwrap_or_default();
    let query_value = |key: &str| {
        query
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();

    match (&parts.method, segments.as_slice()) {
        (&Method::GET, ["info"]) => json_reply(StatusCode::OK, &json!({ "ExperimentalBuild": true })),
        (&Method::POST, ["containers", "create"]) => create(&state, &query_value("name"), &body),
        (&Method::GET, ["containers", "json"]) => list(&state, &query_value("filters")),
        (&Method::GET, ["containers", id, "json"]) => inspect(&state, id),
        (&Method::POST, ["containers", id, "start"]) => start(&state, id, &query_value("checkpoint")),
        (&Method::POST, ["containers", id, "kill"]) => kill(&state, id),
        (&Method::DELETE, ["containers", id]) => remove(&state, id),
        (&Method::POST, ["containers", id, "wait"]) => wait(&state, id),
        (&Method::GET, ["containers", id, "export"]) => export(&state, id),
        (&Method::POST, ["containers", id, "checkpoints"]) => create_checkpoint(&state, id, &body),
        (&Method::GET, ["containers", id, "checkpoints"]) => list_checkpoints(&state, id),
        (&Method::POST, ["commit"]) => commit(&query_value("container")),
        (&Method::POST, ["images", "create"]) => import(&query_value("repo")),
        _ => error_reply(StatusCode::NOT_FOUND, "no such route"),
    }
}

fn create(state: &Mutex<State>, name: &str, body: &str) -> Response<Full<Bytes>> {
    let (fail_sidecars, fail_creates) = {
        let state = lock(state);
        (state.fail_sidecars, state.fail_creates)
    };
    let sidecar_refused = name.ends_with("-egress-proxy") && fail_sidecars;
    if failing(name, FAIL_CREATE) || sidecar_refused || fail_creates {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "create refused");
    }
    let config: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let labels = config["Labels"]
        .as_object()
        .map(|labels| {
            labels
                .iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    let ports = config["ExposedPorts"]
        .as_object()
        .map(|ports| ports.keys().cloned().collect())
        .unwrap_or_default();
    let id = format!("id-{name}");
    lock(state).containers.insert(
        id.clone(),
        Container {
            name: name.to_string(),
            labels,
            status: "created".to_string(),
            exit_code: 0,
            ports,
            config,
        },
    );
    json_reply(StatusCode::CREATED, &json!({ "Id": id, "Warnings": [] }))
}

fn start(state: &Mutex<State>, id: &str, checkpoint: &str) -> Response<Full<Bytes>> {
    let mut state = lock(state);
    let fail_sidecar_rules = state.fail_sidecar_rules;
    let Some(container) = state.containers.get_mut(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    if failing(&container.name, FAIL_START) || (!checkpoint.is_empty() && failing(&container.name, FAIL_RESTORE)) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "start refused");
    }
    container.status = "running".to_string();
    if container.name.ends_with("-egress-proxy") {
        if fail_sidecar_rules {
            container.status = "exited".to_string();
            container.exit_code = 1;
        } else {
            write_sidecar_ready_marker(&container.config);
        }
    }
    no_content()
}

/// Plays the sidecar shell's last step before the proxy starts: the nonce its command writes to the readiness
/// marker lands in the host file bound there.
fn write_sidecar_ready_marker(config: &Value) {
    let target = format!(":{SIDECAR_READY_PATH}");
    let host_path = config["HostConfig"]["Binds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|bind| bind.strip_suffix(&target));
    let script = config["Cmd"]
        .as_array()
        .and_then(|cmd| cmd.last())
        .and_then(Value::as_str)
        .unwrap_or_default();
    let nonce = script
        .split_once(&format!(" > {SIDECAR_READY_PATH}"))
        .and_then(|(before, _)| before.rsplit(' ').next());
    if let (Some(host_path), Some(nonce)) = (host_path, nonce) {
        let _ = std::fs::write(host_path, nonce);
    }
}

fn inspect(state: &Mutex<State>, id: &str) -> Response<Full<Bytes>> {
    let state = lock(state);
    let Some(container) = state.containers.get(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    if failing(&container.name, FAIL_INSPECT) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "inspect refused");
    }
    let ports: serde_json::Map<String, Value> = container
        .ports
        .iter()
        .map(|port| {
            let host_port = port.split('/').next().unwrap_or("0");
            (
                port.clone(),
                json!([{ "HostIp": "127.0.0.1", "HostPort": format!("3{host_port}") }]),
            )
        })
        .collect();
    let mut detail = json!({
        "Id": id,
        "Name": container.name,
        "NetworkSettings": { "Ports": ports },
    });
    if !failing(&container.name, NO_STATE) {
        detail["State"] = json!({ "Status": container.status, "ExitCode": container.exit_code });
    }
    json_reply(StatusCode::OK, &detail)
}

fn list(state: &Mutex<State>, filters: &str) -> Response<Full<Bytes>> {
    let filters: Value = serde_json::from_str(filters).unwrap_or(Value::Null);
    let wanted: Vec<(String, String)> = filters["label"]
        .as_array()
        .map(|labels| {
            labels
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|label| label.split_once('='))
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if wanted.iter().any(|(key, _)| key == FAIL_LIST_LABEL) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "list refused");
    }
    let listed: Vec<Value> = lock(state)
        .containers
        .iter()
        .filter(|(_, container)| {
            wanted
                .iter()
                .all(|(key, value)| container.labels.get(key) == Some(value))
        })
        .map(|(id, container)| json!({ "Id": id, "Labels": container.labels }))
        .collect();
    json_reply(StatusCode::OK, &Value::Array(listed))
}

fn kill(state: &Mutex<State>, id: &str) -> Response<Full<Bytes>> {
    let mut state = lock(state);
    let fail_kills = state.fail_kills;
    let Some(container) = state.containers.get_mut(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    if failing(&container.name, FAIL_KILL) || fail_kills {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "kill refused");
    }
    container.status = "exited".to_string();
    no_content()
}

fn remove(state: &Mutex<State>, id: &str) -> Response<Full<Bytes>> {
    let mut state = lock(state);
    let Some(container) = state.containers.get(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    if failing(&container.name, FAIL_REMOVE) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "remove refused");
    }
    state.containers.remove(id);
    no_content()
}

fn wait(state: &Mutex<State>, id: &str) -> Response<Full<Bytes>> {
    let state = lock(state);
    let Some(container) = state.containers.get(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    if failing(&container.name, FAIL_WAIT) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "wait refused");
    }
    if failing(&container.name, EMPTY_WAIT) {
        return reply(StatusCode::OK, Bytes::new());
    }
    json_reply(StatusCode::OK, &json!({ "StatusCode": container.exit_code }))
}

fn export(state: &Mutex<State>, id: &str) -> Response<Full<Bytes>> {
    let state = lock(state);
    let Some(container) = state.containers.get(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    if failing(&container.name, FAIL_EXPORT) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "export refused");
    }
    reply(StatusCode::OK, Bytes::from_static(EXPORT_BYTES))
}

fn create_checkpoint(state: &Mutex<State>, id: &str, body: &str) -> Response<Full<Bytes>> {
    let mut state = lock(state);
    let Some(container) = state.containers.get(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    if failing(&container.name, FAIL_CHECKPOINT) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "checkpoint refused");
    }
    let options: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if let Some(name) = options["CheckpointID"].as_str() {
        state.checkpoints.push(name.to_string());
    }
    reply(StatusCode::CREATED, Bytes::new())
}

fn list_checkpoints(state: &Mutex<State>, id: &str) -> Response<Full<Bytes>> {
    let state = lock(state);
    let Some(container) = state.containers.get(id) else {
        return error_reply(StatusCode::NOT_FOUND, "no such container");
    };
    let names: Vec<Value> = if failing(&container.name, HIDE_CHECKPOINT) {
        vec![]
    } else {
        state.checkpoints.iter().map(|name| json!({ "Name": name })).collect()
    };
    json_reply(StatusCode::OK, &Value::Array(names))
}

fn commit(container: &str) -> Response<Full<Bytes>> {
    if container.contains(FAIL_COMMIT) {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "commit refused");
    }
    json_reply(StatusCode::CREATED, &json!({ "Id": "sha256:committed" }))
}

fn import(repo: &str) -> Response<Full<Bytes>> {
    if repo == FAIL_IMPORT_REPO {
        return error_reply(StatusCode::INTERNAL_SERVER_ERROR, "import refused");
    }
    reply(
        StatusCode::OK,
        "{\"status\":\"importing\"}\r\n{\"status\":\"done\",\"id\":\"sha256:imported\"}\r\n",
    )
}
