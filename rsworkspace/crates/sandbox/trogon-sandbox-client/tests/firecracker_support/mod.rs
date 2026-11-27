//! Helpers shared by the firecracker lanes.
//!
//! The lanes lease from a broker whose firecracker worker serves the image named by `SANDBOX_FIRECRACKER_IMAGE`
//! (default `guest:base`). When `SANDBOX_FIRECRACKER_STATE_DIR` names the worker's state directory, a released
//! lease is also checked to leave nothing behind on the host: its lease directory and its VMM process. When
//! `SANDBOX_FIRECRACKER_NETNS_PREFIX` names the worker's network namespace prefix, the lease's namespace must be
//! gone too.

#![allow(dead_code)]

use std::future::Future;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use trogon_sandbox_client::{ClientError, CommandOptions, SandboxClient};
use trogon_sandbox_contract::{
    CommandDeadline, CommandOutput, ExecutionId, LeaseGranted, LeaseId, LeasePersistence, LeaseRequest,
    PollCommandResponse, ProviderKind, SandboxSpec, SandboxSpecParts, StdinMode, TenantId,
};

use crate::stack_support;

pub type Fallible<T> = Result<T, Box<dyn std::error::Error>>;

/// The image the firecracker worker serves when `SANDBOX_FIRECRACKER_IMAGE` is unset.
pub const DEFAULT_IMAGE: &str = "guest:base";

/// How long a released lease's host resources may take to disappear.
const HOST_RELEASE_BUDGET: Duration = Duration::from_secs(15);

/// How long the broker may take to grant a lease.
pub const LEASE_TIMEOUT: Duration = Duration::from_secs(60);

/// The image the worker serves.
pub fn image() -> String {
    stack_support::env_or("SANDBOX_FIRECRACKER_IMAGE", DEFAULT_IMAGE)
}

/// A firecracker spec for the served image with `cpu_seconds` and `memory_mib`, every other field at its default.
pub fn spec_parts(cpu_seconds: u32, memory_mib: u32) -> Fallible<SandboxSpecParts> {
    stack_support::spec_parts(stack_support::registry_image(&image()), cpu_seconds, memory_mib)
}

/// A lease of `parts` on the firecracker provider under `tenant`, with a fresh idempotency key built from `label`.
/// It is not persistent, so releasing it leaves nothing resumable behind.
pub fn lease_request(tenant: &str, parts: SandboxSpecParts, ttl_seconds: u32, label: &str) -> Fallible<LeaseRequest> {
    let mut request = stack_support::lease_request(
        TenantId::new(tenant)?,
        ProviderKind::Firecracker,
        SandboxSpec::new(parts)?,
        ttl_seconds,
        label,
    )?;
    request.persistent = LeasePersistence::Ephemeral;
    Ok(request)
}

/// Command options with only the timeout set.
pub fn timeout(timeout_seconds: u32) -> CommandOptions {
    CommandOptions {
        timeout_seconds,
        ..CommandOptions::default()
    }
}

/// Leases `request` and checks that the firecracker worker granted it.
pub async fn lease(client: &SandboxClient, request: &LeaseRequest) -> Fallible<LeaseGranted> {
    let granted = client.lease(request, LEASE_TIMEOUT).await?;
    assert_eq!(granted.provider, ProviderKind::Firecracker);
    assert!(!granted.sandbox_id.is_empty());
    Ok(granted)
}

/// Runs `body` against a fresh lease, then cancels it and checks its host resources are gone, whether or not `body`
/// succeeded. A failure of `body` is reported ahead of a failure to clean up.
pub async fn with_lease<F, Fut>(client: &SandboxClient, request: LeaseRequest, body: F) -> Fallible<()>
where
    F: FnOnce(LeaseGranted) -> Fut,
    Fut: Future<Output = Fallible<()>>,
{
    let granted = lease(client, &request).await?;
    let lease_id = granted.lease_id.clone();
    let outcome = body(granted).await;
    let cancelled = client.cancel(&lease_id).await;
    let released = assert_host_resources_released(&lease_id).await;
    outcome?;
    cancelled?;
    released
}

/// Runs `argv` in the guest.
pub async fn run(
    client: &SandboxClient,
    lease_id: &LeaseId,
    argv: &[&str],
    timeout_seconds: u32,
) -> Result<CommandOutput, ClientError> {
    client
        .run_command(
            lease_id,
            argv.iter().map(|part| (*part).to_string()).collect(),
            timeout(timeout_seconds),
        )
        .await
}

/// Runs `script` under `/bin/sh -c` in the guest with `timeout_seconds` to finish.
pub async fn sh_within(
    client: &SandboxClient,
    lease_id: &LeaseId,
    script: &str,
    timeout_seconds: u32,
) -> Result<CommandOutput, ClientError> {
    client
        .run_command(lease_id, stack_support::sh(script), timeout(timeout_seconds))
        .await
}

/// Runs `script` under `/bin/sh -c` in the guest with 30 seconds to finish.
pub async fn guest_sh(client: &SandboxClient, lease_id: &LeaseId, script: &str) -> Result<CommandOutput, ClientError> {
    sh_within(client, lease_id, script, 30).await
}

