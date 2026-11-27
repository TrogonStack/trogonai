use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, ensure};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_protocol::DispatchSigner;

const MAX_HTTP_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Default)]
pub struct Allocation {
    pub creates: usize,
    pub container: Option<String>,
    pub started: bool,
    baseline_exec: Option<String>,
    pub baseline_complete: bool,
    pub fault_injected: bool,
}

struct Control {
    owner: String,
    fault_lease: String,
    allocations: HashMap<String, Allocation>,
    failed_connections: usize,
}

pub struct Proxy {
    _directory: tempfile::TempDir,
    socket: PathBuf,
    control: Arc<Mutex<Control>>,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Proxy {
    pub async fn start(owner: &str, control_lease: &str, fault_lease: &str) -> anyhow::Result<Self> {
        let upstream = trogon_std::env::ReadEnv::var(&trogon_std::env::SystemEnv, "DOCKER_HOST")
            .unwrap_or_else(|_| "unix:///var/run/docker.sock".into());
        let upstream = PathBuf::from(
            upstream
                .strip_prefix("unix://")
                .context("this actual Docker API fault fixture requires a Unix Docker endpoint")?,
        );
        drop(tokio::time::timeout(Duration::from_secs(5), UnixStream::connect(&upstream)).await??);
        let directory = tempfile::Builder::new().prefix("docker-fault-").tempdir_in("/tmp")?;
        let socket = directory.path().join("api.sock");
        let listener = UnixListener::bind(&socket)?;
        let control = Arc::new(Mutex::new(Control {
            owner: owner.into(),
            fault_lease: fault_lease.into(),
            allocations: HashMap::from([
                (control_lease.into(), Allocation::default()),
                (fault_lease.into(), Allocation::default()),
            ]),
            failed_connections: 0,
        }));
        let cancel = CancellationToken::new();
        let task = tokio::spawn({
            let (control, cancel) = (control.clone(), cancel.clone());
            async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break; };
                            let (upstream, control, cancel) = (upstream.clone(), control.clone(), cancel.clone());
                            connections.spawn(async move {
                                tokio::select! {
                                    _ = cancel.cancelled() => {}
                                    result = serve(stream, &upstream, &control) => {
                                        if result.is_err() { control.lock().unwrap_or_else(std::sync::PoisonError::into_inner).failed_connections += 1; }
                                    }
                                }
                            });
                        }
                        _ = connections.join_next(), if !connections.is_empty() => {}
                    }
                }
                connections.abort_all();
                while connections.join_next().await.is_some() {}
            }
        });
        Ok(Self {
            _directory: directory,
            socket,
            control,
            cancel,
            task: Some(task),
        })
    }

    pub fn docker_host(&self) -> String {
        format!("unix://{}", self.socket.display())
    }

    pub fn allocation(&self, lease: &str) -> Allocation {
        self.control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .allocations
            .get(lease)
            .cloned()
            .unwrap_or_default()
    }

    pub fn failed_connections(&self) -> usize {
        self.control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .failed_connections
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task).await??;
        }
        Ok(())
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct Request {
    method: String,
    path: String,
    header: String,
    body: Vec<u8>,
    upgraded: bool,
}

async fn request(stream: &mut UnixStream) -> anyhow::Result<Request> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        ensure!(bytes.len() < 65536, "Docker proxy request header exceeded its bound");
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await?;
        ensure!(count > 0, "Docker proxy request closed before its header");
        bytes.extend_from_slice(&buffer[..count]);
    };
    let header = std::str::from_utf8(&bytes[..header_end])?.to_owned();
    let mut first = header
        .lines()
        .next()
        .context("Docker proxy request line absent")?
        .split_whitespace();
    let method = first.next().context("Docker proxy method absent")?.to_owned();
    let path = first
        .next()
        .context("Docker proxy path absent")?
        .split('?')
        .next()
        .unwrap_or_default()
        .to_owned();
    let mut length = 0;
    let mut upgraded = false;
    for line in header.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>()?;
            }
            if name.eq_ignore_ascii_case("upgrade") {
                upgraded = true;
            }
            ensure!(
                !name.eq_ignore_ascii_case("transfer-encoding"),
                "Docker proxy fixture requires bounded nonchunked requests"
            );
        }
    }
    ensure!(length <= MAX_HTTP_BYTES, "Docker proxy request body exceeded its bound");
    while bytes.len() < header_end + length {
        let mut buffer = [0; 4096];
        let count = stream.read(&mut buffer).await?;
        ensure!(count > 0, "Docker proxy request body closed early");
        bytes.extend_from_slice(&buffer[..count]);
    }
    ensure!(
        bytes.len() == header_end + length,
        "Docker proxy received pipelined request bytes"
    );
    Ok(Request {
        method,
        path,
        header,
        body: bytes[header_end..].to_vec(),
        upgraded,
    })
}

