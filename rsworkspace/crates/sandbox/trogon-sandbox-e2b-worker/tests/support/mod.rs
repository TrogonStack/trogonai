// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::BTreeMap;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as RoutePath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use trogon_sandbox_contract::Base64Data;
use trogon_sandbox_e2b_worker::{E2bConfig, EnvdTarget, LeaseSandbox};
use url::Url;

pub type Boxed = Box<dyn std::error::Error>;
pub type TestResult = Result<(), Boxed>;

/// Reads the fields of one reply variant, so a test names the variant it expects without branching.
pub fn outcome<T: DeserializeOwned>(reply: &impl Serialize) -> Result<T, Boxed> {
    Ok(serde_json::from_value(serde_json::to_value(reply)?)?)
}

/// The wire form of raw bytes.
pub fn data(bytes: &[u8]) -> Result<Base64Data, Boxed> {
    Ok(Base64Data::new(
        base64::engine::general_purpose::STANDARD.encode(bytes),
    )?)
}

/// The raw bytes of a wire payload.
pub fn decode(data: &Base64Data) -> Result<Vec<u8>, Boxed> {
    Ok(base64::engine::general_purpose::STANDARD.decode(data.as_str())?)
}

/// A sandbox whose envd is the HTTP server at `base_url`.
pub fn sandbox_at(base_url: &str) -> Result<LeaseSandbox, Boxed> {
    let config = E2bConfig::new(Url::parse("https://api.e2b.app")?, "e2b.app", "fixture-key")?
        .with_envd_url(Url::parse(base_url)?)?;
    Ok(LeaseSandbox {
        sandbox_id: "fixture-sandbox".into(),
        target: EnvdTarget::for_sandbox(&config, "fixture-sandbox", None, None)?,
        traffic_token: None,
        domain: "e2b.app".into(),
        exposed_ports: vec![],
    })
}

/// The sorted names directly under `directory`.
pub fn entry_names(directory: &Path) -> io::Result<Vec<String>> {
    let mut names = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<io::Result<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

struct Guest {
    root: PathBuf,
    calls: AtomicUsize,
    shell_calls: AtomicUsize,
    allow_shell: AtomicBool,
    before_shell: Mutex<Option<(PathBuf, Vec<u8>)>>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_owned())
}

fn ensure(condition: bool, message: &str) -> io::Result<()> {
    condition.then_some(()).ok_or_else(|| invalid(message))
}

impl Guest {
    fn path(&self, path: &str) -> io::Result<PathBuf> {
        let path = Path::new(path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    normalized.pop();
                }
                Component::CurDir => {}
                component => normalized.push(component.as_os_str()),
            }
        }
        normalized
            .starts_with(&self.root)
            .then_some(normalized)
            .ok_or_else(|| io::Error::new(io::ErrorKind::PermissionDenied, "path leaves the fixture"))
    }
}

pub struct Fixture {
    _directory: tempfile::TempDir,
    guest: Arc<Guest>,
    server: tokio::task::JoinHandle<()>,
    pub sandbox: LeaseSandbox,
}

