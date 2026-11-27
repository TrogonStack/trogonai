// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::collections::{BTreeSet, HashMap};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, ensure};
use bollard::Docker;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Reads the bound family records and the kernel census from inside the
/// workload PID namespace, and proves the reader identity and privilege.
pub const PROBE: &str = r#"const fs = require('node:fs');
const request = JSON.parse(process.argv[1]);
const readKernel = (pid) => {
  try {
    const value = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
    const fields = value.slice(value.lastIndexOf(')') + 1).trim().split(/\s+/);
    return { pid: Number(pid), startTime: fields[19], ppid: Number(fields[1]) };
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ESRCH') return undefined;
    throw error;
  }
};
const readStat = (pid) => {
  const value = readKernel(pid);
  return value && { pid: value.pid, startTime: value.startTime };
};
const readRecord = (path, limit) => {
  const descriptor = fs.openSync(path, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  try {
    const stat = fs.fstatSync(descriptor);
    if (!stat.isFile() || stat.size > limit) throw Error('native_record_bound_exceeded');
    const bytes = Buffer.alloc(limit + 1);
    let length = 0;
    while (length < bytes.length) {
      const count = fs.readSync(descriptor, bytes, length, bytes.length - length, null);
      if (count === 0) break;
      length += count;
    }
    if (length > limit) throw Error('native_record_bound_exceeded');
    return bytes.subarray(0, length).toString();
  } finally { fs.closeSync(descriptor); }
};
const paths = request.paths.map((path) => `/proc/1/root${path}`);
const ready = paths.slice(0, 2).every((path) => fs.existsSync(path));
const records = ready ? paths.slice(0, 2).map((path) => JSON.parse(readRecord(path, 65536))) : [];
const pids = fs.readdirSync('/proc').filter((value) => /^\d+$/.test(value));
if (pids.length > 4096) throw Error('native_process_census_exceeded');
const members = [];
for (const pid of pids) {
  let environment;
  try {
    environment = fs.readFileSync(`/proc/${pid}/environ`).toString().split('\0');
  } catch (error) {
    if (error.code === 'ENOENT' || error.code === 'ESRCH') continue;
    throw error;
  }
  if (environment.includes(`SANDBOX_ACCEPTANCE_FAMILY=${request.marker}`)) {
    const identity = readStat(pid);
    if (identity) members.push(identity);
  }
}
const pinned = new Map([...records, ...request.tracked].map((identity) => [`${identity.pid}:${identity.startTime}`, identity]));
const tracked = [...pinned.values()].map((identity) => {
  const observed = readStat(identity.pid);
  return { pid: identity.pid, startTime: identity.startTime, state: observed && observed.startTime === identity.startTime ? 'alive' : 'absent' };
});
const starts = fs.existsSync(paths[2]) ? readRecord(paths[2], 1048576).trim().split('\n').filter(Boolean) : [];
const status = fs.readFileSync('/proc/self/status', 'utf8');
const caps = BigInt(`0x${/^CapEff:\s*([a-f0-9]+)$/m.exec(status)[1]}`);
const root = records[0] && readKernel(records[0].pid);
const child = records[1] && readKernel(records[1].pid);
const rootChildLinked = Boolean(root && child && root.startTime === records[0].startTime && child.startTime === records[1].startTime && child.ppid === root.pid);
process.stdout.write(JSON.stringify({
  native: { ready, records, members, tracked, starts },
  proof: { observer: readStat(process.pid), init: readStat(1), uid: process.getuid(), ptrace: (caps & (1n << 19n)) !== 0n, rootChildLinked },
}));
"#;
pub const STREAM_LIMIT: usize = 1024 * 1024;
pub const INPUT_LIMIT: usize = 512 * 1024;
pub const EXEC_BOUND: Duration = Duration::from_secs(5);
const OBSERVER_MEMORY: i64 = 256 * 1024 * 1024;
const OBSERVER_PIDS: i64 = 64;
const OWNER: &str = "sandbox.trogon.live/owner";
const LEASE: &str = "sandbox.trogon.live/lease-id";
const OBSERVER_OWNER: &str = "sandbox.test.observer/owner";

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

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Identity {
    pub pid: u32,
    pub start_time: String,
}

impl Identity {
    fn valid(&self) -> bool {
        self.pid > 0
            && !self.start_time.is_empty()
            && self.start_time.len() <= 20
            && self.start_time.bytes().all(|byte| byte.is_ascii_digit())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarkedIdentity {
    pub pid: u32,
    pub start_time: String,
    pub marker: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tracked {
    pub pid: u32,
    pub start_time: String,
    pub state: TrackedState,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackedState {
    Alive,
    Absent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Native {
    pub ready: bool,
    pub records: Vec<MarkedIdentity>,
    pub members: Vec<Identity>,
    pub tracked: Vec<Tracked>,
    pub starts: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Proof {
    pub observer: Identity,
    pub init: Identity,
    pub uid: u32,
    pub ptrace: bool,
    pub root_child_linked: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    native: Native,
    proof: Proof,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub kind: Operation,
    pub request_id: String,
    pub lease_id: String,
    pub container_id: String,
    pub marker: String,
    pub paths: [String; 3],
    pub tracked: Vec<Identity>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    NativeFamily,
}

impl Request {
    pub fn validate_input(&self) -> Result<&str, Refusal> {
        if self.request_id.is_empty()
            || self.request_id.len() > 64
            || !self
                .request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            || !canonical(&self.container_id)
            || self.lease_id.is_empty()
            || self.lease_id.len() > 128
            || !self
                .lease_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            || self.tracked.len() > 4096
            || self.tracked.iter().any(|identity| !identity.valid())
        {
            return Err(Refusal::InvalidInput);
        }
        let nonce = self
            .marker
            .strip_prefix("sentinel-")
            .or_else(|| self.marker.strip_prefix("owned-"))
            .ok_or(Refusal::InvalidInput)?;
        if nonce.len() != 32 || !nonce.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Refusal::InvalidInput);
        }
        let root = format!("/tmp/named-cancellation-{nonce}/{}", self.marker);
        if self.paths
            != [
                format!("{root}.root.json"),
                format!("{root}.child.json"),
                format!("{root}.starts"),
            ]
        {
            return Err(Refusal::RequestScope);
        }
        Ok(nonce)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Observed {
        request_id: String,
        native: Native,
        proof: Proof,
        observer_container: String,
        observer_absent: bool,
    },
    Refused {
        request_id: String,
        code: Refusal,
    },
    Failed {
        request_id: String,
        code: Failure,
    },
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    InvalidInput,
    RequestScope,
    NamespaceChanged,
    NativeBindingChanged,
    ObserverChanged,
}

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    Deadline,
    AccessDenied,
    OperationNotPermitted,
    ProbeFailed,
    OutputInvalid,
    Transport,
}

fn canonical(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn capability_set(values: &[String]) -> BTreeSet<String> {
    values
        .iter()
        .map(|value| value.trim().to_ascii_uppercase().trim_start_matches("CAP_").to_owned())
        .collect()
}

#[derive(Clone)]
pub struct Binding {
    pub container: String,
    pub lease: String,
    pub owner: String,
    pub image: String,
    init_pid: i64,
    started_at: String,
    retained_capabilities: BTreeSet<String>,
}

impl Binding {
    pub async fn seal(
        docker: &Docker,
        container: String,
        lease: String,
        owner: String,
        image: String,
    ) -> anyhow::Result<Self> {
        Self::seal_with_retained_capabilities(docker, container, lease, owner, image, vec![]).await
    }

    pub async fn seal_with_retained_capabilities(
        docker: &Docker,
        container: String,
        lease: String,
        owner: String,
        image: String,
        retained_capabilities: Vec<String>,
    ) -> anyhow::Result<Self> {
        ensure!(
            canonical(&container) && image.starts_with("sha256:") && canonical(&image[7..]),
            "observer input must be canonical and immutable"
        );
        let retained_capabilities = capability_set(&retained_capabilities);
        ensure!(
            retained_capabilities
                .iter()
                .all(|capability| !matches!(capability.as_str(), "" | "ALL" | "SYS_PTRACE" | "SYS_ADMIN")),
            "observer workload capability profile exceeds its isolation boundary"
        );
        let inspected = docker.inspect_container(&container, None).await?;
        let state = inspected.state.context("owned native state absent")?;
        let binding = Self {
            container,
            lease,
            owner,
            image,
            init_pid: state.pid.filter(|pid| *pid > 0).context("owned init PID absent")?,
            started_at: state.started_at.context("owned init StartedAt absent")?.to_string(),
            retained_capabilities,
        };
        binding
            .verify(docker)
            .await
            .map_err(|_| anyhow::anyhow!("owned namespace binding could not be sealed"))?;
        Ok(binding)
    }

    async fn verify(&self, docker: &Docker) -> Result<(), Refusal> {
        let found = docker
            .inspect_container(&self.container, None)
            .await
            .map_err(|_| Refusal::NativeBindingChanged)?;
        let state = found.state.as_ref().ok_or(Refusal::NativeBindingChanged)?;
        if state.pid != Some(self.init_pid)
            || state.started_at.as_ref().map(ToString::to_string).as_deref() != Some(&self.started_at)
        {
            return Err(Refusal::NamespaceChanged);
        }
        let labels = found
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .ok_or(Refusal::NativeBindingChanged)?;
        let host = found.host_config.as_ref().ok_or(Refusal::NativeBindingChanged)?;
        if found.id.as_deref() != Some(&self.container)
            || found.image.as_deref() != Some(&self.image)
            || state.running != Some(true)
            || labels.get(OWNER) != Some(&self.owner)
            || labels.get(LEASE) != Some(&self.lease)
            || host.privileged == Some(true)
            || capability_set(host.cap_drop.as_deref().unwrap_or_default()) != BTreeSet::from(["ALL".to_owned()])
            || capability_set(host.cap_add.as_deref().unwrap_or_default()) != self.retained_capabilities
        {
            return Err(Refusal::NativeBindingChanged);
        }
        Ok(())
    }
}

pub struct Captured {
    pub exit: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

pub async fn capture(
    docker: &Docker,
    container: &str,
    command: Vec<String>,
    privileged: bool,
) -> anyhow::Result<Captured> {
    let exec = docker
        .create_exec(
            container,
            CreateExecOptions {
                cmd: Some(command),
                user: Some("0".to_owned()),
                privileged: Some(privileged),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                ..Default::default()
            },
        )
        .await?;
    let result = tokio::time::timeout(EXEC_BOUND, async {
        let StartExecResults::Attached { mut output, mut input } =
            docker.start_exec(&exec.id, None::<StartExecOptions>).await?
        else {
            anyhow::bail!("readonly observer exec unexpectedly detached");
        };
        input.shutdown().await?;
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        while let Some(frame) = output.next().await {
            let frame = frame?;
            let destination = match frame {
                bollard::container::LogOutput::StdOut { .. } => &mut stdout,
                bollard::container::LogOutput::StdErr { .. } => &mut stderr,
                _ => anyhow::bail!("readonly observer stream type changed"),
            };
            ensure!(
                destination.len().saturating_add(frame.as_ref().len()) <= STREAM_LIMIT,
                "readonly observer output exceeded its original retention bound"
            );
            destination.extend_from_slice(frame.as_ref());
        }
        let inspected = docker.inspect_exec(&exec.id).await?;
        ensure!(inspected.running == Some(false), "readonly observer exec did not stop");
        Ok(Captured {
            exit: inspected.exit_code.context("readonly observer exit absent")?,
            stdout,
            stderr,
        })
    })
    .await;
    result.context("readonly observer deadline")?
}

pub struct Observer {
    docker: Docker,
    binding: Binding,
    active: Mutex<Option<String>>,
    containers: Mutex<Vec<String>>,
    image: String,
    fixture_owner: String,
    nonce: Mutex<Option<String>>,
    gate: Mutex<()>,
    executions: AtomicUsize,
}

impl Observer {
    pub async fn create(
        docker: Docker,
        binding: Binding,
        image: String,
        fixture_owner: String,
    ) -> anyhow::Result<Arc<Self>> {
        binding
            .verify(&docker)
            .await
            .map_err(|_| anyhow::anyhow!("observer target binding changed"))?;
        ensure!(
            image.starts_with("sha256:") && canonical(&image[7..]),
            "observer interpreter image must be immutable"
        );
        Ok(Arc::new(Self {
            docker,
            binding,
            active: Mutex::new(None),
            containers: Mutex::new(Vec::new()),
            image,
            fixture_owner,
            nonce: Mutex::new(None),
            gate: Mutex::new(()),
            executions: AtomicUsize::new(0),
        }))
    }

    async fn create_probe(&self) -> anyhow::Result<String> {
        let created = self
            .docker
            .create_container(
                Some(
                    CreateContainerOptionsBuilder::new()
                        .name(&format!("trusted-observer-{}", fresh_id()))
                        .build(),
                ),
                ContainerCreateBody {
                    image: Some(self.image.clone()),
                    user: Some("0".into()),
                    entrypoint: Some(vec!["node".into()]),
                    cmd: Some(vec!["-e".into(), "setInterval(()=>{},1000)".into()]),
                    labels: Some(HashMap::from([
                        (OBSERVER_OWNER.into(), self.fixture_owner.clone()),
                        ("sandbox.test.observer/workload".into(), self.binding.container.clone()),
                        ("sandbox.test.observer/lease".into(), self.binding.lease.clone()),
                    ])),
                    host_config: Some(HostConfig {
                        readonly_rootfs: Some(true),
                        network_mode: Some("none".into()),
                        cap_drop: Some(vec!["ALL".into()]),
                        pid_mode: Some(format!("container:{}", self.binding.container)),
                        memory: Some(OBSERVER_MEMORY),
                        memory_swap: Some(OBSERVER_MEMORY),
                        pids_limit: Some(OBSERVER_PIDS),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await?;
        ensure!(
            canonical(&created.id),
            "observer create did not return a canonical identity"
        );
        *self.active.lock().await = Some(created.id.clone());
        self.containers.lock().await.push(created.id.clone());
        if let Err(error) = self.docker.start_container(&created.id, None).await {
            self.cleanup().await?;
            return Err(error.into());
        }
        Ok(created.id)
    }

    pub async fn containers(&self) -> Vec<String> {
        self.containers.lock().await.clone()
    }
    pub fn executions(&self) -> usize {
        self.executions.load(Ordering::Acquire)
    }

    async fn verify_observer(&self, container: &str) -> Result<(), Refusal> {
        let found = self
            .docker
            .inspect_container(container, None)
            .await
            .map_err(|_| Refusal::ObserverChanged)?;
        let host = found.host_config.as_ref().ok_or(Refusal::ObserverChanged)?;
        let labels = found
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .ok_or(Refusal::ObserverChanged)?;
        if found.id.as_deref() != Some(container)
            || found.image.as_deref() != Some(&self.image)
            || found.state.as_ref().and_then(|state| state.running) != Some(true)
            || labels.get(OBSERVER_OWNER) != Some(&self.fixture_owner)
            || labels.get("sandbox.test.observer/workload") != Some(&self.binding.container)
            || labels.get("sandbox.test.observer/lease") != Some(&self.binding.lease)
            || host.pid_mode.as_deref() != Some(&format!("container:{}", self.binding.container))
            || host.readonly_rootfs != Some(true)
            || host.network_mode.as_deref() != Some("none")
            || host.memory != Some(OBSERVER_MEMORY)
            || host.memory_swap != Some(OBSERVER_MEMORY)
            || host.pids_limit != Some(OBSERVER_PIDS)
            || host.privileged == Some(true)
            || host.cap_drop.as_ref() != Some(&vec!["ALL".to_owned()])
            || host.cap_add.as_ref().is_some_and(|caps| !caps.is_empty())
            || found.mounts.as_ref().is_some_and(|mounts| !mounts.is_empty())
        {
            return Err(Refusal::ObserverChanged);
        }
        Ok(())
    }

    async fn validate_request(&self, request: &Request) -> Result<(), Refusal> {
        let nonce = request.validate_input()?;
        if request.container_id != self.binding.container || request.lease_id != self.binding.lease {
            return Err(Refusal::RequestScope);
        }
        let mut held = self.nonce.lock().await;
        if held.as_deref().is_some_and(|held| held != nonce) {
            return Err(Refusal::RequestScope);
        }
        *held = Some(nonce.to_owned());
        Ok(())
    }

    pub async fn observe(&self, request: Request) -> Response {
        let id = request
            .request_id
            .chars()
            .filter(|value| value.is_ascii_alphanumeric() || matches!(value, '-' | '_'))
            .take(64)
            .collect::<String>();
        let _gate = self.gate.lock().await;
        for validation in [
            self.validate_request(&request).await,
            self.binding.verify(&self.docker).await,
        ] {
            if let Err(code) = validation {
                return Response::Refused { request_id: id, code };
            }
        }
        let container = match self.create_probe().await {
            Ok(container) => container,
            Err(_) => {
                return Response::Failed {
                    request_id: id,
                    code: Failure::Transport,
                };
            }
        };
        let result = self.observe_probe(&request, &id, &container).await;
        if self.cleanup().await.is_err() {
            return Response::Failed {
                request_id: id,
                code: Failure::Transport,
            };
        }
        if let Err(code) = self.binding.verify(&self.docker).await {
            return Response::Refused { request_id: id, code };
        }
        result
    }

    async fn observe_probe(&self, request: &Request, id: &str, container: &str) -> Response {
        if let Err(code) = self.verify_observer(container).await {
            return Response::Refused {
                request_id: id.into(),
                code,
            };
        }
        self.executions.fetch_add(1, Ordering::AcqRel);
        let input = serde_json::json!({"paths":request.paths,"marker":request.marker,"tracked":request.tracked});
        let result = capture(
            &self.docker,
            container,
            vec!["node".into(), "-e".into(), PROBE.into(), input.to_string()],
            true,
        )
        .await;
        let captured = match result {
            Ok(captured) => captured,
            Err(_) => {
                return Response::Failed {
                    request_id: id.into(),
                    code: Failure::Deadline,
                };
            }
        };
        for validation in [
            self.binding.verify(&self.docker).await,
            self.verify_observer(container).await,
        ] {
            if let Err(code) = validation {
                return Response::Refused {
                    request_id: id.into(),
                    code,
                };
            }
        }
        if captured.exit != 0 {
            let stderr = String::from_utf8_lossy(&captured.stderr);
            let code = if stderr.contains("EACCES") {
                Failure::AccessDenied
            } else if stderr.contains("EPERM") {
                Failure::OperationNotPermitted
            } else {
                Failure::ProbeFailed
            };
            return Response::Failed {
                request_id: id.into(),
                code,
            };
        }
        let envelope = serde_json::from_slice::<Envelope>(&captured.stdout);
        let Ok(envelope) = envelope else {
            return Response::Failed {
                request_id: id.into(),
                code: Failure::OutputInvalid,
            };
        };
        if !valid_output(&envelope, &request.marker) {
            return Response::Failed {
                request_id: id.into(),
                code: Failure::OutputInvalid,
            };
        }
        Response::Observed {
            request_id: id.into(),
            native: envelope.native,
            proof: envelope.proof,
            observer_container: container.into(),
            observer_absent: true,
        }
    }

    pub async fn cleanup(&self) -> anyhow::Result<()> {
        let mut active = self.active.lock().await;
        let Some(container) = active.as_ref() else {
            return Ok(());
        };
        let found = match self.docker.inspect_container(container, None).await {
            Ok(found) => found,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                *active = None;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        ensure!(
            found.id.as_deref() == Some(container.as_str())
                && found
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(|labels| labels.get(OBSERVER_OWNER) == Some(&self.fixture_owner)
                        && labels.get("sandbox.test.observer/workload") == Some(&self.binding.container)
                        && labels.get("sandbox.test.observer/lease") == Some(&self.binding.lease)),
            "observer teardown ownership changed"
        );
        self.docker
            .remove_container(
                container,
                Some(RemoveContainerOptionsBuilder::new().force(true).v(true).build()),
            )
            .await?;
        ensure!(
            absent(&self.docker, container).await?,
            "observer exact absence unconfirmed"
        );
        *active = None;
        Ok(())
    }
}

fn valid_output(envelope: &Envelope, marker: &str) -> bool {
    let native = &envelope.native;
    native.records.len() <= 2
        && native.members.len() <= 4096
        && native.tracked.len() <= 4096
        && native.starts.len() <= 4096
        && native.records.iter().all(|value| {
            value.marker == marker
                && Identity {
                    pid: value.pid,
                    start_time: value.start_time.clone(),
                }
                .valid()
        })
        && native.members.iter().all(Identity::valid)
        && native.tracked.iter().all(|value| {
            Identity {
                pid: value.pid,
                start_time: value.start_time.clone(),
            }
            .valid()
        })
        && native.starts.iter().all(|value| value == marker)
        && envelope.proof.observer.valid()
        && envelope.proof.init.valid()
        && envelope.proof.init.pid == 1
        && envelope.proof.uid == 0
        && envelope.proof.ptrace
}

pub async fn absent(docker: &Docker, container: &str) -> anyhow::Result<bool> {
    ensure!(canonical(container), "native absence needs canonical identity");
    match docker.inspect_container(container, None).await {
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => Ok(true),
        Ok(found) if found.id.as_deref() == Some(container) => Ok(false),
        _ => anyhow::bail!("exact native absence remains unknown"),
    }
}

pub struct Rpc {
    _directory: tempfile::TempDir,
    pub path: PathBuf,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

#[async_trait::async_trait]
pub trait Host: Send + Sync {
    async fn observe(&self, request: Request) -> Response;
    async fn cleanup(&self) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl Host for Observer {
    async fn observe(&self, request: Request) -> Response {
        Observer::observe(self, request).await
    }
    async fn cleanup(&self) -> anyhow::Result<()> {
        Observer::cleanup(self).await
    }
}

impl Rpc {
    pub async fn start(observer: Arc<Observer>) -> anyhow::Result<Self> {
        Self::start_host(observer).await
    }

    pub async fn start_host(observer: Arc<dyn Host>) -> anyhow::Result<Self> {
        let directory = tempfile::Builder::new().prefix("native-observer-").tempdir_in("/tmp")?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let path = directory.path().join("oracle.sock");
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let stop = CancellationToken::new();
        let task = tokio::spawn({
            let stop = stop.clone();
            async move {
                loop {
                    let accepted = tokio::select! { _ = stop.cancelled() => break, value = listener.accept() => value };
                    let Ok((mut stream, _)) = accepted else {
                        break;
                    };
                    let exchange = async {
                        let mut bytes = Vec::new();
                        loop {
                            let mut chunk = [0; 4096];
                            let count = stream.read(&mut chunk).await?;
                            if count == 0 {
                                break;
                            }
                            ensure!(
                                bytes.len().saturating_add(count) <= INPUT_LIMIT,
                                "observer request bound exceeded"
                            );
                            bytes.extend_from_slice(&chunk[..count]);
                        }
                        let response = match serde_json::from_slice::<Request>(&bytes) {
                            Ok(request) => observer.observe(request).await,
                            Err(_) => Response::Refused {
                                request_id: String::new(),
                                code: Refusal::InvalidInput,
                            },
                        };
                        let bytes = serde_json::to_vec(&response)?;
                        ensure!(bytes.len() <= STREAM_LIMIT, "observer RPC output bound exceeded");
                        stream.write_all(&bytes).await?;
                        stream.shutdown().await?;
                        Ok::<_, anyhow::Error>(())
                    };
                    let result = tokio::select! {
                        _ = stop.cancelled() => { let _ = observer.cleanup().await; break; },
                        result = tokio::time::timeout(EXEC_BOUND, exchange) => result,
                    };
                    if result.is_err() {
                        let _ = observer.cleanup().await;
                        break;
                    }
                }
            }
        });
        Ok(Self {
            _directory: directory,
            path,
            stop,
            task: Some(task),
        })
    }

    pub async fn call(&self, request: &Request) -> anyhow::Result<Response> {
        self.call_bytes(&serde_json::to_vec(request)?).await
    }

    pub async fn call_bytes(&self, bytes: &[u8]) -> anyhow::Result<Response> {
        tokio::time::timeout(EXEC_BOUND, async {
            let mut stream = UnixStream::connect(&self.path).await?;
            stream.write_all(bytes).await?;
            stream.shutdown().await?;
            let mut response = Vec::new();
            (&mut stream)
                .take((STREAM_LIMIT + 1) as u64)
                .read_to_end(&mut response)
                .await?;
            ensure!(
                response.len() <= STREAM_LIMIT,
                "observer RPC response exceeded original output bound"
            );
            Ok::<_, anyhow::Error>(serde_json::from_slice(&response)?)
        })
        .await
        .context("observer RPC exceeded original command bound")?
    }

    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(EXEC_BOUND, task).await??;
        }
        Ok(())
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
