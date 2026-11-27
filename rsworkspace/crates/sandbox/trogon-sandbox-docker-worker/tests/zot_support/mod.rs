//! A throwaway OCI registry container plus helpers shared by the snapshot tests.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    BootMode, LeaseId, LeaseRequest, LeaseRequestKey, OciArtifactRef, ProviderKind, SandboxImage, SandboxSpec,
    SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};
use trogon_sandbox_docker_worker::adapter::{self, DockerWorkerProvider};
use trogon_sandbox_docker_worker::digest_cache::DigestCache;
use trogon_sandbox_docker_worker::snapshot::{gzip_tar, push_rootfs_artifact};
use trogon_sandbox_docker_worker::warm_pool::WarmPoolConfig;
use trogon_sandbox_oci::{OciClient, OciClientConfig, RegistryHost};
use trogon_sandbox_providers::{BollardEngine, ContainerRunSpec, DockerEngine, LocalDockerProvider};
use trogon_sandbox_volumes::VolumeRegion;

use crate::docker_support::{ALPINE_IMAGE, Rig, TestResult, free_port, fresh_id};

pub const ZOT_IMAGE: &str = "ghcr.io/project-zot/zot-linux-arm64:latest";

/// A registry container that is removed when dropped, even if the test fails.
pub struct Zot {
    container: String,
    pub registry: String,
}

impl Zot {
    pub async fn start() -> TestResult<Self> {
        let port = free_port()?;
        let output = tokio::process::Command::new("docker")
            .args(["run", "-d", "--rm", "-p", &format!("{port}:5000"), ZOT_IMAGE])
            .output()
            .await?;
        if !output.status.success() {
            return Err(format!("docker run failed: {}", String::from_utf8_lossy(&output.stderr)).into());
        }
        let zot = Self {
            container: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            registry: format!("127.0.0.1:{port}"),
        };
        zot.wait_ready(port).await?;
        Ok(zot)
    }

    async fn wait_ready(&self, port: u16) -> TestResult {
        let client = reqwest::Client::new();
        for _ in 0..480 {
            if let Ok(response) = client.get(format!("http://127.0.0.1:{port}/v2/")).send().await
                && (response.status().is_success() || response.status().as_u16() == 401)
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Err(format!("zot did not become ready on port {port}").into())
    }

    pub fn host(&self) -> TestResult<RegistryHost> {
        Ok(RegistryHost::new(self.registry.clone())?)
    }

    pub fn oci_client(&self) -> TestResult<OciClient> {
        Ok(OciClient::new(OciClientConfig {
            insecure_registries: vec![self.host()?],
            ..Default::default()
        }))
    }
}

impl Drop for Zot {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container])
            .output();
    }
}

pub fn sleeper_spec(name: &str, env: Vec<String>) -> ContainerRunSpec {
    ContainerRunSpec {
        name: format!("{name}-{}", fresh_id()),
        image: ALPINE_IMAGE.to_string(),
        memory_mib: 256,
        cpu: 1.0,
        disk_mib: None,
        env,
        binds: vec![],
        network_mode: "none".to_string(),
        entrypoint_override: None,
        cmd_override: Some(vec!["sleep".to_string(), "600".to_string()]),
        working_dir: None,
        user: None,
        cap_add: vec![],
        cap_drop: vec![],
        exposed_ports: vec![],
        hardening: Default::default(),
        labels: Default::default(),
    }
}

/// Exports a running alpine container's rootfs and pushes it as a snapshot artifact.
pub async fn push_test_snapshot(zot: &Zot, repository: &str) -> TestResult<OciArtifactRef> {
    let engine = Arc::new(BollardEngine::connect()?);
    let provider = LocalDockerProvider::new(
        Arc::clone(&engine) as Arc<dyn DockerEngine>,
        "sandbox-egress-proxyd:dev".to_string(),
    );
    let container_id = engine
        .create_and_start(sleeper_spec("snapshot-src", vec![]))
        .await?
        .container_id;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let exported = provider.commit_and_export_rootfs(&container_id).await;
    let _ = engine.kill(&container_id).await;
    let gz_tar = gzip_tar(&exported?)?;
    let (artifact_ref, _bytes) = push_rootfs_artifact(&zot.oci_client()?, &zot.registry, repository, gz_tar).await?;
    Ok(artifact_ref)
}

pub fn snapshot_lease_request(artifact_ref: &OciArtifactRef, tenant: &str) -> TestResult<LeaseRequest> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Snapshot {
            reference: artifact_ref.clone(),
            parent: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: std::num::NonZeroU32::MIN,
        memory_mib: std::num::NonZeroU32::new(256).ok_or("memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: vec![],
        drives: vec![],
        exposed_ports: vec![],
        boot_mode: BootMode::Bare,
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

pub async fn build_registry_provider(zot: &Zot, warm_pool_config: WarmPoolConfig) -> TestResult<Rig> {
    let server = JetStreamTestServer::start().await;
    let nats_url = server.address().to_string();
    let inner = LocalDockerProvider::with_default_engine("sandbox-egress-proxyd:dev".to_string())?
        .with_owner(format!("zot-test-{}", fresh_id()));
    let provider = DockerWorkerProvider::new(
        inner,
        ALPINE_IMAGE.to_string(),
        server.jetstream().await,
        nats_url,
        format!("zot-test-{}-worker", fresh_id()),
        adapter::build_oci_client(vec![zot.host()?]),
        zot.registry.clone(),
        None,
        Arc::new(DigestCache::new(32)),
        VolumeRegion::new("us-east-1").ok(),
        None,
        warm_pool_config,
    )?;
    Ok(Rig {
        provider: Arc::new(provider),
        server,
    })
}

pub fn fresh_lease() -> LeaseId {
    crate::docker_support::fresh_lease_id()
}
