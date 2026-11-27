// Shared by the egress probe test binary; each lane uses only part of it.
#![allow(dead_code, reason = "shared test support; every lane uses a different subset")]

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::num::NonZeroU32;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use async_nats::Client;
use bollard::Docker;
use bollard::query_parameters::{EventsOptionsBuilder, ListContainersOptionsBuilder, RemoveContainerOptionsBuilder};
use futures::StreamExt;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_conformance::{ConformanceGrant, NativeCleanupAuthority};
use trogon_sandbox_contract::{
    BootMode, CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, LeaseRequest,
    LeaseRequestKey, NetworkPolicy, OciImageRef, ProviderCleanupRequest, ProviderCleanupResponse,
    ProviderDispatchPayload, ProviderDispatchReply, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecParts,
    SandboxTags, TenantId, WorkerDispatchProtocol, WorkerHeartbeat, WorkspaceSource,
};
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope, subjects};
use trogon_sandbox_providers::{DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER, DOCKER_LABEL_ROLE};

pub const WAIT: Duration = Duration::from_secs(15);
/// The tenant the conformance suite leases under; the control lease shares it so one cleanup path serves both.
pub const TENANT: &str = "conformance";
const OUTPUT_BOUND: usize = 65536;

/// Locks `mutex`, recovering the data if another holder panicked.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A bounded, single-line form of fixture output that hides long token-like words.
fn diagnostic(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .split_whitespace()
        .map(|word| {
            if word.len() >= 24 && word.bytes().filter(|byte| byte.is_ascii_alphanumeric()).count() >= 20 {
                "[redacted long diagnostic token]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(4096)
        .collect()
}

async fn capture(mut reader: impl AsyncRead + Unpin) -> anyhow::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut overflow = false;
    let mut buffer = [0; 4096];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok((bytes, overflow));
        }
        if !overflow && bytes.len() + count <= OUTPUT_BOUND {
            bytes.extend_from_slice(&buffer[..count]);
        } else {
            overflow = true;
        }
    }
}

type Capture = tokio::task::JoinHandle<anyhow::Result<(Vec<u8>, bool)>>;

/// A child process this fixture owns, with its bounded output captured for the teardown receipt.
pub struct Process {
    role: &'static str,
    child: Child,
    pid: u32,
    stdout: Option<Capture>,
    stderr: Option<Capture>,
}

impl Process {
    pub fn start(role: &'static str, command: &mut Command) -> anyhow::Result<Self> {
        let mut child = command
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = child.id().context("owned fixture child omitted PID")?;
        let stdout = Some(tokio::spawn(capture(
            child.stdout.take().context("owned stdout missing")?,
        )));
        let stderr = Some(tokio::spawn(capture(
            child.stderr.take().context("owned stderr missing")?,
        )));
        Ok(Self {
            role,
            child,
            pid,
            stdout,
            stderr,
        })
    }

    pub fn alive(&mut self) -> anyhow::Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }

    /// Interrupts the child, escalates to a kill after five seconds, and returns its teardown receipt.
    pub async fn stop(&mut self) -> anyhow::Result<Value> {
        let mut forced = false;
        if self.alive()? {
            let result = output(Command::new("kill").args(["-INT", &self.pid.to_string()])).await?;
            ensure!(result.status.success(), "could not signal exact owned fixture child");
            match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
                Ok(result) => {
                    result?;
                }
                Err(_) => {
                    forced = true;
                    self.child.start_kill()?;
                    tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await??;
                }
            }
        }
        let stdout = match self.stdout.take() {
            Some(task) => tokio::time::timeout(WAIT, task).await???,
            None => (Vec::new(), false),
        };
        let stderr = match self.stderr.take() {
            Some(task) => tokio::time::timeout(WAIT, task).await???,
            None => (Vec::new(), false),
        };
        Ok(
            json!({"role":self.role,"pid":self.pid,"exit":self.child.try_wait()?.and_then(|exit| exit.code()),
            "forced_stop":forced,"stdout":diagnostic(&stdout.0),"stderr":diagnostic(&stderr.0),
            "output_overflow":stdout.1 || stderr.1}),
        )
    }
}

/// Runs `command` to completion within 30 seconds, with bounded output.
pub async fn output(command: &mut Command) -> anyhow::Result<std::process::Output> {
    let mut child = command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = tokio::spawn(capture(child.stdout.take().context("owned stdout missing")?));
    let stderr = tokio::spawn(capture(child.stderr.take().context("owned stderr missing")?));
    let status = match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(result) => result?,
        Err(_) => {
            child.start_kill()?;
            tokio::time::timeout(WAIT, child.wait()).await??;
            stdout.abort();
            stderr.abort();
            anyhow::bail!("fixture command timed out and was reaped");
        }
    };
    let stdout = tokio::time::timeout(WAIT, stdout).await???;
    let stderr = tokio::time::timeout(WAIT, stderr).await???;
    ensure!(!stdout.1 && !stderr.1, "fixture command output exceeded bound");
    Ok(std::process::Output {
        status,
        stdout: stdout.0,
        stderr: stderr.0,
    })
}

