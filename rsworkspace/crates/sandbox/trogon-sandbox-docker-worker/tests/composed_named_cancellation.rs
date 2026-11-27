//! Named command cancellation through the composed secure stack: a real SpiceDB, a real broker, the actual Docker
//! worker, and the Rust client SDK, with an independent readonly observer witnessing the native process families.
#![cfg(unix)]

#[path = "composed_named_support/native_support.rs"]
mod native_support;
#[path = "composed_named_support/process_support.rs"]
mod process_support;
use native_support::*;
use process_support::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, ensure};
use async_nats::{Client, HeaderMap};
use base64::Engine;
use bollard::Docker;
use bollard::query_parameters::{
    DownloadFromContainerOptionsBuilder, EventsOptionsBuilder, RemoveContainerOptionsBuilder,
    RemoveVolumeOptionsBuilder,
};
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{FutureExt, StreamExt};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_client::{Bus, BusError, BusMessage, ClientError, CommandOptions, NatsBus, SandboxClient};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CommandDeadline, CommandEnv, CommandExecution,
    CommandLaunchOrigin, ExecutionId, LaunchCancellation, LaunchKey, LeaseCreation, LeaseError, LeaseGranted, LeaseId,
    LeaseRequest, LeaseRequestKey, LeaseResponse, OciImageRef, PollCommandResponse, ProcessName, ProcessSpec,
    ProviderKind, RunCommandRequest, RunCommandResponse, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags,
    StdinMode, TenantId, WorkerDispatchProtocol, WorkerHeartbeat, WorkspaceSource,
};
use trogon_sandbox_docker_worker::adapter::command_runtime::CommandRuntime;
use trogon_sandbox_protocol::subjects::LEASE_REQUEST;
use trogon_sandbox_protocol::{DispatchSigner, subjects, tenant_kv_key};
use trogon_sandbox_providers::{
    ContainerHardening, DOCKER_LABEL_LEASE_ID, DOCKER_LABEL_OWNER, DOCKER_LABEL_ROLE, EGRESS_PROXY_IMAGE,
};
use trogon_std::env::{ReadEnv, SystemEnv};
use trusted_observer_support::{Identity, Native, Operation, Request, Response, TrackedState, fresh_id};

mod trusted_observer_support;

const WAIT: Duration = Duration::from_secs(15);
const DIAGNOSTIC_BYTES: usize = 8192;
const CAPTURE_BYTES: usize = 128 * 1024;
const SPICEDB_IMAGE: &str = "sha256:22e6100cb8fad42d5a052f83c915624e21260ccee71b785a6ab79b6872d7e9d7";
const WORKLOAD_IMAGE: &str = "docker.io/library/node:22-bookworm-slim";
const RUNTIME_DIGEST: &str = "sandbox.trogon.live/command-runtime-digest";
const RUNTIME_NONCE: &str = "sandbox.trogon.live/command-runtime-registration";
const RUNTIME_MOUNT_PREFIX: &str = "/.trogon-sandbox-run-";
const WORKLOAD_ROLE: &str = "workload";
const FAMILY_ENV: &str = "SANDBOX_ACCEPTANCE_FAMILY";

