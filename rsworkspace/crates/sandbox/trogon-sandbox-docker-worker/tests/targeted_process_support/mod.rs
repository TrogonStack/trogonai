// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::num::NonZeroU32;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::query_parameters::{ListContainersOptionsBuilder, RemoveContainerOptionsBuilder};
use serde::Deserialize;
use serde_json::Value;
use trogon_sandbox_contract::{
    BootMode, DispatchTarget, LeaseId, LeaseRequest, LeaseRequestKey, NetworkPolicy, OciImageRef,
    ProviderDispatchPayload, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, TenantId,
    WorkspaceSource,
};
use trogon_sandbox_protocol::DispatchSigner;

pub const IMAGE: &str = "docker.io/library/node:22-bookworm-slim";
const OWNER_LABEL: &str = "sandbox.trogon.live/owner";
const LEASE_LABEL: &str = "sandbox.trogon.live/lease-id";

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

/// The lease request every targeted fixture sends: bare boot, `/tmp`, deny-all network, persistent.
pub fn lease_request(
    tenant: &str,
    lease: &LeaseId,
    image: SandboxImage,
    vcpu_millis: Option<u32>,
) -> anyhow::Result<LeaseRequest> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image,
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::MIN,
        memory_mib: NonZeroU32::new(256).context("memory")?,
        snapshot_out: None,
        network_policy: Some(NetworkPolicy::DenyAll),
        mounts: vec![],
        drives: vec![],
        exposed_ports: vec![],
        boot_mode: BootMode::Bare,
        working_dir: Some("/tmp".into()),
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis,
        disk_mib: None,
        retention: Default::default(),
    })?;
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new(tenant)?,
        provider_pref: vec![ProviderKind::LocalDocker],
        spec,
        ttl_seconds: 300,
        idempotency_key: LeaseRequestKey::new(lease.as_str())?,
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

/// The registry image reference of the fixture image.
pub fn registry_image() -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(IMAGE),
        pull_secret: None,
    }
}

/// A dispatch payload for `lease`, optionally addressed to `target`.
pub fn dispatch_payload(
    tenant: &str,
    lease: &LeaseId,
    image: SandboxImage,
    vcpu_millis: Option<u32>,
    target: Option<DispatchTarget>,
) -> anyhow::Result<ProviderDispatchPayload> {
    Ok(ProviderDispatchPayload {
        lease_id: lease.clone(),
        request: lease_request(tenant, lease, image, vcpu_millis)?,
        correlation_id: None,
        resolved_drives: vec![],
        target,
    })
}

pub struct Process(Option<Child>);

