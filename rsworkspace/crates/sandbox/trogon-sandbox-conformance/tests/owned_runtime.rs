//! Runs the suite against a docker or subprocess worker this machine owns, then proves every native resource the
//! worker granted is gone without deleting anything as a fallback.
//!
//! Both tests are ignored and need `SANDBOX_LOCAL_CONFORMANCE=1` plus a worker already attached to the NATS server
//! `SANDBOX_CONFORMANCE_NATS_URL` names, speaking the legacy dispatch protocol. The docker run first proves the
//! cached image can tell a closed port, an open listener and an unrestricted external endpoint apart, so a denial is
//! evidence and not a broken probe. The subprocess run needs `SANDBOX_CONFORMANCE_WORKER_PID`, the worker's process,
//! so a granted PID can be checked for absence in the same PID scope.
//!
//! ```text
//! SANDBOX_LOCAL_CONFORMANCE=1 SANDBOX_CONFORMANCE_NATS_URL=nats://127.0.0.1:4222 \
//!   cargo test -p trogon-sandbox-conformance --test owned_runtime -- --ignored owned_docker
//! ```

#![cfg(unix)]

mod support;

use std::collections::BTreeSet;
use std::process::Output;
use std::time::Duration;

use support::cleanup::{observe_worker, run_with_exact_cleanup};
use support::wire::Sendable;
use tokio::process::Command;
use trogon_sandbox_conformance::{ExecModel, GrantLedger, Target};
use trogon_sandbox_contract::{
    BootMode, OciImageRef, ProviderKind, SandboxId, SandboxImage, SandboxSpec, WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_std::env::{ReadEnv, SystemEnv};

const ENABLE: &str = "SANDBOX_LOCAL_CONFORMANCE";
const WORKER_PID: &str = "SANDBOX_CONFORMANCE_WORKER_PID";
const WORKER_BINARY: &str = "trogon-sandbox-subprocess-worker";
const PROBE_PASSED: &[u8] = b"probe-controls-passed";

/// Proves the image's probe tools tell a closed port, an open listener and an unrestricted endpoint apart.
const EGRESS_CONTROLS: &str = r#"set -eu
for tool in sh nc find tar; do command -v "$tool" >/dev/null || exit 42; done
closed_code=0
closed_output=$(nc -z -w 1 127.0.0.1 39997 2>&1) || closed_code=$?
[ "$closed_code" -eq 1 ] && [ -z "$closed_output" ] || exit 43
nc -l -p 39997 >/dev/null 2>&1 &
listener=$!
trap 'kill "$listener" 2>/dev/null || :; wait "$listener" 2>/dev/null || :' EXIT
sleep 1
open_code=0
open_output=$(nc -z -w 1 127.0.0.1 39997 2>&1) || open_code=$?
[ "$open_code" -eq 0 ] && [ -z "$open_output" ] || exit 44
wait "$listener"
trap - EXIT
external_code=0
external_output=$(nc -z -w 3 1.1.1.1 443 2>&1) || external_code=$?
[ "$external_code" -eq 0 ] && [ -z "$external_output" ] || exit 45
printf 'probe-controls-passed'
"#;

const TOOLS_ONLY: &str =
    "for tool in sh find tar; do command -v \"$tool\" >/dev/null || exit 42; done; printf probe-controls-passed";

fn required(name: &str) -> Sendable<String> {
    let value = SystemEnv.var(name).map_err(|_| format!("{name} must be set"))?;
    if value.is_empty() {
        return Err(format!("{name} must not be empty").into());
    }
    Ok(value)
}

async fn docker(arguments: &[&str]) -> Sendable<Output> {
    Ok(tokio::time::timeout(
        Duration::from_secs(30),
        Command::new("docker").args(arguments).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "the docker CLI outran its bound")??)
}

fn docker_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether the container is gone, from docker's own words; anything else leaves absence unknown.
async fn docker_absent(id: &str) -> Sendable<bool> {
    if !docker_id(id) {
        return Err("a complete docker grant names its full container id".into());
    }
    let output = docker(&["container", "inspect", "--format", "{{.Id}}", "--", id]).await?;
    if output.status.success() {
        if String::from_utf8_lossy(&output.stdout).trim() != id {
            return Err("docker inspect did not confirm the exact granted container".into());
        }
        return Ok(false);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let known_absence = [
        format!("Error: No such object: {id}"),
        format!("Error: No such container: {id}"),
        format!("Error response from daemon: No such container: {id}"),
    ];
    if stderr
        .lines()
        .any(|line| known_absence.iter().any(|expected| line.trim() == expected))
    {
        return Ok(true);
    }
    Err("docker absence stays unknown".into())
}

/// The command name of process `pid`, or `None` when no such process exists.
async fn process_name(pid: &str) -> Sendable<Option<String>> {
    let parsed: u32 = pid.parse().map_err(|_| "a subprocess grant names a PID")?;
    if parsed <= 1 {
        return Err("a subprocess grant names an actual child process".into());
    }
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new("ps")
            .args(["-p", pid, "-o", "comm="])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "the process lookup outran its bound")??;
    let name = String::from_utf8(output.stdout)?.trim().to_owned();
    match (output.status.success(), name.is_empty()) {
        (true, false) => Ok(Some(name)),
        (false, true) => Ok(None),
        _ => Err("subprocess absence stays unknown".into()),
    }
}

async fn subprocess_absent(id: &str) -> Sendable<bool> {
    Ok(process_name(id).await?.is_none())
}

/// Proves the configured worker PID is the subprocess worker, seen from this process's PID scope.
async fn subprocess_scope() -> Sendable<()> {
    let pid = required(WORKER_PID)?;
    let name = process_name(&pid)
        .await?
        .ok_or("the owned subprocess worker runs in this PID scope")?;
    let is_worker = std::path::Path::new(&name)
        .file_name()
        .is_some_and(|file| file == WORKER_BINARY);
    if !is_worker {
        return Err(format!("{WORKER_PID} names {name}, not the subprocess worker").into());
    }
    #[cfg(target_os = "linux")]
    if std::fs::read_link("/proc/self/ns/pid")? != std::fs::read_link(format!("/proc/{pid}/ns/pid"))? {
        return Err("PID absence is checked in the worker's own PID namespace".into());
    }
    tracing::info!(pid = %pid, "owned subprocess worker PID scope");
    Ok(())
}

/// Runs the image's prerequisites in a container it tracks by id, and returns the image's immutable id.
async fn docker_probe(image: &str, egress: bool) -> Sendable<String> {
    let inspected = docker(&["image", "inspect", "--format", "{{.Id}}", "--", image]).await?;
    if !inspected.status.success() {
        return Err("the configured image is already cached".into());
    }
    let image_id = String::from_utf8(inspected.stdout)?.trim().to_owned();
    if !image_id.strip_prefix("sha256:").is_some_and(docker_id) {
        return Err("the cached image inspection names its immutable id".into());
    }
    let dir = tempfile::tempdir()?;
    let cid_path = dir.path().join("probe.cid");
    let cid_argument = cid_path.to_str().ok_or("the probe cid path is UTF-8")?;
    let script = if egress { EGRESS_CONTROLS } else { TOOLS_ONLY };
    let output = docker(&[
        "run",
        "--pull",
        "never",
        "--rm",
        "--cidfile",
        cid_argument,
        "--network",
        "bridge",
        "--entrypoint",
        "sh",
        &image_id,
        "-c",
        script,
    ])
    .await;
    let id = std::fs::read_to_string(&cid_path)
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| docker_id(id))
        .ok_or("the probe kept no container id, so its allocation is unknown")?;
    if !matches!(docker_absent(&id).await, Ok(true)) {
        let removal = docker(&["container", "rm", "--force", "--", &id]).await;
        return Err(format!(
            "the probe container cleanup is unconfirmed (removal attempted: {})",
            removal.is_ok()
        )
        .into());
    }
    let output = output?;
    if !output.status.success() || output.stdout != PROBE_PASSED {
        return Err("the cached image passes the closed port, open listener and unrestricted endpoint controls".into());
    }
    Ok(image_id)
}