/// `output`'s stdout when the command exited 0, else an error carrying both streams.
pub fn succeeded(output: &CommandOutput) -> Fallible<String> {
    if output.exit_code == 0 {
        Ok(stack_support::stdout(output))
    } else {
        Err(format!(
            "exit code {}: stdout {:?}, stderr {:?}",
            output.exit_code,
            stack_support::stdout(output),
            stack_support::stderr(output)
        )
        .into())
    }
}

/// Runs `script` and returns its stdout, failing unless it exited 0.
pub async fn guest_stdout(client: &SandboxClient, lease_id: &LeaseId, script: &str) -> Fallible<String> {
    succeeded(&guest_sh(client, lease_id, script).await?)
}

/// Starts `script` as a detached command that may run for two minutes.
pub async fn detached(
    client: &SandboxClient,
    lease_id: &LeaseId,
    script: &str,
    stdin: StdinMode,
) -> Result<ExecutionId, ClientError> {
    client
        .run_command_detached(
            lease_id,
            stack_support::sh(script),
            timeout(120),
            stdin,
            CommandDeadline::Timeout,
        )
        .await
}

/// Polls a detached command until it completes, failing when it has not within `within`.
pub async fn poll_until_completed(
    client: &SandboxClient,
    lease_id: &LeaseId,
    execution_id: &ExecutionId,
    within: Duration,
) -> Fallible<CommandOutput> {
    let deadline = Instant::now() + within;
    loop {
        match client.poll_command(lease_id, execution_id).await? {
            PollCommandResponse::Completed(output) => return Ok(output),
            PollCommandResponse::Running => {}
            other => return Err(format!("unexpected poll response: {other:?}").into()),
        }
        if Instant::now() >= deadline {
            return Err(format!("execution {execution_id} did not complete within {within:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The worker's state directory, when `SANDBOX_FIRECRACKER_STATE_DIR` names it.
pub fn state_dir() -> Option<PathBuf> {
    crate::stack_support::var("SANDBOX_FIRECRACKER_STATE_DIR")
        .ok()
        .map(PathBuf::from)
}

/// The worker's state directory, or an error naming the variable that supplies it.
pub fn required_state_dir() -> Fallible<PathBuf> {
    state_dir().ok_or_else(|| "set SANDBOX_FIRECRACKER_STATE_DIR to the worker's state directory".into())
}

/// The worker's record of a lease, gone once the VM is released.
pub fn host_lease_dir(lease_id: &LeaseId) -> Option<PathBuf> {
    state_dir().map(|dir| dir.join(lease_id.as_str()))
}

/// The lease's network namespace, when `SANDBOX_FIRECRACKER_NETNS_PREFIX` names the worker's prefix.
pub fn host_netns(lease_id: &LeaseId) -> Option<PathBuf> {
    crate::stack_support::var("SANDBOX_FIRECRACKER_NETNS_PREFIX")
        .ok()
        .map(|prefix| PathBuf::from("/run/netns").join(format!("{prefix}{lease_id}")))
}

/// The pid of the lease's VMM, from the pid file the worker records in the lease directory.
pub fn vmm_pid(lease_id: &LeaseId) -> Fallible<u32> {
    let file = required_state_dir()?.join(lease_id.as_str()).join("firecracker.pid");
    let text = std::fs::read_to_string(&file).map_err(|error| format!("reading {}: {error}", file.display()))?;
    Ok(text.trim().parse()?)
}

/// The pid of the supervised worker, from the file `SANDBOX_SMOKE_FIRECRACKER_WORKER_PIDFILE` names.
pub fn worker_pid() -> Fallible<u32> {
    let file = stack_support::required_env("SANDBOX_SMOKE_FIRECRACKER_WORKER_PIDFILE")?;
    let text = std::fs::read_to_string(&file).map_err(|error| format!("reading {file}: {error}"))?;
    Ok(text.trim().parse()?)
}

/// Sends `signal` (a name such as `KILL`, `STOP` or `CONT`) to `pid`.
pub fn signal(pid: u32, signal: &str) -> Fallible<()> {
    let status = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(pid.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kill -{signal} {pid} failed").into())
    }
}

/// Whether a process whose command line names the lease's directory is still running.
pub fn vmm_running_for(lease_id: &LeaseId) -> bool {
    let Some(dir) = host_lease_dir(lease_id) else {
        return false;
    };
    std::process::Command::new("pgrep")
        .arg("-f")
        .arg("--")
        .arg(dir.join(""))
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Waits for everything the lease held on the host to go: its directory, its VMM and its network namespace. Each
/// check applies only when the variable that locates it is set.
pub async fn assert_host_resources_released(lease_id: &LeaseId) -> Fallible<()> {
    let deadline = Instant::now() + HOST_RELEASE_BUDGET;
    loop {
        let leftover = [host_lease_dir(lease_id), host_netns(lease_id)]
            .into_iter()
            .flatten()
            .find(|path| path.exists());
        let vmm = vmm_running_for(lease_id);
        if leftover.is_none() && !vmm {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(match leftover {
                Some(path) => format!("{} still exists after the lease was released", path.display()),
                None => format!("the VMM of lease {lease_id} outlived the lease"),
            }
            .into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Waits until a command on the lease is refused, which is what a released lease answers.
pub async fn assert_commands_denied(client: &SandboxClient, lease_id: &LeaseId) -> Fallible<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let result = run(client, lease_id, &["true"], 5).await;
        if matches!(result, Err(ClientError::Denied(_))) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("run_command on a released lease was not denied: {result:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