/// Links the GNU tools that match the remote Linux guest into the fixture's tool directory.
#[cfg(target_os = "macos")]
fn link_gnu_tools(bin: &Path) -> io::Result<()> {
    for tool in [
        "dd", "mv", "rm", "mkdir", "rmdir", "chmod", "tar", "stat", "find", "cp", "readlink",
    ] {
        let gnu_tool = format!("g{tool}");
        let found = ["/opt/homebrew/bin", "/usr/local/bin"]
            .iter()
            .map(|directory| Path::new(directory).join(&gnu_tool))
            .find(|path| path.is_file());
        if let Some(path) = found {
            std::os::unix::fs::symlink(path, bin.join(tool))?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn link_gnu_tools(_bin: &Path) -> io::Result<()> {
    Ok(())
}

impl Fixture {
    pub async fn new() -> Result<Self, Boxed> {
        let directory = tempfile::TempDir::new()?;
        let root = directory.path().canonicalize()?;
        std::fs::create_dir(root.join("home"))?;
        std::fs::create_dir(root.join("tmp"))?;
        std::fs::create_dir(root.join("bin"))?;
        link_gnu_tools(&root.join("bin"))?;
        std::fs::write(
            root.join("shell-env"),
            b"export PATH=\"$SANDBOX_FIXTURE_BIN:/usr/bin:/bin\"\n",
        )?;
        let guest = Arc::new(Guest {
            root,
            calls: AtomicUsize::new(0),
            shell_calls: AtomicUsize::new(0),
            allow_shell: AtomicBool::new(true),
            before_shell: Mutex::new(None),
        });
        let router = Router::new()
            .route("/filesystem.Filesystem/{rpc}", post(filesystem))
            .route("/process.Process/Start", post(process))
            .route("/files", get(read).post(write))
            .with_state(guest.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = Url::parse(&format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let config = E2bConfig::new(endpoint.clone(), "e2b.app", "fixture-key")?.with_envd_url(endpoint)?;
        let target = EnvdTarget::for_sandbox(&config, "fixture", None, None)?;
        Ok(Self {
            _directory: directory,
            guest,
            server,
            sandbox: LeaseSandbox {
                sandbox_id: "fixture".into(),
                target,
                traffic_token: None,
                domain: "e2b.app".into(),
                exposed_ports: vec![],
            },
        })
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.guest.root.join(relative)
    }

    pub fn calls(&self) -> usize {
        self.guest.calls.load(Ordering::SeqCst)
    }

    pub fn shell_calls(&self) -> usize {
        self.guest.shell_calls.load(Ordering::SeqCst)
    }

    pub fn disable_shell(&self) {
        self.guest.allow_shell.store(false, Ordering::SeqCst);
    }

    pub fn overwrite_before_shell(&self, relative: &str, data: &[u8]) -> io::Result<()> {
        let path = self.guest.path(relative)?;
        *self.guest.before_shell.lock().unwrap_or_else(PoisonError::into_inner) = Some((path, data.to_vec()));
        Ok(())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn error(error: io::Error) -> Response {
    let status = match error.kind() {
        io::ErrorKind::NotFound => StatusCode::NOT_FOUND,
        io::ErrorKind::PermissionDenied => StatusCode::FORBIDDEN,
        io::ErrorKind::AlreadyExists => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    };
    (status, Json(json!({"message": error.to_string()}))).into_response()
}

fn entry(path: &Path) -> io::Result<Value> {
    let link = std::fs::symlink_metadata(path)?;
    let metadata = std::fs::metadata(path).unwrap_or_else(|_| link.clone());
    let kind = if metadata.is_dir() {
        "FILE_TYPE_DIRECTORY"
    } else {
        "FILE_TYPE_FILE"
    };
    let name = path.file_name().ok_or_else(|| invalid("path has no name"))?;
    Ok(json!({
        "name": name.to_string_lossy(), "path": path.to_string_lossy(),
        "type": kind, "size": metadata.len().to_string(), "mode": metadata.permissions().mode() & 0o7777,
        "isSymlink": link.is_symlink(),
        "symlinkTarget": std::fs::read_link(path).ok().map(|p| p.to_string_lossy().into_owned())
    }))
}

fn filesystem_call(guest: &Guest, rpc: &str, body: &Value) -> io::Result<Value> {
    let path = |key: &str| guest.path(body[key].as_str().unwrap_or(""));
    match rpc {
        "Stat" => Ok(json!({"entry": entry(&path("path")?)?})),
        "ListDir" => {
            let mut entries = Vec::new();
            for child in std::fs::read_dir(path("path")?)? {
                entries.push(entry(&child?.path())?);
            }
            Ok(json!({"entries": entries}))
        }
        "MakeDir" => {
            let path = path("path")?;
            std::fs::create_dir_all(&path)?;
            Ok(json!({"entry": entry(&path)?}))
        }
        "Move" => {
            let destination = path("destination")?;
            std::fs::rename(path("source")?, &destination)?;
            Ok(json!({"entry": entry(&destination)?}))
        }
        _ => {
            let path = path("path")?;
            if std::fs::symlink_metadata(&path)?.is_dir() {
                std::fs::remove_dir_all(path)?;
            } else {
                std::fs::remove_file(path)?;
            }
            Ok(json!({}))
        }
    }
}

async fn filesystem(
    State(guest): State<Arc<Guest>>,
    RoutePath(rpc): RoutePath<String>,
    Json(body): Json<Value>,
) -> Response {
    guest.calls.fetch_add(1, Ordering::SeqCst);
    match filesystem_call(&guest, &rpc, &body) {
        Ok(value) => Json(value).into_response(),
        Err(e) => error(e),
    }
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
    username: String,
}

async fn read(State(guest): State<Arc<Guest>>, Query(query): Query<FileQuery>) -> Response {
    guest.calls.fetch_add(1, Ordering::SeqCst);
    let run = || -> io::Result<Vec<u8>> {
        ensure(!query.username.is_empty(), "username is required")?;
        std::fs::read(guest.path(&query.path)?)
    };
    match run() {
        Ok(bytes) => bytes.into_response(),
        Err(e) => error(e),
    }
}

fn multipart_content(headers: &HeaderMap, body: &[u8]) -> io::Result<Vec<u8>> {
    let content_type = headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| invalid("missing content type"))?;
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .ok_or_else(|| invalid("missing boundary"))?
        .trim_matches('"');
    let start = body
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| invalid("missing part body"))?
        + 4;
    let end_marker = format!("\r\n--{boundary}--");
    let end = body
        .windows(end_marker.len())
        .rposition(|window| window == end_marker.as_bytes())
        .ok_or_else(|| invalid("missing closing boundary"))?;
    Ok(body[start..end].to_vec())
}

async fn write(
    State(guest): State<Arc<Guest>>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    guest.calls.fetch_add(1, Ordering::SeqCst);
    let run = || -> io::Result<()> {
        ensure(!query.username.is_empty(), "username is required")?;
        let content = multipart_content(&headers, &body)?;
        let path = guest.path(&query.path)?;
        std::fs::create_dir_all(path.parent().ok_or_else(|| invalid("path has no parent"))?)?;
        std::fs::write(path, content)
    };
    match run() {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error(e),
    }
}

fn stream(output: &[u8], stderr: &[u8], exit_code: i32) -> io::Result<Response> {
    let mut body = Vec::new();
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    for (flags, message) in [
        (0u8, json!({"event": {"start": {"pid": 1}}})),
        (
            0,
            json!({"event": {"data": {"stdout": b64(output), "stderr": b64(stderr)}}}),
        ),
        (0, json!({"event": {"end": {"exited": true, "exitCode": exit_code}}})),
        (2, json!({})),
    ] {
        let payload = serde_json::to_vec(&message)?;
        body.push(flags);
        body.extend_from_slice(&u32::try_from(payload.len()).map_err(io::Error::other)?.to_be_bytes());
        body.extend(payload);
    }
    Ok(([("content-type", "application/connect+json")], body).into_response())
}

async fn process_call(guest: &Guest, body: &[u8]) -> io::Result<Response> {
    let pending = guest.before_shell.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some((path, data)) = pending {
        std::fs::write(path, data)?;
    }
    let message: Value = serde_json::from_slice(body.get(5..).ok_or_else(|| invalid("short frame"))?)?;
    ensure(message["process"]["cmd"] == "/bin/bash", "fixture runs bash only")?;
    let args: Vec<String> = serde_json::from_value(message["process"]["args"].clone())?;
    for arg in args.iter().filter(|arg| arg.starts_with('/')) {
        guest.path(arg)?;
    }
    let extra_env: BTreeMap<String, String> =
        serde_json::from_value(message["process"]["envs"].clone()).unwrap_or_default();
    ensure(
        extra_env.is_empty(),
        "fixture commands must not inherit arbitrary environment values",
    )?;
    let mut command = tokio::process::Command::new("/bin/bash");
    command
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", guest.root.join("home"))
        .env("TMPDIR", guest.root.join("tmp"))
        .env("BASH_ENV", guest.root.join("shell-env"))
        .env("SANDBOX_FIXTURE_BIN", guest.root.join("bin"))
        .env("LC_ALL", "C")
        .current_dir(&guest.root)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(30), command.output())
        .await
        .map_err(io::Error::other)??;
    stream(&output.stdout, &output.stderr, output.status.code().unwrap_or(-1))
}

async fn process(State(guest): State<Arc<Guest>>, body: Bytes) -> Response {
    guest.calls.fetch_add(1, Ordering::SeqCst);
    guest.shell_calls.fetch_add(1, Ordering::SeqCst);
    if !guest.allow_shell.load(Ordering::SeqCst) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match process_call(&guest, &body).await {
        Ok(response) => response,
        Err(e) => error(e),
    }
}
