// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! A Daytona stand-in whose sandbox is the host: toolbox commands run in `/bin/sh` on this machine, files live
//! where the commands put them, and a session is a child process fed through its stdin.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, Command};
use trogon_sandbox_contract::{CommandOutput, LeaseId, PollCommandResponse, RunCommandRequest, RunCommandResponse};
use trogon_sandbox_daytona_worker::{
    DaytonaApi, DaytonaApiKey, DaytonaConfig, DaytonaEndpoint, DaytonaExec, DaytonaOrganizationId, LeaseSandbox,
};
use trogon_std::env::{ReadEnv, SystemEnv};

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;
pub type TestResult = Result<(), Boxed>;

/// The sandbox the guest stands for.
pub const SANDBOX: &str = "host-1";

/// The directory commands start in; the host has no workspace directory.
pub const CWD: &str = "/tmp";

/// The identifier of every command a session starts.
pub const COMMAND_ID: &str = "actual-command";

/// One request the guest received.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// An answer a test forces for downloads whose path contains `needle`.
#[derive(Clone)]
struct Fault {
    needle: String,
    status: u16,
    remaining: usize,
}

#[derive(Default)]
struct State {
    log: Vec<Seen>,
    children: HashMap<String, Child>,
    inputs: HashMap<String, ChildStdin>,
    download_faults: Vec<Fault>,
    input_fault: Option<u16>,
    path: OsString,
}