fn endpoint(path: &str) -> &str {
    let Some((version, rest)) = path.strip_prefix('/').and_then(|path| path.split_once('/')) else {
        return path;
    };
    if version
        .strip_prefix('v')
        .is_some_and(|version| !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.'))
    {
        &path[path.len() - rest.len() - 1..]
    } else {
        path
    }
}

fn response_json(bytes: &[u8]) -> anyhow::Result<(u16, Value)> {
    let end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .context("Docker proxy response header absent")?
        + 4;
    let header = std::str::from_utf8(&bytes[..end])?;
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("Docker proxy response status absent")?
        .parse()?;
    let mut body = bytes[end..].to_vec();
    if header.lines().any(|line| {
        line.split_once(':').is_some_and(|(key, value)| {
            key.eq_ignore_ascii_case("transfer-encoding") && value.trim().eq_ignore_ascii_case("chunked")
        })
    }) {
        let mut decoded = Vec::new();
        let mut rest = body.as_slice();
        loop {
            let line_end = rest
                .windows(2)
                .position(|window| window == b"\r\n")
                .context("Docker proxy chunk header absent")?;
            let length = usize::from_str_radix(
                std::str::from_utf8(&rest[..line_end])?
                    .split(';')
                    .next()
                    .unwrap_or_default(),
                16,
            )?;
            rest = &rest[line_end + 2..];
            if length == 0 {
                break;
            }
            ensure!(
                length <= rest.len().saturating_sub(2) && &rest[length..length + 2] == b"\r\n",
                "Docker proxy chunk body invalid"
            );
            decoded.extend_from_slice(&rest[..length]);
            rest = &rest[length + 2..];
        }
        body = decoded;
    }
    let value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    };
    Ok((status, value))
}

fn observe(request: &Request, bytes: &[u8], control: &mut Control) -> anyhow::Result<()> {
    let path = endpoint(&request.path);
    if !matches!(request.method.as_str(), "GET" | "POST") {
        return Ok(());
    }
    let interesting = path == "/containers/create"
        || path.ends_with("/exec")
        || path.ends_with("/start")
        || path.starts_with("/exec/") && path.ends_with("/json");
    if !interesting {
        return Ok(());
    }
    let (status, response) = response_json(bytes)?;
    if !(200..300).contains(&status) {
        return Ok(());
    }
    if request.method == "POST" && path == "/containers/create" {
        let body: Value = serde_json::from_slice(&request.body)?;
        if body
            .pointer("/Labels/sandbox.trogon.live~1owner")
            .and_then(Value::as_str)
            != Some(control.owner.as_str())
        {
            return Ok(());
        }
        let lease = body
            .pointer("/Labels/sandbox.trogon.live~1lease-id")
            .and_then(Value::as_str)
            .context("actual owned Docker create omitted lease label")?;
        let entry = control
            .allocations
            .get_mut(lease)
            .context("actual Docker create used an unregistered owned lease")?;
        let id = response
            .get("Id")
            .and_then(Value::as_str)
            .context("actual Docker create response omitted its canonical ID")?;
        ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "actual Docker create ID was not canonical"
        );
        entry.creates += 1;
        entry.container = Some(id.into());
        return Ok(());
    }
    for entry in control.allocations.values_mut() {
        let Some(id) = &entry.container else {
            continue;
        };
        if request.method == "POST" && path == format!("/containers/{id}/start") {
            entry.started = true;
        }
        if request.method == "POST" && path == format!("/containers/{id}/exec") {
            let body: Value = serde_json::from_slice(&request.body)?;
            if body
                .get("Cmd")
                .and_then(Value::as_array)
                .is_some_and(|command| command.get(4).and_then(Value::as_str) == Some("/workspace"))
            {
                entry.baseline_exec = Some(
                    response
                        .get("Id")
                        .and_then(Value::as_str)
                        .context("actual baseline exec response omitted identity")?
                        .into(),
                );
            }
        }
        if request.method == "GET"
            && entry
                .baseline_exec
                .as_ref()
                .is_some_and(|exec| path == format!("/exec/{exec}/json"))
        {
            ensure!(
                response.get("ID").and_then(Value::as_str) == entry.baseline_exec.as_deref(),
                "baseline exec inspection changed identity"
            );
            entry.baseline_complete = response.get("Running").and_then(Value::as_bool) == Some(false);
        }
    }
    Ok(())
}