/// Like [`output`], failing unless the command exits successfully; returns its stdout.
pub async fn successful(command: &mut Command) -> anyhow::Result<Vec<u8>> {
    let output = output(command).await?;
    ensure!(
        output.status.success(),
        "fixture command failed: status={:?}; stdout={}; stderr={}",
        output.status.code(),
        diagnostic(&output.stdout),
        diagnostic(&output.stderr)
    );
    Ok(output.stdout)
}

/// Clears the environment of `command`, keeping only what reaching the Docker daemon needs.
pub fn inherit_runtime(command: &mut Command) {
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
    ] {
        if let Some(value) = trogon_std::env::ReadEnv::var_os(&trogon_std::env::SystemEnv, name) {
            command.env(name, value);
        }
    }
}

/// Starts an owned JetStream `nats-server` on a random loopback port and connects to it.
pub async fn nats(children: &mut Vec<Process>, directory: &Path) -> anyhow::Result<(Client, String)> {
    let mut command = Command::new("nats-server");
    inherit_runtime(&mut command);
    command
        .args(["-a", "127.0.0.1", "-p", "-1", "-js", "-sd"])
        .arg(directory)
        .arg("--ports_file_dir")
        .arg(directory);
    let child = Process::start("nats", &mut command)?;
    let ports = directory.join(format!("nats-server_{}.ports", child.pid));
    children.push(child);
    tokio::time::timeout(WAIT, async {
        loop {
            ensure!(
                children.last_mut().context("owned NATS child missing")?.alive()?,
                "owned NATS exited before readiness"
            );
            if let Ok(bytes) = std::fs::read(&ports)
                && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
                && let Some(url) = value["nats"][0].as_str()
                && let Ok(client) = async_nats::connect(url).await
            {
                async_nats::jetstream::new(client.clone()).query_account().await?;
                return anyhow::Ok((client, url.to_owned()));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("owned NATS readiness timed out")?
}

pub fn signer() -> anyhow::Result<Arc<DispatchSigner>> {
    let mut seed = [0; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut seed)?;
    Ok(Arc::new(DispatchSigner::new("egress-native", seed)))
}

/// Waits for the first heartbeat of the owned worker and requires that it earns assigned epoch dispatch and
/// DenyAll.
pub async fn heartbeat(client: &Client, worker: &mut Process) -> anyhow::Result<WorkerHeartbeat> {
    let mut subscription = client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::LocalDocker))
        .await?;
    client.flush().await?;
    let message = tokio::time::timeout(WAIT, subscription.next())
        .await?
        .context("actual Docker heartbeat missing")?;
    ensure!(worker.alive()?, "actual worker exited before heartbeat");
    let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
    ensure!(
        heartbeat.provider == ProviderKind::LocalDocker
            && heartbeat.profile.provider == ProviderKind::LocalDocker
            && message.subject.as_str() == subjects::worker_heartbeat(ProviderKind::LocalDocker, &heartbeat.worker_id)
            && SystemTime::now()
                .duration_since(heartbeat.emitted_at)
                .is_ok_and(|age| age <= WAIT)
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
            && heartbeat.supports_lease_epoch.is_supported()
            && heartbeat.profile.isolation.egress.deny_all.is_supported(),
        "actual fresh worker must earn assigned epoch transport and DenyAll"
    );
    Ok(heartbeat)
}

pub fn canonical(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// The bare, persistent lease request the control and the suite share, on `image` under `policy`.
fn control_request(image: &str, policy: NetworkPolicy) -> anyhow::Result<LeaseRequest> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(image),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::MIN.saturating_add(3599),
        memory_mib: NonZeroU32::MIN.saturating_add(511),
        snapshot_out: None,
        network_policy: Some(policy),
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
        tenant: TenantId::new(TENANT)?,
        provider_pref: vec![ProviderKind::LocalDocker],
        spec,
        ttl_seconds: 120,
        idempotency_key: LeaseRequestKey::new(uuid::Uuid::now_v7().to_string())?,
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

/// The spec the suite leases with: bare boot of `image`. The suite sets the network policy per lease.
pub fn suite_spec(image: &str) -> anyhow::Result<SandboxSpec> {
    Ok(control_request(image, NetworkPolicy::AllowAll)?.spec)
}

/// Acquires an AllowAll lease on the worker that sent `heartbeat`, independently of the suite, and returns it as a
/// grant whose cleanup authority is earned only when the reply matches the exact assignment.
pub async fn acquire_control(
    client: &Client,
    signer: &DispatchSigner,
    heartbeat: &WorkerHeartbeat,
    image: &str,
) -> anyhow::Result<ConformanceGrant> {
    let lease_id = LeaseId::new(uuid::Uuid::now_v7().to_string());
    let target = DispatchTarget {
        worker_id: heartbeat.worker_id.clone(),
        attempt_id: DispatchAttemptId::new(uuid::Uuid::now_v7().to_string())?,
        epoch: Some(LeaseEpoch::FIRST),
    };
    let payload = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request: control_request(image, NetworkPolicy::AllowAll)?,
        correlation_id: None,
        resolved_drives: vec![],
        target: Some(target.clone()),
    };
    let body = signer.sign_payload(&serde_json::to_vec(&payload)?, OP_LEASE_DISPATCH)?;
    let reply = client
        .send_request(
            subjects::worker_dispatch(ProviderKind::LocalDocker, &heartbeat.worker_id),
            async_nats::Request::new().payload(body.into()).timeout(Some(WAIT)),
        )
        .await?;
    let reply: ProviderDispatchReply =
        serde_json::from_slice(&reply.payload).context("control dispatch reply did not match the public contract")?;
    let ProviderDispatchReply::Granted(native) = reply else {
        anyhow::bail!("independent AllowAll control was not granted");
    };
    let exact = native.worker_id.as_ref() == Some(&target.worker_id)
        && native.attempt_id.as_ref() == Some(&target.attempt_id)
        && native.epoch == target.epoch;
    Ok(ConformanceGrant {
        provider: ProviderKind::LocalDocker,
        lease_id,
        native,
        native_cleanup_authority: if exact {
            NativeCleanupAuthority::RequestMatched
        } else {
            NativeCleanupAuthority::ReconciliationRequired
        },
    })
}

pub async fn absent(docker: &Docker, id: &str) -> anyhow::Result<bool> {
    ensure!(canonical(id), "absence requires exact canonical native identity");
    match docker.inspect_container(id, None).await {
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(true),
        Ok(value) if value.id.as_deref() == Some(id) => Ok(false),
        _ => anyhow::bail!("exact native absence is unknown"),
    }
}

pub async fn wait_absent(docker: &Docker, id: &str) -> anyhow::Result<()> {
    tokio::time::timeout(WAIT, async {
        loop {
            if absent(docker, id).await? {
                break anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .context("exact native absence did not settle")?
}

#[derive(Clone, Serialize)]
pub struct Allocation {
    pub lease: String,
    pub role: String,
    pub network: String,
    pub image_verified: bool,
}

#[derive(Clone, Default, Serialize)]
pub struct Trace {
    pub allocations: BTreeMap<String, Allocation>,
    pub errors: Vec<&'static str>,
}

/// Records every container the worker `owner` creates, with its lease, role, network mode and whether it runs
/// `image`, until the token is cancelled.
pub fn observe(
    docker: &Docker,
    owner: String,
    image: String,
) -> (Arc<Mutex<Trace>>, CancellationToken, tokio::task::JoinHandle<()>) {
    let trace = Arc::new(Mutex::new(Trace::default()));
    let cancel = CancellationToken::new();
    let state = trace.clone();
    let stop = cancel.clone();
    let docker = docker.clone().with_timeout(Duration::from_secs(600));
    let task = tokio::spawn(async move {
        let filters = HashMap::from([
            ("type".to_owned(), vec!["container".to_owned()]),
            ("event".to_owned(), vec!["create".to_owned()]),
            ("label".to_owned(), vec![format!("{DOCKER_LABEL_OWNER}={owner}")]),
        ]);
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        let mut events = docker.events(Some(
            EventsOptionsBuilder::new().since(&since).filters(&filters).build(),
        ));
        loop {
            let event = tokio::select! { _ = stop.cancelled() => break, value = events.next() => value };
            let Some(Ok(event)) = event else {
                lock(&state).errors.push("native_event_stream_ended");
                break;
            };
            let Some(actor) = event.actor else {
                continue;
            };
            let Some(id) = actor.id.filter(|id| canonical(id)) else {
                continue;
            };
            let labels = actor.attributes.unwrap_or_default();
            if labels.get(DOCKER_LABEL_OWNER) != Some(&owner) {
                continue;
            }
            let Some(lease) = labels.get(DOCKER_LABEL_LEASE_ID).cloned() else {
                lock(&state).errors.push("native_allocation_lease_missing");
                continue;
            };
            if lock(&state).allocations.len() >= 16 {
                lock(&state).errors.push("native_allocation_bound");
                break;
            }
            let mut allocation = Allocation {
                lease,
                role: labels.get(DOCKER_LABEL_ROLE).cloned().unwrap_or_default(),
                network: String::new(),
                image_verified: false,
            };
            lock(&state).allocations.insert(id.clone(), allocation.clone());
            match docker.inspect_container(&id, None).await {
                Ok(value) if value.id.as_deref() == Some(id.as_str()) => {
                    let matched = value
                        .config
                        .as_ref()
                        .and_then(|config| config.labels.as_ref())
                        .is_some_and(|labels| {
                            labels.get(DOCKER_LABEL_OWNER) == Some(&owner)
                                && labels.get(DOCKER_LABEL_LEASE_ID) == Some(&allocation.lease)
                        });
                    if !matched {
                        lock(&state).errors.push("native_live_owner_changed");
                        continue;
                    }
                    allocation.network = value
                        .host_config
                        .as_ref()
                        .and_then(|host| host.network_mode.clone())
                        .unwrap_or_default();
                    allocation.image_verified = value.image.as_deref() == Some(image.as_str());
                    lock(&state).allocations.insert(id, allocation);
                }
                _ => lock(&state).errors.push("native_live_provenance_unknown"),
            }
        }
    });
    (trace, cancel, task)
}

/// Releases `grant` through the exact assigned cleanup verb and requires the worker to confirm it.
pub async fn cancel(
    client: &Client,
    signer: &DispatchSigner,
    grant: &ConformanceGrant,
    worker: &trogon_sandbox_contract::WorkerId,
) -> anyhow::Result<()> {
    ensure!(
        grant.provider == ProviderKind::LocalDocker
            && grant.native_cleanup_authority == NativeCleanupAuthority::RequestMatched
            && grant.native.worker_id.as_ref() == Some(worker)
            && grant.native.attempt_id.is_some()
            && grant.native.epoch.is_some_and(|epoch| epoch.get() > 0),
        "cleanup requires exact assigned native authority"
    );
    let request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: grant.lease_id.clone(),
            provider: ProviderKind::LocalDocker,
            target: DispatchTarget {
                worker_id: worker.clone(),
                attempt_id: grant
                    .native
                    .attempt_id
                    .clone()
                    .context("cleanup attempt identity missing")?,
                epoch: grant.native.epoch,
            },
        },
        intent: CancelIntent::Release,
    };
    let subject = subjects::worker_cleanup(ProviderKind::LocalDocker, worker);
    let bytes = serde_json::to_vec(&request)?;
    let scope = VerbScope {
        tenant: TenantId::new(TENANT)?,
        lease_id: grant.lease_id.clone(),
        worker_id: Some(worker.clone()),
        attempt_id: grant.native.attempt_id.clone(),
        epoch: grant.native.epoch,
    };
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &bytes, &scope)?.as_str());
    let reply = client
        .send_request(
            subject,
            async_nats::Request::new()
                .headers(headers)
                .payload(bytes.into())
                .timeout(Some(WAIT)),
        )
        .await?;
    let response: ProviderCleanupResponse = serde_json::from_slice(&reply.payload)?;
    ensure!(
        matches!(&response, ProviderCleanupResponse::Cleaned { identity, intent }
            if identity == &request.identity && intent == &request.intent),
        "exact assigned fixture cleanup did not confirm its retained identity and intent"
    );
    eprintln!(
        "native fixture assigned cleanup receipt: {}",
        serde_json::to_value(&response)?
    );
    Ok(())
}

/// Every container, running or not, that carries `owner`.
pub async fn owned(docker: &Docker, owner: &str) -> anyhow::Result<Vec<String>> {
    let filters = HashMap::from([("label".to_owned(), vec![format!("{DOCKER_LABEL_OWNER}={owner}")])]);
    docker
        .list_containers(Some(
            ListContainersOptionsBuilder::new().all(true).filters(&filters).build(),
        ))
        .await?
        .into_iter()
        .map(|container| {
            container
                .id
                .filter(|id| canonical(id))
                .context("owned native ID is not canonical")
        })
        .collect()
}

/// Force-removes `id` only while it still carries `owner` (and `lease`, when given), then waits for it to go.
pub async fn remove_owned(docker: &Docker, id: &str, owner: &str, lease: Option<&str>) -> anyhow::Result<()> {
    ensure!(canonical(id), "fixture cleanup requires canonical ID");
    let current = docker.inspect_container(id, None).await?;
    ensure!(
        current.id.as_deref() == Some(id)
            && current
                .config
                .as_ref()
                .and_then(|config| config.labels.as_ref())
                .is_some_and(
                    |labels| labels.get(DOCKER_LABEL_OWNER).is_some_and(|value| value == owner)
                        && lease
                            .is_none_or(|lease| labels.get(DOCKER_LABEL_LEASE_ID).is_some_and(|value| value == lease))
                ),
        "fixture cleanup refuses changed owner or lease"
    );
    docker
        .remove_container(
            id,
            Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
        )
        .await?;
    wait_absent(docker, id).await
}