pub struct Guest {
    pub url: String,
    state: Arc<tokio::sync::Mutex<State>>,
    journal: Arc<Mutex<Vec<Seen>>>,
    _tools: tempfile::TempDir,
    task: tokio::task::JoinHandle<()>,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A directory of the tools the command wrappers need, under the names a Linux sandbox gives them. Where the host
/// ships the GNU versions under a `g` prefix, the links point at those.
fn tool_directory() -> Result<(tempfile::TempDir, OsString), Boxed> {
    let directory = tempfile::tempdir()?;
    let search: Vec<PathBuf> = std::env::split_paths(&SystemEnv.var_os("PATH").unwrap_or_default()).collect();
    for tool in ["timeout", "base64", "mkfifo", "rm", "sleep", "tee"] {
        let found = [format!("g{tool}"), tool.to_owned()]
            .into_iter()
            .find_map(|name| search.iter().map(|path| path.join(&name)).find(|path| path.is_file()))
            .ok_or_else(|| format!("the guest needs {tool} on the PATH"))?;
        std::os::unix::fs::symlink(found, directory.path().join(tool))?;
    }
    let path = std::env::join_paths(std::iter::once(directory.path().to_path_buf()).chain(search))?;
    Ok((directory, path))
}

impl Guest {
    pub async fn start() -> Result<Self, Boxed> {
        let (tools, path) = tool_directory()?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let state = Arc::new(tokio::sync::Mutex::new(State {
            path,
            ..State::default()
        }));
        let journal: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let (served, written, base) = (state.clone(), journal.clone(), url.clone());
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { return };
                        connections.spawn(serve(socket, served.clone(), written.clone(), base.clone()));
                    }
                    Some(_) = connections.join_next() => {}
                }
            }
        });
        Ok(Self {
            url,
            state,
            journal,
            _tools: tools,
            task,
        })
    }

    pub fn api(&self) -> Result<DaytonaApi, Boxed> {
        let config = DaytonaConfig::new(
            DaytonaEndpoint::parse(&self.url)?,
            DaytonaApiKey::new(&format!("key-{}", uuid::Uuid::new_v4().simple()))?,
            Some(DaytonaOrganizationId::new("org-1")?),
        );
        Ok(DaytonaApi::new(config, Duration::from_secs(10))?)
    }

    pub fn exec(&self) -> Result<DaytonaExec, Boxed> {
        Ok(DaytonaExec::new(self.api()?))
    }

    /// An executor that runs detached commands under the supervisor named by `SANDBOX_DAYTONA_COMMAND_SUPERVISOR_ARTIFACT`
    /// and `SANDBOX_DAYTONA_COMMAND_SUPERVISOR_SHA256`. `None` when either is unset or the host is not Linux, so a
    /// test that needs a real supervisor skips itself.
    pub fn exec_with_supervisor(&self) -> Result<Option<DaytonaExec>, Boxed> {
        if !cfg!(target_os = "linux") {
            return Ok(None);
        }
        let (Some(path), Ok(pin)) = (
            SystemEnv.var_os("SANDBOX_DAYTONA_COMMAND_SUPERVISOR_ARTIFACT"),
            SystemEnv.var("SANDBOX_DAYTONA_COMMAND_SUPERVISOR_SHA256"),
        ) else {
            return Ok(None);
        };
        let runtime = trogon_sandbox_daytona_worker::OrdinaryCommandRuntime::load(std::path::Path::new(&path), &pin)?;
        Ok(Some(self.exec()?.with_runtime(runtime)))
    }

    pub fn sandbox(&self) -> LeaseSandbox {
        LeaseSandbox { id: SANDBOX.to_owned() }
    }

    /// Requests received so far, oldest first.
    pub fn seen(&self) -> Vec<Seen> {
        locked(&self.journal).clone()
    }

    /// The requests with `method` whose path ends with `suffix`.
    pub fn calls(&self, method: &str, suffix: &str) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|seen| seen.method == method && seen.path.ends_with(suffix))
            .collect()
    }

    /// The command lines of every toolbox `process/execute` call.
    pub fn executed(&self) -> Vec<String> {
        self.calls("POST", "/process/execute")
            .iter()
            .filter_map(|seen| seen.json()["command"].as_str().map(str::to_owned))
            .collect()
    }

    /// The command lines every session was asked to run.
    pub fn session_commands(&self) -> Vec<String> {
        self.seen()
            .iter()
            .filter(|seen| {
                seen.method == "POST" && seen.path.ends_with("/exec") && seen.path.contains("/process/session/")
            })
            .filter_map(|seen| seen.json()["command"].as_str().map(str::to_owned))
            .collect()
    }

    pub fn session_deletes(&self) -> usize {
        self.seen()
            .iter()
            .filter(|seen| seen.method == "DELETE" && seen.path.contains("/process/session/"))
            .count()
    }

    /// The input lines sent to sessions.
    pub fn inputs(&self) -> Vec<String> {
        self.calls("POST", "/input")
            .iter()
            .filter_map(|seen| seen.json()["data"].as_str().map(str::to_owned))
            .collect()
    }

    /// Paths of the files the worker deleted.
    pub fn deleted_files(&self) -> Vec<String> {
        self.calls("DELETE", "/files")
            .iter()
            .filter_map(|seen| seen.query.get("path").cloned())
            .collect()
    }

    /// Downloads whose path contains `needle` answer `status` the next `times` times.
    pub async fn fail_downloads(&self, needle: &str, status: u16, times: usize) {
        self.state.lock().await.download_faults.push(Fault {
            needle: needle.to_owned(),
            status,
            remaining: times,
        });
    }

    /// Every session input call answers `status` until cleared.
    pub async fn fail_inputs(&self, status: Option<u16>) {
        self.state.lock().await.input_fault = status;
    }

    /// Forgets every session's input pipe, as a toolbox that lost its session records would.
    pub async fn lose_sessions(&self) {
        self.state.lock().await.inputs.clear();
    }

    /// Whether a session's command is still running.
    pub async fn session_running(&self) -> bool {
        let mut state = self.state.lock().await;
        state
            .children
            .values_mut()
            .any(|child| matches!(child.try_wait(), Ok(None)))
    }

    /// Waits until no session command runs any longer.
    pub async fn settle(&self) -> bool {
        for _ in 0..600 {
            if !self.session_running().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    /// Runs `command` on the guest and returns its exit code and output.
    pub async fn shell(&self, command: &str) -> Result<(i32, String), Boxed> {
        let path = self.state.lock().await.path.clone();
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .env("PATH", path)
            .stdin(Stdio::null())
            .output()
            .await?;
        Ok((output.status.code().unwrap_or(-1), String::from_utf8(output.stdout)?))
    }
}

impl Drop for Guest {
    fn drop(&mut self) {
        self.task.abort();
        if let Ok(mut state) = self.state.try_lock() {
            for child in state.children.values_mut() {
                if let Some(pid) = child.id() {
                    let _ = std::process::Command::new("kill")
                        .args(["-KILL", "--", &format!("-{pid}")])
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }
    }
}

/// A request that runs `command` in a lease of this guest; `change` adjusts the wire form first.
pub fn run_request(command: &[&str], change: impl FnOnce(&mut Value)) -> Result<RunCommandRequest, serde_json::Error> {
    let mut wire = json!({"lease_id": LEASE, "command": command, "cwd": CWD, "timeout_seconds": 10});
    change(&mut wire);
    serde_json::from_value(wire)
}

pub const LEASE: &str = "lease-host";

pub fn lease_id() -> LeaseId {
    LeaseId::new(LEASE)
}

pub fn completed(response: RunCommandResponse) -> Result<CommandOutput, Boxed> {
    match response {
        RunCommandResponse::Completed(output) => Ok(output),
        other => Err(format!("not completed: {other:?}").into()),
    }
}

/// Starts a detached command and returns its execution id.
pub async fn start(guest: &Guest, exec: &DaytonaExec, request: &RunCommandRequest) -> Result<String, Boxed> {
    match exec.handle_run_command(&guest.sandbox(), request).await {
        RunCommandResponse::Started(started) => Ok(started.execution_id.as_str().to_owned()),
        other => Err(format!("not started: {other:?}").into()),
    }
}

/// Polls until the command finishes.
pub async fn finish(guest: &Guest, exec: &DaytonaExec, execution: &str) -> Result<CommandOutput, Boxed> {
    for _ in 0..600 {
        match exec.handle_poll_command(&guest.sandbox(), LEASE, execution).await {
            PollCommandResponse::Completed(output) => return Ok(output),
            PollCommandResponse::Running => tokio::time::sleep(Duration::from_millis(50)).await,
            other => return Err(format!("poll failed: {other:?}").into()),
        }
    }
    Err("the command never finished".into())
}

pub async fn within<T>(future: impl std::future::Future<Output = T>) -> Result<T, Boxed> {
    Ok(tokio::time::timeout(Duration::from_secs(60), future).await?)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<Seen> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
        let size = socket.read(&mut chunk).await.ok()?;
        if size == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..size]);
    };
    let header = String::from_utf8_lossy(&bytes[..end]).into_owned();
    let mut first = header.lines().next()?.split_whitespace();
    let method = first.next()?.to_owned();
    let target = first.next()?.to_owned();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let length = header
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < end + length {
        let size = socket.read(&mut chunk).await.ok()?;
        if size == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..size]);
    }
    Some(Seen {
        method,
        path: path.to_owned(),
        query: url::form_urlencoded::parse(query.as_bytes())
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect(),
        body: bytes[end..end + length].to_vec(),
    })
}