impl Process {
    pub fn is_alive(&mut self) -> anyhow::Result<bool> {
        Ok(self.0.as_mut().context("owned process stopped")?.try_wait()?.is_none())
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

    pub async fn stop_gracefully(&mut self) -> anyhow::Result<()> {
        let Some(child) = self.0.as_mut() else {
            return Ok(());
        };
        if child.try_wait()?.is_none() {
            ensure!(
                Command::new("kill")
                    .args(["-INT", &child.id().to_string()])
                    .status()?
                    .success(),
                "could not interrupt the exact owned worker"
            );
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if child.try_wait()?.is_some() {
                        return Ok::<_, anyhow::Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .context("exact owned worker did not stop gracefully")??;
        }
        self.0.take();
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub struct Nats {
    _process: Process,
    _directory: tempfile::TempDir,
    pub url: String,
    monitor: String,
    pub client: async_nats::Client,
}

#[derive(Deserialize)]
pub struct Subscription {
    pub subject: String,
    pub cid: u64,
    #[serde(default)]
    pub qgroup: Option<String>,
}

impl Nats {
    pub async fn start() -> anyhow::Result<Self> {
        let directory = tempfile::tempdir()?;
        let child = Command::new("nats-server")
            .args(["-c", "/dev/stdin"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("the actual-worker fixture requires nats-server")?;
        let mut process = Process(Some(child));
        let child = process.0.as_mut().context("owned NATS process missing")?;
        let ports = directory.path().join(format!("nats-server_{}.ports", child.id()));
        writeln!(
            child.stdin.take().context("owned NATS stdin missing")?,
            "host: 127.0.0.1\nport: -1\nhttp: \"127.0.0.1:-1\"\nports_file_dir: {}\nmax_payload: 65536\njetstream {{ store_dir: {} }}",
            serde_json::to_string(&directory.path())?,
            serde_json::to_string(&directory.path().join("jetstream"))?
        )?;
        let (url, monitor, client) = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                ensure!(process.is_alive()?, "owned NATS exited during startup");
                if let Ok(bytes) = std::fs::read(&ports)
                    && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                    && let (Some(url), Some(monitor)) = (value["nats"][0].as_str(), value["monitoring"][0].as_str())
                    && let Ok(client) = async_nats::connect(url.to_owned()).await
                {
                    return Ok::<_, anyhow::Error>((url.to_owned(), monitor.to_owned(), client));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .context("owned NATS startup deadline expired")??;
        async_nats::jetstream::new(client.clone()).query_account().await?;
        Ok(Self {
            _process: process,
            _directory: directory,
            url,
            monitor,
            client,
        })
    }

    pub async fn subscriptions(&self, subject: &str) -> anyhow::Result<Vec<Subscription>> {
        #[derive(Deserialize)]
        struct Subscriptions {
            #[serde(default)]
            subscriptions_list: Vec<Subscription>,
        }
        let bytes = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()?
            .get(format!("{}/subsz", self.monitor.trim_end_matches('/')))
            .query(&[("subs", "true"), ("test", subject)])
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        Ok(serde_json::from_slice::<Subscriptions>(&bytes)?.subscriptions_list)
    }
}

pub fn signer() -> anyhow::Result<DispatchSigner> {
    let mut bytes = [0; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(DispatchSigner::new("docker-process-intake", bytes))
}

pub fn worker(nats: &Nats, state: &Path, owner: &str, signer: &DispatchSigner) -> anyhow::Result<Process> {
    worker_with_supervisor(nats, state, owner, signer, None)
}

pub fn worker_with_supervisor(
    nats: &Nats,
    state: &Path,
    owner: &str,
    signer: &DispatchSigner,
    supervisor: Option<&Path>,
) -> anyhow::Result<Process> {
    configured_worker(nats, state, owner, signer, supervisor, None)
}

pub fn worker_with_registry(
    nats: &Nats,
    state: &Path,
    owner: &str,
    signer: &DispatchSigner,
    registry: &str,
) -> anyhow::Result<Process> {
    configured_worker(nats, state, owner, signer, None, Some(registry))
}

fn configured_worker(
    nats: &Nats,
    state: &Path,
    owner: &str,
    signer: &DispatchSigner,
    supervisor: Option<&Path>,
    registry: Option<&str>,
) -> anyhow::Result<Process> {
    std::fs::create_dir(state)?;
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
    if let Some(supervisor) = supervisor {
        command.env("SANDBOX_DOCKER_COMMAND_SUPERVISOR", supervisor);
    }
    if let Some(registry) = registry {
        command.env("SANDBOX_SNAPSHOT_REGISTRY", registry);
        command.env("SANDBOX_OCI_INSECURE_REGISTRIES", registry);
    }
    let child = command
        .env("SANDBOX_NATS_URL", &nats.url)
        .env("SANDBOX_DOCKER_WORKER_OWNER", owner)
        .env("SANDBOX_DOCKER_STATE_DIR", state)
        .env("SANDBOX_IMAGE", IMAGE)
        .env("SANDBOX_EGRESS_PROXY_IMAGE", IMAGE)
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
        .context("failed to start the actual Docker worker binary")?;
    Ok(Process(Some(child)))
}

pub struct Native {
    docker: bollard::Docker,
    owner: String,
    leases: Vec<String>,
    ids: Vec<String>,
}

impl Native {
    pub async fn new(owner: String) -> anyhow::Result<Self> {
        let docker = bollard::Docker::connect_with_defaults()?;
        tokio::time::timeout(Duration::from_secs(5), docker.ping()).await??;
        tokio::time::timeout(Duration::from_secs(5), docker.inspect_image(IMAGE)).await??;
        Ok(Self {
            docker,
            owner,
            leases: vec![],
            ids: vec![],
        })
    }

    pub fn register_lease(&mut self, lease: &str) {
        self.leases.push(lease.to_owned());
    }

    pub fn register_id(&mut self, id: &str) {
        if id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            eprintln!("targeted intake allocation ownership: owned_container_id={id}");
        }
        self.ids.push(id.to_owned());
    }

    pub async fn owned(&self) -> anyhow::Result<Vec<String>> {
        let filters = HashMap::from([("label".to_owned(), vec![format!("{OWNER_LABEL}={}", self.owner)])]);
        let containers = tokio::time::timeout(
            Duration::from_secs(5),
            self.docker.list_containers(Some(
                ListContainersOptionsBuilder::new().all(true).filters(&filters).build(),
            )),
        )
        .await??;
        containers
            .into_iter()
            .map(|container| container.id.context("owned container has no canonical ID"))
            .collect()
    }

    pub async fn running(&self, id: &str, lease: &str) -> anyhow::Result<()> {
        ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "grant must carry a canonical actual Docker ID"
        );
        let inspected = tokio::time::timeout(Duration::from_secs(5), self.docker.inspect_container(id, None)).await??;
        ensure!(
            inspected.id.as_deref() == Some(id)
                && inspected.state.as_ref().and_then(|state| state.running) == Some(true)
                && inspected
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(|labels| labels.get(OWNER_LABEL) == Some(&self.owner)
                        && labels.get(LEASE_LABEL).is_some_and(|value| value == lease)),
            "grant is not its exact running owned native lease"
        );
        Ok(())
    }

    pub async fn absent(&self, id: &str) -> anyhow::Result<bool> {
        match tokio::time::timeout(Duration::from_secs(5), self.docker.inspect_container(id, None)).await? {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(true),
            Ok(_) => Ok(false),
            Err(_) => anyhow::bail!("native absence inspection failed without an exact 404"),
        }
    }

    pub async fn wait_absent(&self, id: &str) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if self.absent(id).await? {
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .context("product cleanup did not confirm exact native absence")??;
        Ok(())
    }

    pub async fn finish(&mut self) -> anyhow::Result<bool> {
        let remaining = self.owned().await?;
        let fallback = !remaining.is_empty();
        for id in remaining {
            let inspected =
                tokio::time::timeout(Duration::from_secs(5), self.docker.inspect_container(&id, None)).await??;
            ensure!(
                inspected.id.as_deref() == Some(id.as_str())
                    && inspected
                        .config
                        .as_ref()
                        .and_then(|config| config.labels.as_ref())
                        .is_some_and(|labels| labels.get(OWNER_LABEL) == Some(&self.owner)
                            && labels.get(LEASE_LABEL).is_some_and(|lease| self.leases.contains(lease))),
                "fixture reconciliation refuses changed or unregistered native ownership"
            );
            eprintln!("targeted intake reconciliation ownership: owned_container_id={id}");
            self.ids.push(id.clone());
            tokio::time::timeout(
                Duration::from_secs(10),
                self.docker.remove_container(
                    &id,
                    Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
                ),
            )
            .await??;
        }
        self.ids.sort();
        self.ids.dedup();
        for id in &self.ids {
            ensure!(
                self.absent(id).await?,
                "exact owned native container absence remains unconfirmed"
            );
        }
        ensure!(
            self.owned().await?.is_empty(),
            "owned native assets remain after cleanup"
        );
        eprintln!(
            "targeted intake cleanup: exact_containers_absent=true owned_assets_empty=true fallback_used={fallback}"
        );
        Ok(fallback)
    }
}