/// The child of a named family: it records its own kernel identity and then idles, ignoring SIGTERM.
const CHILD_SCRIPT: &str = r#"const fs=require('node:fs');process.on('SIGTERM',()=>{});const f=fs.readFileSync('/proc/self/stat','utf8');fs.writeFileSync(process.argv[1]+'.tmp',JSON.stringify({pid:process.pid,startTime:f.slice(f.lastIndexOf(')')+1).trim().split(/\s+/)[19],marker:process.argv[2]}));fs.renameSync(process.argv[1]+'.tmp',process.argv[1]);setInterval(()=>{},1000);"#;
const PARENT_HEAD: &str = r#"const fs=require('node:fs');process.on('SIGTERM',()=>{});require('node:child_process').spawn(process.execPath,['-e',"#;
const PARENT_TAIL: &str = r#",process.argv[2],process.argv[4]],{stdio:'ignore'});const f=fs.readFileSync('/proc/self/stat','utf8');fs.appendFileSync(process.argv[3],process.argv[4]+'\n');fs.writeFileSync(process.argv[1]+'.tmp',JSON.stringify({pid:process.pid,startTime:f.slice(f.lastIndexOf(')')+1).trim().split(/\s+/)[19],marker:process.argv[4]}));fs.renameSync(process.argv[1]+'.tmp',process.argv[1]);process.stdout.write(process.argv[4]);setInterval(()=>{},1000);"#;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn redact(line: &[u8], secrets: &[String]) -> String {
    let mut text = String::from_utf8_lossy(line).into_owned();
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        text = text.replace(secret, "[redacted]");
    }
    let mut diagnostic = text
        .split_whitespace()
        .map(|word| {
            if word.len() >= 32 && word.bytes().filter(|byte| byte.is_ascii_alphanumeric()).count() >= 24 {
                "[redacted long diagnostic token]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if diagnostic.len() > DIAGNOSTIC_BYTES {
        let mut start = diagnostic.len() - DIAGNOSTIC_BYTES;
        while !diagnostic.is_char_boundary(start) {
            start += 1;
        }
        diagnostic.drain(..start);
    }
    diagnostic
}

fn append_tail(tail: &Arc<Mutex<String>>, line: String) {
    let mut tail = locked(tail);
    tail.push_str(&line);
    tail.push('\n');
    while tail.len() > DIAGNOSTIC_BYTES {
        let end = tail.find('\n').map_or(tail.len(), |index| index + 1);
        tail.drain(..end);
    }
}

async fn drain_tail<R: AsyncRead + Unpin>(mut reader: R, tail: Arc<Mutex<String>>, secrets: Vec<String>) {
    let mut buffer = [0; 2048];
    let mut line = Vec::new();
    let mut discarded = false;
    while let Ok(count) = reader.read(&mut buffer).await {
        if count == 0 {
            break;
        }
        for byte in &buffer[..count] {
            if *byte == b'\n' {
                append_tail(
                    &tail,
                    if discarded {
                        "[oversized diagnostic line omitted]".into()
                    } else {
                        redact(&line, &secrets)
                    },
                );
                line.clear();
                discarded = false;
            } else if !discarded {
                if line.len() == DIAGNOSTIC_BYTES {
                    line.clear();
                    discarded = true;
                } else {
                    line.push(*byte);
                }
            }
        }
    }
    if discarded {
        append_tail(&tail, "[oversized diagnostic line omitted]".into());
    } else if !line.is_empty() {
        append_tail(&tail, redact(&line, &secrets));
    }
}

#[tokio::test]
async fn diagnostic_tails_redact_split_secrets_and_keep_startup_errors() -> anyhow::Result<()> {
    let secret = format!("owned-{}-diagnostic", "value".repeat(10));
    let input = format!("{} {secret} startup_failure\n", "padding ".repeat(254));
    let tail = Arc::new(Mutex::new(String::new()));
    drain_tail(input.as_bytes(), tail.clone(), vec![secret.clone()]).await;
    let observed = locked(&tail);
    ensure!(!observed.contains(&secret), "the secret leaked into the tail");
    ensure!(observed.contains("startup_failure"), "the startup error was dropped");
    ensure!(observed.contains("[redacted]"), "the secret was not marked as redacted");
    ensure!(observed.len() <= DIAGNOSTIC_BYTES, "the tail exceeded its bound");
    Ok(())
}

#[tokio::test]
async fn oversized_diagnostic_lines_are_omitted_without_partial_secret_output() -> anyhow::Result<()> {
    let input = format!("{}\nstartup_failure\n", "sensitive".repeat(DIAGNOSTIC_BYTES));
    let tail = Arc::new(Mutex::new(String::new()));
    drain_tail(input.as_bytes(), tail.clone(), Vec::new()).await;
    let observed = locked(&tail);
    ensure!(
        !observed.contains("sensitive"),
        "part of the oversized line leaked into the tail"
    );
    ensure!(
        observed.contains("oversized diagnostic line omitted"),
        "the oversized line was not marked as omitted"
    );
    ensure!(observed.contains("startup_failure"), "the startup error was dropped");
    Ok(())
}

#[derive(Clone, Default, Serialize)]
struct NativeRecord {
    lease: String,
    role: String,
    image_verified: bool,
    isolation_verified: bool,
    artifact_verified: bool,
    runtime_volume: Option<String>,
}

#[derive(Clone, Default, Serialize)]
struct NativeTrace {
    containers: BTreeMap<String, NativeRecord>,
    runtime_writers: BTreeSet<String>,
    runtime_volumes: BTreeSet<String>,
    errors: Vec<&'static str>,
}

fn lease_request(tenant: &TenantId, image: &str) -> anyhow::Result<LeaseRequest> {
    let spec = SandboxSpec::new(SandboxSpecParts {
        image: SandboxImage::Registry {
            reference: OciImageRef::parse_unqualified(image),
            pull_secret: None,
        },
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: std::num::NonZeroU32::new(60).context("cpu seconds")?,
        memory_mib: std::num::NonZeroU32::new(256).context("memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: vec![],
        drives: vec![],
        exposed_ports: vec![],
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
        tenant: tenant.clone(),
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

/// What the lossy bus saw of the one strong named launch it is armed for.
#[derive(Default)]
struct Loss {
    process_name: String,
    launch_key: Option<LaunchKey>,
    execution_id: Option<ExecutionId>,
    origin: Option<CommandLaunchOrigin>,
    suppressed: u32,
}

/// A bus that delivers everything except the Started reply of an armed strong named launch, which it swallows so
/// the caller never learns the launch began.
struct LossBus {
    inner: NatsBus,
    loss: Arc<Mutex<Option<Loss>>>,
    observed: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Bus for LossBus {
    async fn request(
        &self,
        subject: &str,
        payload: Bytes,
        timeout: Duration,
        headers: Option<HeaderMap>,
    ) -> Result<Option<Bytes>, BusError> {
        let launch = serde_json::from_slice::<RunCommandRequest>(&payload)
            .ok()
            .filter(|request| {
                subject == subjects::lease_run_command(&request.lease_id)
                    && request.require_launch_cancellation == LaunchCancellation::Required
            });
        let armed = launch.is_some_and(|launch| {
            let mut loss = locked(&self.loss);
            let Some(active) = loss.as_mut() else {
                return false;
            };
            let named = matches!(
                &launch.execution,
                CommandExecution::Detached { process: Some(ProcessSpec { name: Some(name), .. }), .. }
                    if name.as_str() == active.process_name
            );
            if !named || active.suppressed != 0 {
                return false;
            }
            active.launch_key = launch.launch_key;
            true
        });
        let reply = self.inner.request(subject, payload, timeout, headers).await?;
        if !armed {
            return Ok(reply);
        }
        let started = reply
            .as_ref()
            .and_then(|bytes| serde_json::from_slice::<RunCommandResponse>(bytes).ok());
        let Some(RunCommandResponse::Started(started)) = started else {
            return Ok(reply);
        };
        if let Some(active) = locked(&self.loss).as_mut() {
            active.execution_id = Some(started.execution_id);
            active.origin = Some(started.origin);
            active.suppressed += 1;
        }
        self.observed.notify_one();
        std::future::pending().await
    }

    async fn subscribe(&self, subject: &str) -> Result<BoxStream<'static, BusMessage>, BusError> {
        self.inner.subscribe(subject).await
    }
}

/// Where the readonly observer looks: the exact lease and workload container the SDK was granted.
struct Scope<'a> {
    rpc: &'a trusted_observer_support::Rpc,
    lease: &'a str,
    container: &'a str,
}

fn process_script() -> anyhow::Result<String> {
    Ok(format!(
        "{PARENT_HEAD}{}{PARENT_TAIL}",
        serde_json::to_string(CHILD_SCRIPT)?
    ))
}

fn paths_for(root: &str, name: &str) -> [String; 3] {
    [
        format!("{root}/{name}.root.json"),
        format!("{root}/{name}.child.json"),
        format!("{root}/{name}.starts"),
    ]
}

/// A detached named launch of the two-process family script; `strong` asks for an assigned, cancelable launch.
fn named_launch(lease: &LeaseId, name: &str, paths: &[String; 3], strong: bool) -> anyhow::Result<RunCommandRequest> {
    let options = CommandOptions::default();
    let mut command = vec!["node".to_owned(), "-e".to_owned(), process_script()?];
    command.extend(paths.iter().cloned());
    command.push(name.to_owned());
    Ok(RunCommandRequest {
        lease_id: lease.clone(),
        command,
        cwd: options.cwd,
        env: CommandEnv::new(BTreeMap::from([(FAMILY_ENV.to_owned(), name.to_owned())]))?,
        timeout_seconds: options.timeout_seconds,
        stream_output: options.stream_output,
        run_as_user: options.run_as_user,
        output_retention: options.output_retention,
        max_output_bytes: None,
        redact_values: options.redact_values,
        launch_key: if strong {
            Some(LaunchKey::new(uuid::Uuid::new_v4().simple().to_string())?)
        } else {
            None
        },
        require_launch_cancellation: if strong {
            LaunchCancellation::Required
        } else {
            LaunchCancellation::NotRequired
        },
        execution: CommandExecution::Detached {
            stdin: StdinMode::Closed,
            deadline: CommandDeadline::Lease,
            process: Some(ProcessSpec {
                name: Some(ProcessName::new(name)?),
                readiness: None,
            }),
        },
    })
}

/// Sends an ordinary named launch and returns its execution and whether the provider created or reused it.
async fn ordinary_launch(
    sdk: &SandboxClient,
    lease: &LeaseId,
    name: &str,
    paths: &[String; 3],
) -> anyhow::Result<(ExecutionId, CommandLaunchOrigin)> {
    match sdk.send_run_command(named_launch(lease, name, paths, false)?).await? {
        RunCommandResponse::Started(started) => Ok((started.execution_id, started.origin)),
        other => anyhow::bail!("ordinary named launch did not start: {other:?}"),
    }
}

/// Observes the named family through the readonly observer until both of its identity records exist.
async fn native_family(
    scope: &Scope<'_>,
    paths: &[String; 3],
    marker: &str,
    tracked: &[Identity],
) -> anyhow::Result<Native> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let request = Request {
            kind: Operation::NativeFamily,
            request_id: fresh_id(),
            lease_id: scope.lease.to_owned(),
            container_id: scope.container.to_owned(),
            marker: marker.to_owned(),
            paths: paths.clone(),
            tracked: tracked.to_vec(),
        };
        let state = match scope.rpc.call(&request).await? {
            Response::Observed { request_id, native, .. } => {
                ensure!(request_id == request.request_id, "native observer response mismatch");
                native
            }
            other => anyhow::bail!("native observer did not observe the family: {other:?}"),
        };
        if state.ready {
            match state.records.as_slice() {
                [root, child] if root.marker == marker && child.marker == marker => {
                    ensure!(root.pid != child.pid, "native child identity is not distinct");
                }
                _ => anyhow::bail!("native family identity missing"),
            }
            return Ok(state);
        }
        ensure!(tokio::time::Instant::now() < deadline, "native family did not start");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn require_live(state: &Native, marker: &str) -> anyhow::Result<()> {
    ensure!(state.starts == [marker], "native user effect duplicated");
    ensure!(
        state.tracked.len() >= 2
            && state
                .tracked
                .iter()
                .all(|identity| identity.state == TrackedState::Alive),
        "native family is not alive"
    );
    ensure!(
        state.records.iter().all(|identity| {
            state
                .members
                .iter()
                .any(|member| member.pid == identity.pid && member.start_time == identity.start_time)
        }),
        "native family marker control failed"
    );
    Ok(())
}

fn family(state: &Native) -> Vec<String> {
    let mut identities = state
        .members
        .iter()
        .map(|identity| format!("{}:{}", identity.pid, identity.start_time))
        .collect::<Vec<_>>();
    identities.sort();
    identities
}

fn require_same_family(before: &Native, after: &Native) -> anyhow::Result<()> {
    ensure!(family(before) == family(after), "named native family census changed");
    Ok(())
}

fn family_gone(state: &Native) -> bool {
    state.members.is_empty()
        && state
            .tracked
            .iter()
            .all(|identity| identity.state == TrackedState::Absent)
}

/// Cancels the launch carrying `request.launch_key`, waiting out a pending cleanup until it settles.
async fn settled_cancel(
    sdk: &SandboxClient,
    request: &CancelCommandLaunchRequest,
) -> anyhow::Result<CancelCommandLaunchResponse> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match sdk.cancel_command_launch(request.clone()).await? {
                CancelCommandLaunchResponse::Pending { .. } => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                settled => break anyhow::Ok(settled),
            }
        }
    })
    .await
    .context("named launch cancellation did not settle")?
}