fn json_reply(status: u16, value: Value) -> (u16, Vec<u8>) {
    (status, value.to_string().into_bytes())
}

async fn answer(request: &Seen, state: &tokio::sync::Mutex<State>, base: &str) -> (u16, Vec<u8>) {
    let path = request.path.as_str();
    let mut state = state.lock().await;
    if request.method == "GET" && path.ends_with("/toolbox-proxy-url") {
        return json_reply(200, json!({"url": format!("{base}/box")}));
    }
    if request.method == "POST" && path.ends_with("/process/execute") {
        let line = request.json()["command"].as_str().unwrap_or_default().to_owned();
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(line)
            .env("PATH", &state.path)
            .stdin(Stdio::null())
            .output()
            .await;
        return match output {
            Ok(output) => {
                let mut result = output.stdout;
                result.extend(output.stderr);
                json_reply(
                    200,
                    json!({"exitCode": output.status.code().unwrap_or(-1), "result": String::from_utf8_lossy(&result)}),
                )
            }
            Err(error) => (500, error.to_string().into_bytes()),
        };
    }
    if request.method == "POST" && path.ends_with("/process/session") {
        return json_reply(200, json!({}));
    }
    if request.method == "POST" && path.contains("/process/session/") && path.ends_with("/exec") {
        let session = path.trim_end_matches("/exec").to_owned();
        let line = request.json()["command"].as_str().unwrap_or_default().to_owned();
        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(line)
            .env("PATH", &state.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn();
        return match spawned {
            Ok(mut child) => {
                if let Some(stdin) = child.stdin.take() {
                    state.inputs.insert(session.clone(), stdin);
                }
                state.children.insert(session, child);
                json_reply(200, json!({"cmdId": COMMAND_ID}))
            }
            Err(error) => (500, error.to_string().into_bytes()),
        };
    }
    if request.method == "POST" && path.ends_with(&format!("/command/{COMMAND_ID}/input")) {
        if let Some(status) = state.input_fault {
            return (status, b"input refused".to_vec());
        }
        let session = path.split("/command/").next().unwrap_or_default().to_owned();
        let data = request.json()["data"].as_str().unwrap_or_default().to_owned();
        let Some(input) = state.inputs.get_mut(&session) else {
            return (404, b"session not found".to_vec());
        };
        let line = if data.ends_with('\n') {
            data
        } else {
            format!("{data}\n")
        };
        return match input.write_all(line.as_bytes()).await {
            Ok(()) => json_reply(200, json!({})),
            Err(_) => (503, b"input write failed".to_vec()),
        };
    }
    if request.method == "DELETE" && path.contains("/process/session/") {
        state.inputs.remove(path);
        return json_reply(200, json!({}));
    }
    if request.method == "POST" && path.ends_with("/files/bulk-upload") {
        return match parse_upload(&request.body) {
            Some((target, content)) => match std::fs::write(&target, content) {
                Ok(()) => json_reply(200, json!({})),
                Err(error) => (500, error.to_string().into_bytes()),
            },
            None => (400, b"malformed upload".to_vec()),
        };
    }
    if request.method == "POST" && path.ends_with("/files/permissions") {
        let target = request.query.get("path").cloned().unwrap_or_default();
        let mode = request
            .query
            .get("mode")
            .and_then(|mode| u32::from_str_radix(mode, 8).ok())
            .unwrap_or(0o600);
        return match std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)) {
            Ok(()) => json_reply(200, json!({})),
            Err(_) => (404, b"file not found".to_vec()),
        };
    }
    if path.ends_with("/files/download") || path.ends_with("/files") {
        let file = request.query.get("path").cloned().unwrap_or_default();
        if request.method == "GET" {
            for fault in &mut state.download_faults {
                if fault.remaining > 0 && file.contains(&fault.needle) {
                    fault.remaining -= 1;
                    return (fault.status, b"read refused".to_vec());
                }
            }
            return match std::fs::read(&file) {
                Ok(bytes) => (200, bytes),
                Err(_) => (404, b"file not found".to_vec()),
            };
        }
        if request.method == "DELETE" {
            let _ = std::fs::remove_file(&file);
            return json_reply(200, json!({}));
        }
    }
    json_reply(404, json!({"message": "unknown guest operation"}))
}