async fn target(kind: ProviderKind) -> Sendable<(Target, GrantLedger, WorkerHeartbeat)> {
    if SystemEnv.var(ENABLE).as_deref() != Ok("1") {
        return Err(format!("set {ENABLE}=1 to run the owned runtime conformance").into());
    }
    let client = async_nats::connect(required(support::memory::NATS_URL)?).await?;
    let heartbeat = observe_worker(&client, kind).await?;
    if heartbeat.dispatch_protocol != WorkerDispatchProtocol::Legacy {
        return Err("this run uses legacy dispatch; an assigned worker needs its own transport run".into());
    }
    let ledger = GrantLedger::default();
    let mut target = Target::new(client, kind)
        .for_worker(&heartbeat)?
        .signed_from_env(&SystemEnv)?
        .with_grant_ledger(ledger.clone());
    let profile = &heartbeat.profile;
    target.supports_pause_resume = profile.lifecycle.supports_pause_resume.is_supported();
    target.graceful_stop = profile.isolation.supports_graceful_stop.is_supported();
    target.command_restart = profile.isolation.supports_command_restart.is_supported();
    target.named_processes = profile.isolation.supports_named_processes.is_supported();
    target.process_readiness = profile.isolation.supports_process_readiness.is_supported();
    target.checkpoint_running_unsupported = !profile.snapshot.can_checkpoint_running.is_supported();
    target.native_passthrough = profile.placement.supports_native_passthrough.is_supported();
    target.redispatch_idempotent = true;
    target.file_ops = profile.isolation.supports_file_ops.is_supported();
    target.file_search = profile.isolation.supports_file_search.is_supported();
    target.file_archive = profile.isolation.supports_file_archive.is_supported();
    target.denied_connect_fails = profile.isolation.egress.deny_all.is_supported();
    let image = SystemEnv.var(support::memory::IMAGE).ok();
    if kind == ProviderKind::Subprocess {
        target.exec = ExecModel::None;
        target.image = image.unwrap_or_default();
        "/workspace".clone_into(&mut target.file_root);
    } else if let Some(image) = image {
        target.image = image;
    }
    Ok((target, ledger, heartbeat))
}

