//! A scripted SPACE and the builders the tests share.

#![allow(dead_code)]

mod fake;

use std::error::Error;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;
use trogon_sandbox_contract::{LeaseId, ProviderDispatchPayload, RunCommandRequest};
use trogon_sandbox_space_worker::{
    MaxLeases, SpaceApi, SpaceConfig, SpaceConfigParts, SpaceOrigin, SpaceProvider, SpaceTemplate, SpaceToken,
};
use url::Url;

pub use fake::*;

pub type TestResult = Result<(), Box<dyn Error>>;

pub fn origin(uri: &str) -> Result<SpaceOrigin, Box<dyn Error>> {
    Ok(SpaceOrigin::parse(Url::parse(uri)?)?)
}

pub fn config(dir: &Path, max_leases: u32, file_read_timeout: Duration) -> Result<SpaceConfig, Box<dyn Error>> {
    Ok(SpaceConfig::new(SpaceConfigParts {
        template: SpaceTemplate::parse("seed:latest")?,
        state_dir: dir.to_path_buf(),
        boot_timeout: Duration::from_millis(400),
        operation_timeout: Duration::from_millis(400),
        poll_interval: Duration::from_millis(10),
        max_leases: MaxLeases::parse(max_leases)?,
        command_grace: Duration::from_secs(1),
        file_read_timeout,
    })?)
}

pub fn api(control: &str, data: &str) -> Result<SpaceApi, Box<dyn Error>> {
    Ok(
        SpaceApi::new(origin(control)?, origin(data)?, SpaceToken::parse(MANAGEMENT_TOKEN)?)?
            .with_retry_base(Duration::from_millis(5)),
    )
}

/// A provider over the scripted SPACE.
pub struct Rig {
    pub space: Space,
    pub provider: Arc<SpaceProvider>,
    pub dir: TempDir,
}

impl Rig {
    pub async fn start() -> Result<Self, Box<dyn Error>> {
        Self::with(4).await
    }

    pub async fn with(max_leases: u32) -> Result<Self, Box<dyn Error>> {
        Self::build(max_leases, None, Duration::from_secs(5)).await
    }

    /// A provider whose data plane is somewhere other than the control plane.
    pub async fn split(data: &str) -> Result<Self, Box<dyn Error>> {
        Self::build(4, Some(data), Duration::from_secs(5)).await
    }

    /// A provider that gives up on a file read after `limit`.
    pub async fn reading_for(limit: Duration) -> Result<Self, Box<dyn Error>> {
        Self::build(4, None, limit).await
    }

    async fn build(max_leases: u32, data: Option<&str>, read_limit: Duration) -> Result<Self, Box<dyn Error>> {
        let space = Space::start().await;
        let dir = tempfile::tempdir()?;
        let uri = space.uri();
        let provider = Arc::new(SpaceProvider::new(
            api(&uri, data.unwrap_or(&uri))?,
            config(dir.path(), max_leases, read_limit)?,
        )?);
        Ok(Self { space, provider, dir })
    }

    /// Leases through an untargeted dispatch.
    pub async fn lease(&self, lease: &str) -> Result<LeaseId, Box<dyn Error>> {
        let payload = dispatch(lease, None)?;
        self.provider.lease_dispatch(&payload).await?;
        Ok(payload.lease_id)
    }
}

pub fn lease_request(key: &str) -> Value {
    json!({
        "tenant": "acme",
        "provider_pref": ["space"],
        "spec": {
            "image": {"kind": "registry", "reference": {"registry": "", "repository": "seed", "tag": "latest"}},
            "workspace": {"empty": {}},
            "cpu_seconds": 10,
            "memory_mib": 512,
        },
        "ttl_seconds": 600,
        "idempotency_key": key,
        "persistent": false,
    })
}

pub fn dispatch_json(lease: &str, target: Option<(&str, &str)>) -> Value {
    let mut payload = json!({"lease_id": lease, "request": lease_request(lease)});
    if let Some((worker, attempt)) = target {
        payload["target"] = json!({"worker_id": worker, "attempt_id": attempt});
    }
    payload
}

pub fn dispatch(lease: &str, target: Option<(&str, &str)>) -> Result<ProviderDispatchPayload, Box<dyn Error>> {
    Ok(serde_json::from_value(dispatch_json(lease, target))?)
}

/// An address nothing listens on.
pub async fn closed_origin() -> Result<String, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    Ok(format!("http://{address}"))
}

/// A data plane that opens every event stream, sends `prefix` and then goes quiet.
pub async fn hanging_events(prefix: &'static str) -> Result<String, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut seen = Vec::new();
                let mut buffer = [0u8; 1024];
                while !seen.windows(4).any(|window| window == b"\r\n\r\n") {
                    match tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => seen.extend_from_slice(&buffer[..read]),
                    }
                }
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{prefix}\r\n",
                    prefix.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, head.as_bytes()).await;
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            });
        }
    });
    Ok(format!("http://{address}"))
}

pub fn run_request(
    lease: &LeaseId,
    command: &[&str],
    timeout_seconds: u32,
) -> Result<RunCommandRequest, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "lease_id": lease,
        "command": command,
        "timeout_seconds": timeout_seconds,
    }))?)
}

/// The identity of a dispatch attempt run by the rig's worker.
pub fn identity(
    rig: &Rig,
    lease: &str,
    attempt: &str,
) -> Result<trogon_sandbox_contract::LeaseAttemptIdentity, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "lease_id": lease,
        "provider": "space",
        "target": {"worker_id": rig.worker(), "attempt_id": attempt},
    }))?)
}

pub fn cleanup_request(
    rig: &Rig,
    lease: &str,
    attempt: &str,
    intent: &str,
) -> Result<trogon_sandbox_contract::ProviderCleanupRequest, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "identity": identity(rig, lease, attempt)?,
        "intent": intent,
    }))?)
}

impl Rig {
    /// The worker identifier the dispatch ledger holds.
    pub fn worker(&self) -> String {
        self.provider.dispatch_ledger().worker_id().as_str().to_owned()
    }

    /// Leases through a dispatch that names this worker and `attempt`.
    pub async fn lease_targeted(&self, lease: &str, attempt: &str) -> Result<(), Box<dyn Error>> {
        let worker = self.worker();
        let payload = dispatch(lease, Some((&worker, attempt)))?;
        self.provider.lease_dispatch(&payload).await?;
        Ok(())
    }

    /// Registers a sandbox this worker owns, as if an earlier run had made it.
    pub fn plant(&self, id: &str, lease: &str, attempt: &str) {
        self.space.state().sandboxes.insert(
            id.to_owned(),
            Sandbox {
                state: "running".into(),
                metadata: json!({
                    "sandbox_owner": self.worker(),
                    "sandbox_lease": lease,
                    "sandbox_dispatch_attempt": attempt,
                    "sandbox_attempt": "unrelated",
                }),
                failure_code: None,
            },
        );
    }
}