async fn remove_container(docker: &Docker, id: &str) {
    let _ = docker
        .remove_container(
            id,
            Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
        )
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires SANDBOX_DOCKER_COMPOSED_NAMED=1, SANDBOX_DOCKER_COMMAND_SUPERVISOR, actual Docker, cached node:22, SpiceDB and egress proxy images, grpcurl, nats-server and a built trogon-sandbox-broker; run with --ignored"]
async fn actual_docker_named_cancellation_uses_the_secure_composed_sdk_stack() -> anyhow::Result<()> {
    ensure!(
        ReadEnv::var(&SystemEnv, "SANDBOX_DOCKER_COMPOSED_NAMED").as_deref() == Ok("1"),
        "set SANDBOX_DOCKER_COMPOSED_NAMED=1 for owned secure composition"
    );
    let supervisor = PathBuf::from(
        ReadEnv::var_os(&SystemEnv, "SANDBOX_DOCKER_COMMAND_SUPERVISOR")
            .context("the actual trusted static supervisor artifact must be configured")?,
    );
    let egress_reference = ReadEnv::var(&SystemEnv, "SANDBOX_EGRESS_PROXY_IMAGE")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| EGRESS_PROXY_IMAGE.to_owned());
    let directory = tempfile::tempdir()?;
    let docker = Docker::connect_with_defaults()?.with_timeout(Duration::from_secs(20));
    let owner = format!("secure-docker-{}", fresh_id());
    let mut children = Vec::new();
    let mut spice_id = None;
    let mut native = None;
    let mut native_observer: Option<(Arc<SdkNativeObserver>, trusted_observer_support::Rpc)> = None;
    let mut grant: Option<LeaseGranted> = None;
    let mut product_cleanup = false;
    let mut redaction_secrets = Vec::new();
    let mut report = json!({"phase":"preflight", "fallback_used":false});
    let operation = std::panic::AssertUnwindSafe(async {
        let broker = binary("trogon-sandbox-broker")?;
        let worker = PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-docker-worker"));
        let runtime = CommandRuntime::load(&supervisor, owner.clone())?;
        let digest = runtime.digest().to_owned();
        report["supervisor_sha256"] = digest.clone().into();
        let information = docker.info().await?;
        report["docker_daemon_id"] = information.id.context("actual Docker daemon identity missing")?.into();
        let image = docker
            .inspect_image(WORKLOAD_IMAGE)
            .await
            .context("the exact Node22 workload image must already be cached")?
            .id
            .context("cached workload image lacks canonical identity")?;
        ensure!(
            image.strip_prefix("sha256:").is_some_and(canonical),
            "cached workload image is not canonical"
        );
        docker
            .inspect_image(SPICEDB_IMAGE)
            .await
            .context("the pinned real SpiceDB image must already be cached")?;
        let egress_proxy_image = docker
            .inspect_image(&egress_reference)
            .await
            .context("the existing egress proxy image must already be cached; this lane never pulls it")?
            .id
            .context("cached egress proxy image lacks canonical identity")?;
        ensure!(
            egress_proxy_image.strip_prefix("sha256:").is_some_and(canonical),
            "cached egress proxy image is not canonical"
        );
        report["workload_image"] = json!({"reference":WORKLOAD_IMAGE,"id":image});
        report["egress_proxy_image"] = json!({"reference":egress_reference,"id":egress_proxy_image});
        let (trace, cancel, task) = native_trace(&docker, owner.clone(), image.clone(), digest.clone());
        native = Some((trace.clone(), cancel, task));
        let host = Arc::new(SdkNativeObserver {
            docker: docker.clone(),
            trace: trace.clone(),
            owner: owner.clone(),
            image,
            digest,
            bound: tokio::sync::Mutex::new(None),
        });
        let rpc = trusted_observer_support::Rpc::start_host(host.clone()).await?;
        native_observer = Some((host, rpc));

        let tenant = TenantId::new(format!("secure-docker-{}", fresh_id()))?;
        let principal = format!("secure-docker-{}", fresh_id());
        let token = fresh_id() + &fresh_id();
        let api_key = fresh_id() + &fresh_id();
        let denied_key = fresh_id() + &fresh_id();
        let unknown_key = fresh_id() + &fresh_id();
        let (seed, _) = DispatchSigner::generate_keypair_b64();
        let decoded = base64::engine::general_purpose::STANDARD.decode(&seed)?;
        let signer = DispatchSigner::new(
            "secure-docker",
            <[u8; 32]>::try_from(decoded.as_slice()).map_err(|_| anyhow::anyhow!("ephemeral signing seed invalid"))?,
        );
        let secrets = vec![
            token.clone(),
            api_key.clone(),
            denied_key.clone(),
            unknown_key.clone(),
            seed.clone(),
        ];
        redaction_secrets = secrets.clone();

        report["phase"] = "real_spicedb".into();
        let created = successful(
            Command::new("docker").env("SPICEDB_GRPC_PRESHARED_KEY", &token).args([
                "run",
                "--detach",
                "--rm",
                "--pull",
                "never",
                "--publish",
                "127.0.0.1::50051",
                "--env",
                "SPICEDB_GRPC_PRESHARED_KEY",
                SPICEDB_IMAGE,
                "serve",
                "--datastore-engine",
                "memory",
                "--grpc-addr",
                "0.0.0.0:50051",
            ]),
            None,
        )
        .await?;
        let cid = std::str::from_utf8(&created)?.trim().to_owned();
        ensure!(canonical(&cid), "real SpiceDB omitted its exact canonical container identity");
        spice_id = Some(cid.clone());
        let spice = docker.inspect_container(&cid, None).await?;
        ensure!(
            spice.id.as_deref() == Some(cid.as_str())
                && spice.image.as_deref() == Some(SPICEDB_IMAGE)
                && spice.state.as_ref().and_then(|state| state.running) == Some(true),
            "SpiceDB positive control must belong to the identical native Docker daemon"
        );
        let port = successful(Command::new("docker").args(["port", &cid, "50051/tcp"]), None).await?;
        let address = std::str::from_utf8(&port)?.trim().to_owned();
        ensure!(address.starts_with("127.0.0.1:"), "real SpiceDB must bind only loopback");
        let schema_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../devops/helm/charts/spicedb/schema.zed");
        let schema = std::fs::read_to_string(&schema_path).context("the sandbox SpiceDB schema must exist")?;
        tokio::time::timeout(WAIT, async {
            loop {
                if grpc(
                    &address,
                    &token,
                    "authzed.api.v1.SchemaService/WriteSchema",
                    json!({"schema":schema}),
                )
                .await
                .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("real SpiceDB readiness did not settle")?;
        grpc(
            &address,
            &token,
            "authzed.api.v1.PermissionsService/WriteRelationships",
            json!({"updates":[{
                "operation":"OPERATION_CREATE", "relationship":{
                    "resource":{"objectType":"sandbox_tenant","objectId":tenant.as_str()}, "relation":"operator",
                    "subject":{"object":{"objectType":"service_account","objectId":principal}}
                }}]}),
        )
        .await?;
        let permission = grpc(
            &address,
            &token,
            "authzed.api.v1.PermissionsService/CheckPermission",
            json!({
                "consistency":{"fullyConsistent":true},
                "resource":{"objectType":"sandbox_tenant","objectId":tenant.as_str()},
                "permission":"lease_sandbox",
                "subject":{"object":{"objectType":"service_account","objectId":principal}}
            }),
        )
        .await?;
        ensure!(
            permission["permissionship"] == "PERMISSIONSHIP_HAS_PERMISSION",
            "actual SpiceDB must permit the exact tenant and principal"
        );

        let (client, nats_url) = nats(&mut children, directory.path()).await?;
        let js = async_nats::jetstream::new(client.clone());
        let keys = js
            .create_or_update_key_value(async_nats::jetstream::kv::Config {
                bucket: "sandbox-api-keys".into(),
                ..Default::default()
            })
            .await?;
        for (key, service_account) in [(&api_key, principal.as_str()), (&denied_key, "unrelated-principal")] {
            keys.put(
                hex_sha256(key.as_bytes()),
                serde_json::to_vec(&json!({"tenant":tenant.as_str(),"service_account":service_account,
                    "name":"owned-secure-docker","created_at":SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()}))?
                .into(),
            )
            .await?;
        }
        let policies = js
            .create_or_update_key_value(async_nats::jetstream::kv::Config {
                bucket: "sandbox-image-allowlist".into(),
                ..Default::default()
            })
            .await?;
        policies
            .put(
                tenant_kv_key(&tenant),
                serde_json::to_vec(&json!({"patterns":[WORKLOAD_IMAGE],"registries":[]}))?.into(),
            )
            .await?;

        report["phase"] = "secure_stack_startup".into();
        let mut command = Command::new(broker);
        inherit_runtime(&mut command);
        command
            .env("SPICEDB_TOKEN", &token)
            .env("SANDBOX_DISPATCH_SIGNING_KEY", &seed)
            .env("SANDBOX_DISPATCH_SIGNING_KEY_ID", signer.key_id())
            .args([
                "--nats-url",
                &nats_url,
                "--spicedb-endpoint",
                &format!("http://{address}"),
                "--quota-backend",
                "nats-kv",
                "--idempotency-backend",
                "nats-kv",
                "--jetstream-replicas",
                "1",
                "--entity-mode",
                "off",
                "--metrics-addr",
                "127.0.0.1:0",
            ]);
        children.push(ManagedChild::start("broker", &mut command, &secrets)?);
        let mut heartbeats = client
            .subscribe(subjects::worker_heartbeat_filter(ProviderKind::LocalDocker))
            .await?;
        client.flush().await?;
        let state_dir = directory.path().join("worker-state");
        std::fs::create_dir(&state_dir)?;
        std::fs::set_permissions(&state_dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
        let mut command = Command::new(worker);
        inherit_runtime(&mut command);
        command
            .env("SANDBOX_NATS_URL", &nats_url)
            .env("SANDBOX_DOCKER_WORKER_OWNER", &owner)
            .env("SANDBOX_DOCKER_STATE_DIR", &state_dir)
            .env("SANDBOX_DOCKER_COMMAND_SUPERVISOR", &supervisor)
            .env("SANDBOX_IMAGE", WORKLOAD_IMAGE)
            .env("SANDBOX_EGRESS_PROXY_IMAGE", &egress_proxy_image)
            .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
            .env("SANDBOX_WORKER_AUTHORITY", "enforce")
            .env("SANDBOX_FENCE_EPOCH", "require")
            .env("SANDBOX_DISPATCH_VERIFY", "require")
            .env("SANDBOX_DISPATCH_FENCE_FILE", directory.path().join("worker.fence"))
            .env(
                "SANDBOX_DISPATCH_PUBLIC_KEYS",
                format!("{}:{}", signer.key_id(), signer.public_key_b64()),
            );
        report["phase"] = "actual_worker_startup".into();
        children.push(ManagedChild::start("docker-worker", &mut command, &secrets)?);
        let heartbeat = tokio::time::timeout(WAIT, heartbeats.next())
            .await?
            .context("actual Docker heartbeat missing")?;
        let heartbeat: WorkerHeartbeat = serde_json::from_slice(&heartbeat.payload)?;
        ensure!(
            heartbeat.provider == ProviderKind::LocalDocker
                && heartbeat.profile.provider == ProviderKind::LocalDocker
                && !heartbeat.worker_id.as_str().is_empty(),
            "actual Docker worker heartbeat has invalid provenance"
        );
        report["actual_worker"] = json!({"worker_id":heartbeat.worker_id.as_str(),
            "dispatch_protocol":heartbeat.dispatch_protocol,"supports_lease_epoch":heartbeat.supports_lease_epoch});
        ensure!(
            heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1
                && heartbeat.supports_lease_epoch.is_supported(),
            "actual Docker worker has not earned the broker's assigned strong-command gate"
        );

        report["phase"] = "secure_broker_controls".into();
        let lease = lease_request(&tenant, WORKLOAD_IMAGE)?;
        let mut controls = Vec::new();
        for (key, reason) in [(None, "MissingHeader"), (Some(denied_key.as_str()), "PermissionDenied")] {
            let response = tokio::time::timeout(WAIT, async {
                loop {
                    if let Ok(response) = request(&client, &lease, key).await {
                        break response;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .context("real secure broker did not become ready")?;
            controls.push(denial_summary(&response, &secrets));
            ensure!(
                matches!(&response, LeaseResponse::Denied(LeaseError::Unauthorized { reason: observed }) if observed == reason),
                "real secure broker accepted missing or unauthorized credentials"
            );
        }
        let unlisted = lease_request(&tenant, "unlisted-secure-docker")?;
        let denied = request(&client, &unlisted, Some(&api_key)).await?;
        report["image_policy_response"] = denial_summary(&denied, &secrets);
        ensure!(
            matches!(&denied, LeaseResponse::Denied(LeaseError::InvalidSpec { reason }) if reason.contains("allow-list")),
            "actual image policy did not refuse the unlisted image"
        );
        report["secure_control_responses"] = json!(controls);
        ensure!(
            locked(&trace).containers.is_empty(),
            "secure negative controls must not allocate native workloads"
        );

        report["phase"] = "composed_sdk".into();
        let stranger = SandboxClient::from_client(client.clone()).with_api_key(unknown_key.clone());
        ensure!(
            matches!(
                stranger.lease(&lease, WAIT).await,
                Err(ClientError::Denied(LeaseError::Unauthorized { .. }))
            ),
            "the SDK must surface an unknown API key as an authorization denial"
        );
        let loss = Arc::new(Mutex::new(None));
        let observed = Arc::new(tokio::sync::Notify::new());
        let sdk = Arc::new(
            SandboxClient::from_bus(Box::new(LossBus {
                inner: NatsBus::new(client.clone()),
                loss: loss.clone(),
                observed: observed.clone(),
            }))
            .with_api_key(api_key.clone()),
        );
        let profile = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(profiles) = sdk.list_capabilities(Some(ProviderKind::LocalDocker)).await
                    && let Some(profile) = profiles.into_iter().next()
                {
                    break profile;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .context("the broker never listed the actual Docker worker capabilities")?;
        let isolation = &heartbeat.profile.isolation;
        report["advertised"] = json!({"named":profile.supports_named_processes,
            "cancellation":profile.supports_command_cancellation,
            "launch_cancellation":profile.supports_command_launch_cancellation});
        ensure!(
            profile.supports_named_processes == isolation.supports_named_processes
                && profile.supports_command_cancellation == isolation.supports_command_cancellation
                && profile.supports_command_launch_cancellation == isolation.supports_command_launch_cancellation,
            "SDK capabilities must match the actual emitted Docker heartbeat"
        );
        ensure!(
            profile.supports_named_processes.is_supported(),
            "actual worker has not earned named processes"
        );

        report["phase"] = "owned_docker_lease".into();
        let granted = sdk.lease(&lease, Duration::from_secs(180)).await?;
        ensure!(
            granted.created == LeaseCreation::Created
                && granted.provider == ProviderKind::LocalDocker
                && canonical(&granted.sandbox_id),
            "SDK grant lacks exact newly created Docker ownership"
        );
        grant = Some(granted.clone());
        let lease_id = granted.lease_id.clone();
        let scope = Scope {
            rpc: native_observer
                .as_ref()
                .map(|(_, rpc)| rpc)
                .context("native observer binding missing")?,
            lease: lease_id.as_str(),
            container: &granted.sandbox_id,
        };
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let root = format!("/tmp/named-cancellation-{nonce}");
        let prepared = sdk
            .run_command(
                &lease_id,
                vec!["mkdir".into(), "-p".into(), root.clone()],
                CommandOptions::default(),
            )
            .await?;
        ensure!(prepared.exit_code == 0, "the named family root could not be prepared");
        let sentinel = format!("sentinel-{nonce}");
        let target = format!("owned-{nonce}");
        let sentinel_paths = paths_for(&root, &sentinel);
        let target_paths = paths_for(&root, &target);

        report["phase"] = "ordinary_created_reused_native_controls".into();
        let (sentinel_id, sentinel_origin) = ordinary_launch(&sdk, &lease_id, &sentinel, &sentinel_paths).await?;
        let first_native = native_family(&scope, &sentinel_paths, &sentinel, &[]).await?;
        require_live(&first_native, &sentinel)?;
        let (reused_id, reused_origin) = ordinary_launch(&sdk, &lease_id, &sentinel, &sentinel_paths).await?;
        let sentinel_native = native_family(&scope, &sentinel_paths, &sentinel, &first_native.members).await?;
        require_live(&sentinel_native, &sentinel)?;
        require_same_family(&first_native, &sentinel_native)?;
        ensure!(
            sentinel_origin == CommandLaunchOrigin::Created
                && reused_origin == CommandLaunchOrigin::Reused
                && sentinel_id == reused_id,
            "ordinary named origin control failed"
        );
        ensure!(
            matches!(sdk.poll_command(&lease_id, &sentinel_id).await?, PollCommandResponse::Running),
            "ordinary named sentinel is not running"
        );

        report["phase"] = "actual_named_provider_strong_cancellation".into();
        ensure!(
            profile.supports_command_cancellation.is_supported()
                && profile.supports_command_launch_cancellation.is_supported(),
            "actual named provider has not earned strong cancellation"
        );
        let mut cases = Vec::new();
        for (case, fresh) in [("fresh_named_lost_started", true), ("reused_named_lost_started", false)] {
            report["phase"] = case.into();
            let name = if fresh { &target } else { &sentinel };
            let paths = if fresh { &target_paths } else { &sentinel_paths };
            *locked(&loss) = Some(Loss {
                process_name: name.clone(),
                ..Default::default()
            });
            let launch = named_launch(&lease_id, name, paths, true)?;
            let launch_key = launch.launch_key.clone().context("strong launch omitted its launch key")?;
            let waiting_sdk = sdk.clone();
            let mut waiting = tokio::spawn(async move { waiting_sdk.send_run_command(launch).await });
            let early = tokio::time::timeout(Duration::from_secs(20), async {
                tokio::select! {
                    () = observed.notified() => None,
                    finished = &mut waiting => Some(finished),
                }
            })
            .await
            .context("the named Started reply was never observed")?;
            if let Some(finished) = early {
                anyhow::bail!("named launch returned before its Started reply was lost: {finished:?}");
            }
            let (execution, origin, suppressed, recorded_key) = {
                let state = locked(&loss);
                let active = state.as_ref().context("named loss state vanished")?;
                (
                    active.execution_id.clone().context("lost Started omitted its execution")?,
                    active.origin.context("lost Started omitted its origin")?,
                    active.suppressed,
                    active.launch_key.clone(),
                )
            };
            ensure!(
                suppressed == 1 && recorded_key.as_ref() == Some(&launch_key),
                "actual named Started frame was not lost exactly once"
            );
            ensure!(
                origin
                    == if fresh {
                        CommandLaunchOrigin::Created
                    } else {
                        CommandLaunchOrigin::Reused
                    },
                "actual named Started origin is incorrect"
            );
            ensure!(
                (execution == sentinel_id) != fresh,
                "named Started execution ownership is incorrect"
            );
            let before = native_family(
                &scope,
                paths,
                name,
                if fresh { &[] } else { &sentinel_native.members },
            )
            .await?;
            require_live(&before, name)?;
            waiting.abort();
            let _ = waiting.await;
            *locked(&loss) = None;
            let cancel = CancelCommandLaunchRequest {
                lease_id: lease_id.clone(),
                launch_key,
            };
            let outcome = settled_cancel(&sdk, &cancel).await?;
            if fresh {
                ensure!(
                    outcome
                        == CancelCommandLaunchResponse::Canceled {
                            execution_id: Some(execution.clone())
                        },
                    "fresh named launch was not canceled as its own execution: {outcome:?}"
                );
                let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
                let target_native = loop {
                    let state = native_family(&scope, paths, name, &before.members).await?;
                    if family_gone(&state) || tokio::time::Instant::now() >= deadline {
                        break state;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                };
                ensure!(family_gone(&target_native), "fresh named native family survived cancellation");
                ensure!(target_native.starts == [name.as_str()], "fresh named user effect replayed");
                ensure!(
                    matches!(sdk.poll_command(&lease_id, &execution).await?, PollCommandResponse::Completed(_)),
                    "fresh named execution is not completed"
                );
                let repeated = sdk.cancel_command_launch(cancel).await?;
                ensure!(repeated == outcome, "fresh named cancel receipt is not replayable");
            } else {
                ensure!(
                    outcome
                        == CancelCommandLaunchResponse::Preserved {
                            execution_id: sentinel_id.clone()
                        },
                    "reused named launch did not report preservation: {outcome:?}"
                );
                let target_native = native_family(&scope, &sentinel_paths, &sentinel, &sentinel_native.members).await?;
                require_live(&target_native, &sentinel)?;
            }
            let after = native_family(&scope, &sentinel_paths, &sentinel, &sentinel_native.members).await?;
            require_live(&after, &sentinel)?;
            require_same_family(&sentinel_native, &after)?;
            ensure!(
                matches!(sdk.poll_command(&lease_id, &sentinel_id).await?, PollCommandResponse::Running),
                "unrelated native sentinel was canceled"
            );
            cases.push(json!({"case":case,"execution_id":execution.as_str(),"outcome":outcome}));
        }
        report["cases"] = json!(cases);

        report["phase"] = "product_delete".into();
        let observed_record = tokio::time::timeout(WAIT, async {
            loop {
                if let Some(record) = locked(&trace).containers.get(&granted.sandbox_id).cloned()
                    && record.lease == lease_id.as_str()
                    && record.role == WORKLOAD_ROLE
                    && record.image_verified
                    && record.isolation_verified
                    && record.artifact_verified
                {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .context("exact live owner/lease/image/isolation/artifact control did not settle")?;
        report["native_record"] = serde_json::to_value(&observed_record)?;
        sdk.delete_lease(tenant.clone(), &lease_id).await?;
        tokio::time::timeout(WAIT, async {
            loop {
                if container_absent(&docker, &granted.sandbox_id).await? {
                    break anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .context("SDK product delete did not prove exact owned native absence")??;
        product_cleanup = true;
        report["phase"] = "complete".into();
        anyhow::Ok(())
    })
    .catch_unwind()
    .await;

    report["primary_phase"] = report["phase"].clone();
    match &operation {
        Ok(Ok(())) => report["primary_outcome"] = "passed".into(),
        Ok(Err(error)) => {
            report["primary_outcome"] = "failed".into();
            report["primary_error"] = redact(format!("{error:#}").as_bytes(), &redaction_secrets).into();
        }
        Err(_) => report["primary_outcome"] = "panicked".into(),
    }
    let mut cleanup_errors = Vec::new();
    if let Some((host, rpc)) = native_observer.as_mut() {
        if rpc.shutdown().await.is_err() {
            cleanup_errors.push("native_observer_rpc_did_not_stop");
        }
        if trusted_observer_support::Host::cleanup(host.as_ref()).await.is_err() {
            cleanup_errors.push("native_observer_teardown_failed");
        }
        let containers = host.containers().await;
        let mut absent = true;
        for id in &containers {
            absent &= container_absent(&docker, id).await.unwrap_or(false);
        }
        report["native_observer_containers"] = json!(containers);
        if !absent {
            cleanup_errors.push("native_observer_exact_absence_unconfirmed");
        }
    }
    if grant.is_some() && !product_cleanup {
        report["fallback_used"] = true.into();
        cleanup_errors.push("product_delete_unconfirmed_before_worker_teardown");
    }
    for child in children.iter_mut().rev() {
        if child.stop().await.is_err() {
            cleanup_errors.push("owned_child_teardown_failed");
        }
    }
    report["child_diagnostics"] = json!(children.iter().map(ManagedChild::diagnostics).collect::<Vec<_>>());
    if let Some((trace, cancel, mut task)) = native {
        cancel.cancel();
        if !matches!(tokio::time::timeout(WAIT, &mut task).await, Ok(Ok(()))) {
            task.abort();
            let _ = task.await;
            cleanup_errors.push("native_provenance_observer_did_not_stop");
        }
        let trace = locked(&trace).clone();
        report["native_allocation_receipts"] = serde_json::to_value(&trace)?;
        if !trace.errors.is_empty() {
            cleanup_errors.push("native_provenance_incomplete");
        }
        for (id, evidence) in &trace.containers {
            if !container_absent(&docker, id).await.unwrap_or(false) {
                report["fallback_used"] = true.into();
                cleanup_errors.push("product_native_cleanup_missing");
                let current = docker.inspect_container(id, None).await;
                if current
                    .as_ref()
                    .ok()
                    .and_then(|value| value.config.as_ref())
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(|labels| {
                        labels.get(DOCKER_LABEL_OWNER) == Some(&owner)
                            && labels.get(DOCKER_LABEL_LEASE_ID) == Some(&evidence.lease)
                    })
                {
                    remove_container(&docker, id).await;
                }
            }
            if !container_absent(&docker, id).await.unwrap_or(false) {
                cleanup_errors.push("exact_native_absence_unknown");
            }
        }
        let supervisor_digest = report["supervisor_sha256"].as_str().map(str::to_owned);
        for id in &trace.runtime_writers {
            if !container_absent(&docker, id).await.unwrap_or(false) {
                report["fallback_used"] = true.into();
                cleanup_errors.push("runtime_seed_writer_cleanup_missing");
                if docker.inspect_container(id, None).await.ok().is_some_and(|container| {
                    container.id.as_deref() == Some(id.as_str())
                        && container
                            .config
                            .as_ref()
                            .and_then(|config| config.labels.as_ref())
                            .is_some_and(|labels| {
                                labels.get(DOCKER_LABEL_OWNER) == Some(&owner)
                                    && labels.get(RUNTIME_DIGEST).map(String::as_str) == supervisor_digest.as_deref()
                            })
                }) {
                    remove_container(&docker, id).await;
                }
            }
            if !container_absent(&docker, id).await.unwrap_or(false) {
                cleanup_errors.push("runtime_seed_writer_absence_unknown");
            }
        }
        for name in &trace.runtime_volumes {
            if !volume_absent(&docker, name).await.unwrap_or(false) {
                report["fallback_used"] = true.into();
                cleanup_errors.push("worker_runtime_artifact_cleanup_missing");
                if docker.inspect_volume(name).await.ok().is_some_and(|volume| {
                    volume.name == *name
                        && volume.labels.get(DOCKER_LABEL_OWNER) == Some(&owner)
                        && volume.labels.get(RUNTIME_DIGEST).map(String::as_str) == supervisor_digest.as_deref()
                }) {
                    let _ = docker
                        .remove_volume(name, Some(RemoveVolumeOptionsBuilder::new().force(false).build()))
                        .await;
                }
            }
            if !volume_absent(&docker, name).await.unwrap_or(false) {
                cleanup_errors.push("runtime_artifact_absence_unknown");
            }
        }
    }
    if let Some(id) = spice_id {
        let stopped = matches!(
            output(Command::new("docker").args(["stop", "--time", "5", &id]), None).await,
            Ok(result) if result.status.success()
        );
        if !stopped {
            cleanup_errors.push("spicedb_stop_failed");
        }
        let absent = if stopped {
            matches!(
                tokio::time::timeout(WAIT, async {
                    loop {
                        if container_absent(&docker, &id).await? {
                            break anyhow::Ok(());
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await,
                Ok(Ok(()))
            )
        } else {
            container_absent(&docker, &id).await.unwrap_or(false)
        };
        if !absent {
            report["fallback_used"] = true.into();
            cleanup_errors.push("spicedb_required_fallback");
            remove_container(&docker, &id).await;
        }
        if !container_absent(&docker, &id).await.unwrap_or(false) {
            cleanup_errors.push("spicedb_absence_unknown");
        }
    }
    report["cleanup_errors"] = json!(cleanup_errors);
    eprintln!("secure composed Docker receipt: {report}");
    match operation {
        Ok(Ok(())) => {
            ensure!(
                cleanup_errors.is_empty(),
                "secure composed Docker fixture cleanup failed: {cleanup_errors:?}"
            );
            Ok(())
        }
        Ok(Err(_)) => anyhow::bail!(
            "secure composed Docker failed at {}: {}; cleanup_errors={cleanup_errors:?}",
            report["primary_phase"],
            report["primary_error"]
        ),
        Err(_) => anyhow::bail!(
            "secure composed Docker panicked at {}; cleanup_errors={cleanup_errors:?}",
            report["primary_phase"]
        ),
    }
}