fn bare_spec(image: &str) -> Sendable<SandboxSpec> {
    let mut parts = support::request::spec()
        .map_err(|error| error.to_string())?
        .into_parts();
    parts.image = SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(image),
        pull_secret: None,
    };
    parts.cpu_seconds = std::num::NonZeroU32::MIN.saturating_add(3599);
    parts.boot_mode = BootMode::Bare;
    Ok(SandboxSpec::new(parts)?)
}

async fn run(kind: ProviderKind) -> Sendable<()> {
    let (mut target, ledger, heartbeat) = target(kind).await?;
    if kind == ProviderKind::LocalDocker {
        target.image = docker_probe(&target.image, target.denied_connect_fails).await?;
        target.spec = Some(bare_spec(&target.image)?);
    } else {
        subprocess_scope().await?;
    }
    let outcome = run_with_exact_cleanup(
        &target,
        &ledger,
        |id: SandboxId| async move {
            match kind {
                ProviderKind::LocalDocker => docker_absent(id.as_str()).await,
                _ => subprocess_absent(id.as_str()).await,
            }
        },
        |_id: SandboxId| async {
            Sendable::Err("owned runtime conformance never deletes a native resource as a passing fallback".into())
        },
    )
    .await;
    let grants = ledger.snapshot();
    let native_ids: BTreeSet<_> = grants
        .iter()
        .map(|grant| grant.native.sandbox_id.as_str().to_owned())
        .collect();
    tracing::info!(
        worker = %heartbeat.worker_id.as_str(),
        image = %target.image,
        grants = grants.len(),
        distinct_native_ids = ?native_ids,
        "owned runtime receipt"
    );
    outcome?;
    let foreign = grants.iter().any(|grant| {
        grant
            .native
            .worker_id
            .as_ref()
            .is_some_and(|worker| worker != &heartbeat.worker_id)
    });
    if foreign {
        return Err("a grant came from a worker other than the observed one".into());
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs SANDBOX_LOCAL_CONFORMANCE=1 and an owned docker worker on SANDBOX_CONFORMANCE_NATS_URL"]
async fn owned_docker_conforms_and_every_native_grant_is_absent() -> Result<(), Box<dyn std::error::Error>> {
    Ok(run(ProviderKind::LocalDocker)
        .await
        .map_err(|error| error.to_string())?)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs SANDBOX_LOCAL_CONFORMANCE=1, SANDBOX_CONFORMANCE_WORKER_PID and an owned subprocess worker on SANDBOX_CONFORMANCE_NATS_URL"]
async fn owned_subprocess_conforms_and_every_native_grant_is_absent() -> Result<(), Box<dyn std::error::Error>> {
    Ok(run(ProviderKind::Subprocess).await.map_err(|error| error.to_string())?)
}