/// The path and content of the one file a multipart upload carries.
fn parse_upload(body: &[u8]) -> Option<(String, Vec<u8>)> {
    let find = |haystack: &[u8], needle: &[u8], from: usize| {
        haystack
            .get(from..)?
            .windows(needle.len())
            .position(|window| window == needle)
            .map(|at| at + from)
    };
    let marker = b"name=\"files[0].path\"\r\n\r\n";
    let start = find(body, marker, 0)? + marker.len();
    let end = find(body, b"\r\n--", start)?;
    let target = String::from_utf8(body[start..end].to_vec()).ok()?;
    let file = find(body, b"name=\"files[0].file\"", end)?;
    let content_start = find(body, b"\r\n\r\n", file)? + 4;
    let content_end = body.len().checked_sub(b"\r\n--".len())?;
    let content_end = (content_start..=content_end)
        .rev()
        .find(|at| body[*at..].starts_with(b"\r\n--"))?;
    Some((target, body[content_start..content_end].to_vec()))
}

async fn serve(
    mut socket: tokio::net::TcpStream,
    state: Arc<tokio::sync::Mutex<State>>,
    journal: Arc<Mutex<Vec<Seen>>>,
    base: String,
) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    locked(&journal).push(request.clone());
    let (status, body) = answer(&request, &state, &base).await;
    let header = format!(
        "HTTP/1.1 {status} Guest\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if socket.write_all(header.as_bytes()).await.is_ok() {
        let _ = socket.write_all(&body).await;
    }
}
