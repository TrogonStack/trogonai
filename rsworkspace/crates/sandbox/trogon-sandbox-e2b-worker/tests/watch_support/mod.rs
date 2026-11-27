// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_nats::Client;
use axum::Router;
use bytes::Bytes;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    FileOp, FileOpRequest, FileOpResponse, LeaseId, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind,
    SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, SandboxUserName, TenantId, WorkerId, WorkspaceSource,
};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals};
use trogon_sandbox_e2b_worker::{E2bApi, E2bConfig, E2bExec, E2bWorkerProvider, Envd};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, nats_transfer, subjects};
use url::Url;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
pub type Boxed = Box<dyn std::error::Error>;

/// How long any single wait in these tests may take.
pub const WAIT: Duration = Duration::from_secs(10);

/// Locks a fixture's shared state even when another task panicked while holding it.
pub fn locked<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A request for a persistent registry-image lease, or for another image when one is given.
pub fn lease_request(key: &str, image: Option<SandboxImage>) -> Result<LeaseRequest, Boxed> {
    let image = image.unwrap_or(SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified("base"),
        pull_secret: None,
    });
    let spec = SandboxSpec::new(SandboxSpecParts {
        image,
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
        tenant: TenantId::new("watch-fixture")?,
        provider_pref: vec![ProviderKind::E2b],
        spec,
        ttl_seconds: 600,
        idempotency_key: LeaseRequestKey::new(key)?,
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

/// Serves `router` on a loopback port; the returned task ends the server when aborted.
pub async fn serve(router: Router) -> Result<(Url, tokio::task::JoinHandle<()>), Boxed> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((url, server))
}

/// A provider that talks to `url` for both the control plane and every sandbox daemon.
pub fn provider(url: &Url, exec: E2bExec) -> Result<Arc<E2bWorkerProvider>, Boxed> {
    let config = E2bConfig::new(url.clone(), "e2b.test", "api-key")?.with_envd_url(url.clone())?;
    Ok(Arc::new(E2bWorkerProvider::new(
        E2bApi::new(config, Duration::from_secs(5))?,
        "base",
        Arc::new(exec),
    )))
}

/// The terminals the worker needs to serve a provider.
pub fn terminals(provider: &Arc<E2bWorkerProvider>, envd: Envd, client: &Client) -> Arc<E2bTerminals> {
    Arc::new(E2bTerminals::new(
        Arc::new(E2bGuestShell::new(Arc::clone(provider), envd)),
        client.clone(),
    ))
}

/// A NATS server with JetStream and the worker serving `provider` on it.
pub struct Served {
    _nats: JetStreamTestServer,
    pub client: Client,
    stop: tokio_util::sync::CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}

impl Served {
    /// Starts the worker and waits until it answers per-lease file operations.
    pub async fn start(provider: Arc<E2bWorkerProvider>, envd: Envd, probe_lease: &str) -> Result<Self, Boxed> {
        let nats = JetStreamTestServer::start().await;
        let client = nats.client().await;
        let stop = tokio_util::sync::CancellationToken::new();
        let worker_id = WorkerId::new("worker-watch")?;
        let terminals = terminals(&provider, envd, &client);
        let task = tokio::spawn({
            let (client, stop) = (client.clone(), stop.clone());
            async move {
                let _ = trogon_sandbox_e2b_worker::run(
                    client.clone(),
                    async_nats::jetstream::new(client),
                    provider,
                    &worker_id,
                    stop,
                    terminals,
                    AuthorityConfig::new(AuthorityMode::Off),
                )
                .await;
            }
        });
        let served = Self {
            _nats: nats,
            client,
            stop,
            task,
        };
        tokio::time::timeout(WAIT, async {
            while served
                .raw_file_op(probe_lease, &FileOp::Stat { path: "/".into() }, None)
                .await
                .is_err()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(served)
    }

    async fn raw_file_op(&self, lease: &str, op: &FileOp, as_user: Option<&str>) -> Result<Bytes, Boxed> {
        let request = FileOpRequest {
            lease_id: LeaseId::new(lease),
            op: op.clone(),
            as_user: as_user.map(SandboxUserName::new).transpose()?,
        };
        let subject = subjects::provider_file_op(ProviderKind::E2b, &request.lease_id);
        Ok(nats_transfer::request(&self.client, &subject, serde_json::to_vec(&request)?.into(), None, WAIT).await?)
    }

    /// Sends one file operation to the lease over NATS.
    pub async fn file_op(&self, lease: &str, op: FileOp) -> Result<FileOpResponse, Boxed> {
        self.file_op_as(lease, op, None).await
    }

    /// Sends one file operation that runs as `user`.
    pub async fn file_op_as(&self, lease: &str, op: FileOp, user: Option<&str>) -> Result<FileOpResponse, Boxed> {
        Ok(serde_json::from_slice(&self.raw_file_op(lease, &op, user).await?)?)
    }
}

/// Reads one typed shape out of a response without matching on the variant.
pub fn shape<T: serde::de::DeserializeOwned>(response: &FileOpResponse) -> Result<T, Boxed> {
    Ok(serde_json::from_value(serde_json::to_value(response)?)?)
}

/// The fields of a watch poll that returned events.
#[derive(serde::Deserialize)]
pub struct PolledEvents {
    pub events: Vec<trogon_sandbox_contract::FileWatchEvent>,
    pub next_seq: u64,
    pub overflowed: trogon_sandbox_contract::FileWatchOverflow,
}

/// The id of a started watch.
#[derive(serde::Deserialize)]
pub struct StartedWatch {
    pub watch_id: String,
}