async fn serve(mut downstream: UnixStream, upstream_path: &Path, control: &Arc<Mutex<Control>>) -> anyhow::Result<()> {
    let request = tokio::time::timeout(Duration::from_secs(10), request(&mut downstream)).await??;
    let inject = {
        let mut control = control.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let fault_lease = control.fault_lease.clone();
        let allocation = control
            .allocations
            .get_mut(&fault_lease)
            .context("fault allocation missing")?;
        if request.method == "GET"
            && allocation.started
            && allocation.baseline_complete
            && !allocation.fault_injected
            && allocation
                .container
                .as_ref()
                .is_some_and(|id| endpoint(&request.path) == format!("/containers/{id}/json"))
        {
            allocation.fault_injected = true;
            true
        } else {
            false
        }
    };
    if inject {
        let body = br#"{"message":"owned post-allocation inspect fault"}"#;
        downstream.write_all(format!("HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await?;
        downstream.write_all(body).await?;
        downstream.shutdown().await?;
        return Ok(());
    }
    let mut upstream = UnixStream::connect(upstream_path).await?;
    let mut header = request.header.clone();
    if !request.upgraded {
        header = request
            .header
            .lines()
            .filter(|line| {
                !line
                    .split_once(':')
                    .is_some_and(|(name, _)| name.eq_ignore_ascii_case("connection"))
            })
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\r\n");
        header.push_str("\r\nConnection: close\r\n\r\n");
    }
    upstream.write_all(header.as_bytes()).await?;
    upstream.write_all(&request.body).await?;
    if request.upgraded {
        tokio::time::timeout(
            Duration::from_secs(30),
            tokio::io::copy_bidirectional(&mut downstream, &mut upstream),
        )
        .await??;
    } else {
        let mut bytes = Vec::new();
        tokio::time::timeout(
            Duration::from_secs(20),
            (&mut upstream)
                .take((MAX_HTTP_BYTES + 1) as u64)
                .read_to_end(&mut bytes),
        )
        .await??;
        ensure!(
            bytes.len() <= MAX_HTTP_BYTES,
            "Docker proxy response exceeded its bound"
        );
        observe(
            &request,
            &bytes,
            &mut control.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        )?;
        downstream.write_all(&bytes).await?;
        downstream.shutdown().await?;
    }
    Ok(())
}

pub struct Worker(Option<Child>);

impl Worker {
    pub fn start(
        nats: &str,
        state: &Path,
        owner: &str,
        signer: &DispatchSigner,
        proxy: Option<&Proxy>,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(state)?;
        std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o700))?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-docker-worker"));
        command.env_clear();
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "DOCKER_HOST",
            "DOCKER_CONTEXT",
            "DOCKER_CONFIG",
            "DOCKER_CERT_PATH",
            "DOCKER_TLS_VERIFY",
            "XDG_RUNTIME_DIR",
            "SSL_CERT_FILE",
            "SSL_CERT_DIR",
        ] {
            if let Some(value) = trogon_std::env::ReadEnv::var_os(&trogon_std::env::SystemEnv, name) {
                command.env(name, value);
            }
        }
        if let Some(proxy) = proxy {
            command.env("DOCKER_HOST", proxy.docker_host());
        }
        let child = command
            .env("SANDBOX_NATS_URL", nats)
            .env("SANDBOX_DOCKER_STATE_DIR", state)
            .env("SANDBOX_DOCKER_WORKER_OWNER", owner)
            .env("SANDBOX_IMAGE", super::targeted_process_support::IMAGE)
            .env("SANDBOX_EGRESS_PROXY_IMAGE", super::targeted_process_support::IMAGE)
            .env("SANDBOX_WARM_POOL_ENABLED", "false")
            .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
            .env("SANDBOX_WORKER_AUTHORITY", "off")
            .env("SANDBOX_DISPATCH_VERIFY", "require")
            .env("SANDBOX_FENCE_EPOCH", "require")
            .env("SANDBOX_DISPATCH_FENCE_FILE", state.join("dispatch-fence.jsonl"))
            .env(
                "SANDBOX_DISPATCH_PUBLIC_KEYS",
                format!("{}:{}", signer.key_id(), signer.public_key_b64()),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("actual inspect-fault worker could not start")?;
        Ok(Self(Some(child)))
    }

    pub fn alive(&mut self) -> anyhow::Result<bool> {
        Ok(self.0.as_mut().context("owned worker stopped")?.try_wait()?.is_none())
    }

    pub fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(mut child) = self.0.take() {
            if child.try_wait()?.is_none() {
                child.kill()?;
            }
            child.wait()?;
        }
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
