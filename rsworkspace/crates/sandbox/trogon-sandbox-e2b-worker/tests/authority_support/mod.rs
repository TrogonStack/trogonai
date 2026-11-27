// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use trogon_sandbox_contract::{
    LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags,
    TenantId, WorkspaceSource,
};
use trogon_sandbox_e2b_worker::{E2bApi, E2bConfig, E2bExec, E2bWorkerProvider, Envd};
use url::Url;

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;

const MAX_REQUEST_BYTES: usize = 65_536;

#[derive(Default)]
pub struct Control {
    pub creates: AtomicUsize,
    pub connects: AtomicUsize,
    pub drop_pause_replies: AtomicUsize,
    pub reject_pause_replies: AtomicUsize,
    pub block_creates: AtomicBool,
    pub first_create: Notify,
    pub second_create: Notify,
    pub release_creates: Notify,
    live: Mutex<HashMap<String, bool>>,
}

impl Control {
    fn live(&self) -> MutexGuard<'_, HashMap<String, bool>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the sandbox is paused, or `None` when it does not exist.
    pub fn paused(&self, id: &str) -> Option<bool> {
        self.live().get(id).copied()
    }

    pub fn live_count(&self) -> usize {
        self.live().len()
    }

    /// Pauses a sandbox behind the worker's back.
    pub fn external_pause(&self, id: &str) {
        if let Some(paused) = self.live().get_mut(id) {
            *paused = true;
        }
    }
}

/// A loopback stand-in for the E2B control plane that tracks which sandboxes are live and paused.
pub struct Api {
    pub url: Url,
    pub control: Arc<Control>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for Api {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}

impl Api {
    pub async fn start() -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
        let control = Arc::new(Control::default());
        let state = control.clone();
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    Ok((socket, _)) = listener.accept() => {
                        connections.spawn(serve(socket, state.clone()));
                    }
                    Some(_) = connections.join_next() => {}
                }
            }
        });
        Ok(Self {
            url,
            control,
            server: Some(server),
        })
    }

    pub fn provider(&self) -> Result<Arc<E2bWorkerProvider>, Boxed> {
        let config = E2bConfig::new(self.url.clone(), "e2b.test", "k")?.with_envd_url(self.url.clone())?;
        Ok(Arc::new(E2bWorkerProvider::new(
            E2bApi::new(config, Duration::from_secs(5))?,
            "base",
            Arc::new(E2bExec::new(Envd::new(Duration::from_secs(3))?)),
        )))
    }

    pub async fn shutdown(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
        let size = socket.read(&mut chunk).await.ok().filter(|size| *size > 0)?;
        bytes.extend_from_slice(&chunk[..size]);
    };
    let header = std::str::from_utf8(&bytes[..end]).ok()?;
    let mut first = header.lines().next()?.split_whitespace();
    let (method, path) = (first.next()?.to_owned(), first.next()?.to_owned());
    let length = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .map_or(0, |(_, length)| {
            length.trim().parse::<usize>().unwrap_or(MAX_REQUEST_BYTES)
        });
    while bytes.len() < end + length.min(MAX_REQUEST_BYTES) {
        let size = socket.read(&mut chunk).await.ok().filter(|size| *size > 0)?;
        bytes.extend_from_slice(&chunk[..size]);
    }
    Some((method, path))
}

fn countdown(counter: &AtomicUsize) -> bool {
    counter
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| count.checked_sub(1))
        .is_ok()
}

async fn respond(socket: &mut tokio::net::TcpStream, status: u16, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

async fn serve(mut socket: tokio::net::TcpStream, control: Arc<Control>) {
    let Some((method, path)) = read_request(&mut socket).await else {
        return;
    };
    let (status, body) = if method == "POST" && path == "/v2/sandboxes" {
        let ordinal = control.creates.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("vm-{ordinal}");
        control.live().insert(id.clone(), false);
        control.first_create.notify_one();
        if ordinal > 1 {
            control.second_create.notify_one();
        }
        if control.block_creates.load(Ordering::SeqCst) {
            control.release_creates.notified().await;
        }
        (201, json!({"sandboxID": id, "templateID": "base"}).to_string())
    } else if method == "POST" && path.starts_with("/sandboxes/") && path.ends_with("/pause") {
        if countdown(&control.reject_pause_replies) {
            return respond(&mut socket, 403, &json!({"message": "pause is forbidden"}).to_string()).await;
        }
        let id = path.trim_start_matches("/sandboxes/").trim_end_matches("/pause");
        let present = control.live().get_mut(id).map(|paused| *paused = true).is_some();
        if present && countdown(&control.drop_pause_replies) {
            return;
        }
        (if present { 204 } else { 404 }, String::new())
    } else if method == "POST" && path.starts_with("/v2/sandboxes/") && path.ends_with("/connect") {
        let id = path.trim_start_matches("/v2/sandboxes/").trim_end_matches("/connect");
        let present = control.live().get_mut(id).map(|paused| *paused = false).is_some();
        if present {
            control.connects.fetch_add(1, Ordering::SeqCst);
        }
        (
            if present { 200 } else { 404 },
            json!({"sandboxID": id, "templateID": "base"}).to_string(),
        )
    } else if let Some(id) = path.strip_prefix("/sandboxes/") {
        match (method.as_str(), control.paused(id)) {
            ("GET", Some(paused)) => (
                200,
                json!({"sandboxID": id, "state": if paused { "paused" } else { "running" }}).to_string(),
            ),
            ("DELETE", _) => (
                if control.live().remove(id).is_some() { 204 } else { 404 },
                String::new(),
            ),
            _ => (404, String::new()),
        }
    } else {
        (404, String::new())
    };
    respond(&mut socket, status, &body).await;
}

pub fn lease_request() -> Result<LeaseRequest, Boxed> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified("base"),
            pull_secret: None,
        },
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
        tenant: TenantId::new("authority-fixture")?,
        provider_pref: vec![ProviderKind::E2b],
        spec,
        ttl_seconds: 60,
        idempotency_key: LeaseRequestKey::new("authority-fixture")?,
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
