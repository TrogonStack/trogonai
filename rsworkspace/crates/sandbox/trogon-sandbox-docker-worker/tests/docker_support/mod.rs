//! Shared by the Docker-backed test binaries; each one uses a different subset.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::BTreeMap;
use std::sync::Arc;
use trogon_sandbox_contract::PollCommandResponse;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    BootMode, LeaseId, LeaseRequest, LeaseRequestKey, OciImageRef, Port, ProviderKind, SandboxImage, SandboxSpec,
    SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};
use trogon_sandbox_docker_worker::adapter::{self, DockerWorkerProvider};
use trogon_sandbox_docker_worker::digest_cache::DigestCache;
use trogon_sandbox_providers::LocalDockerProvider;
use trogon_sandbox_volumes::VolumeRegion;

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const ALPINE_IMAGE: &str = "alpine:3.20";

/// A fresh identifier in the ULID shape.
pub fn fresh_id() -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bits = uuid::Uuid::now_v7().as_u128();
    let mut text = [b'0'; 26];
    for slot in text.iter_mut().rev() {
        *slot = ALPHABET[(bits & 0x1f) as usize];
        bits >>= 5;
    }
    text.iter().map(|byte| char::from(*byte)).collect()
}

pub fn fresh_lease_id() -> LeaseId {
    LeaseId::new(fresh_id())
}

pub fn free_port() -> TestResult<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// A lease request on the plain alpine image with the given
/// boot mode.
pub fn lease_request(tenant: &str, boot_mode: BootMode) -> TestResult<LeaseRequest> {
    lease_request_on_image(tenant, boot_mode, ALPINE_IMAGE)
}

/// Like [`lease_request`], on the given image reference.
pub fn lease_request_on_image(tenant: &str, boot_mode: BootMode, image: &str) -> TestResult<LeaseRequest> {
    lease_request_full(tenant, boot_mode, image, &[])
}

/// Like [`lease_request`], publishing the given container ports at create time.
pub fn lease_request_exposing(tenant: &str, boot_mode: BootMode, ports: &[u16]) -> TestResult<LeaseRequest> {
    lease_request_full(tenant, boot_mode, ALPINE_IMAGE, ports)
}

fn lease_request_full(tenant: &str, boot_mode: BootMode, image: &str, ports: &[u16]) -> TestResult<LeaseRequest> {
    let exposed_ports = ports
        .iter()
        .map(|port| Port::new(*port))
        .collect::<Result<Vec<_>, _>>()?;
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(image),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: std::num::NonZeroU32::MIN,
        memory_mib: std::num::NonZeroU32::new(256).ok_or("memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: vec![],
        drives: vec![],
        exposed_ports,
        boot_mode,
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
        tenant: TenantId::new(tenant)?,
        provider_pref: vec![ProviderKind::LocalDocker],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new(fresh_id())?,
        isolation_tier: None,
        host_imports: vec![],
        required_capabilities: None,
        prewarm: Default::default(),
        tags: SandboxTags::default(),
        name: None,
        persistent: Default::default(),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

/// A provider wired to a NATS container and the local Docker daemon. The server lives as long as the rig.
pub struct Rig {
    pub provider: Arc<DockerWorkerProvider>,
    pub server: JetStreamTestServer,
}

impl std::ops::Deref for Rig {
    type Target = DockerWorkerProvider;

    fn deref(&self) -> &DockerWorkerProvider {
        &self.provider
    }
}

/// Starts NATS one test at a time so concurrent container starts do not race the Docker port publisher.
async fn start_nats() -> JetStreamTestServer {
    static STARTING: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = STARTING.get_or_init(|| tokio::sync::Mutex::new(())).lock().await;
    JetStreamTestServer::start().await
}

pub async fn build_provider(worker: &str) -> TestResult<Rig> {
    let server = start_nats().await;
    let nats_url = server.address().to_string();
    let inner = LocalDockerProvider::with_default_engine("sandbox-egress-proxyd:dev".to_string())?
        .with_owner(format!("command-test-{}", fresh_id()));
    let provider = DockerWorkerProvider::new(
        inner,
        ALPINE_IMAGE.to_string(),
        server.jetstream().await,
        nats_url,
        format!("{worker}-worker"),
        adapter::build_oci_client(vec![]),
        "docker.io".to_string(),
        None,
        Arc::new(DigestCache::new(8)),
        VolumeRegion::new("us-east-1").ok(),
        None,
        adapter::default_warm_pool_config(),
    )?;
    Ok(Rig {
        provider: Arc::new(provider),
        server,
    })
}

/// Polls a detached execution until it completes, failing on any other reply or when the deadline passes.
pub async fn wait_completed(
    provider: &DockerWorkerProvider,
    lease_id: &LeaseId,
    execution_id: &trogon_sandbox_contract::ExecutionId,
    seconds: u64,
) -> TestResult<trogon_sandbox_contract::CommandOutput> {
    let wait = async {
        loop {
            match provider.poll_command_for_lease(lease_id, execution_id).await {
                Some(PollCommandResponse::Completed(output)) => return Ok(output),
                Some(PollCommandResponse::Running) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                other => return Err(format!("unexpected poll response while waiting: {other:?}")),
            }
        }
    };
    Ok(tokio::time::timeout(std::time::Duration::from_secs(seconds), wait).await??)
}
