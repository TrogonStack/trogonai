//! Exec and egress behavior of `trogon-sandbox-k8s-worker` against a live cluster.
//!
//! Needs a stack with the k8s worker registered, on a cluster whose CNI enforces `NetworkPolicy`. See
//! `stack_support` for how the lane finds NATS. Optional settings:
//!
//! - `SANDBOX_SMOKE_K8S_IMAGE`: an image that runs as a numeric non-root user and ships busybox `sh`, `sleep` and
//!   `nc` (default `nginxinc/nginx-unprivileged:alpine`).
//! - `SANDBOX_SMOKE_K8S_PROBE_IP`: an internet address accepting TCP on port 80 (default `1.1.1.1`).
//! - `SANDBOX_SMOKE_K8S_OTHER_IP`: another address outside the allowlist the test sets (default `8.8.8.8`, which
//!   answers on port 53).
//! - `SANDBOX_SMOKE_TENANT`: the tenant the leases run under (default `k8s-live-tenant`).
//!
//! Sweep the tenant afterwards with the `stack_leak_sweep` lane under the same `SANDBOX_SMOKE_TENANT`.
//!
//! The warm pool check, skipped when they are unset, needs `SANDBOX_SMOKE_K8S_SNAPSHOT_REGISTRY` and `SANDBOX_SMOKE_K8S_SNAPSHOT_DIGEST`: a rootfs
//! artifact in the registry and repository the worker's warm pool pulls from (`SANDBOX_SNAPSHOT_REGISTRY` and
//! `SANDBOX_SNAPSHOT_REPOSITORY` on the worker, default repository `sandbox/rootfs`, overridden here with
//! `SANDBOX_SMOKE_K8S_SNAPSHOT_REPOSITORY`) whose tar holds `marker.txt` containing `warm`.
//!
//! The checkpoint check needs the broker and worker to share `SANDBOX_SNAPSHOT_REGISTRY`, reachable from the worker.
//!
//! The restart check, skipped when it is unset, needs `SANDBOX_SMOKE_K8S_RESTART_WORKER`: a shell command that restarts the k8s worker and
//! returns once the new process has started.

mod common;
mod stack_support;

use std::future::Future;
use std::time::{Duration, Instant};

use common::TestResult;
use trogon_sandbox_client::{ClientError, CommandOptions, SandboxClient};
use trogon_sandbox_contract::{
    AllowRange, ArtifactType, CommandDeadline, Digest, DomainPattern, DomainRule, DomainRuleAction, ExecutionId,
    IpRange, KillCommandResponse, LeaseError, LeaseId, LeaseRequest, NetworkPolicy, OciArtifactRef, OpaqueTunnelPolicy,
    PollCommandResponse, ProviderKind, RestartCommandResponse, SandboxImage, SandboxSpec, SnapshotLeaseResponse,
    StdinMode, StopCommandResponse, StopSignal, TenantId, UnixSignal,
};

const DEFAULT_IMAGE: &str = "nginxinc/nginx-unprivileged:alpine";
const DEFAULT_TENANT: &str = "k8s-live-tenant";
const DEFAULT_REPOSITORY: &str = "sandbox/rootfs";

fn probe_ip() -> String {
    stack_support::env_or("SANDBOX_SMOKE_K8S_PROBE_IP", "1.1.1.1")
}

fn other_ip() -> String {
    stack_support::env_or("SANDBOX_SMOKE_K8S_OTHER_IP", "8.8.8.8")
}

fn tenant() -> Result<TenantId, Box<dyn std::error::Error>> {
    Ok(TenantId::new(stack_support::env_or(
        "SANDBOX_SMOKE_TENANT",
        DEFAULT_TENANT,
    ))?)
}

/// Fails with `message` unless `condition` holds, so the caller can still release its lease.
fn ensure(condition: bool, message: impl Into<String>) -> TestResult {
    if condition { Ok(()) } else { Err(message.into().into()) }
}

/// Runs `checks` and releases `lease` whether or not they pass, so a failed check does not leave a pod running on
/// the cluster.
async fn with_release(client: &SandboxClient, lease: &LeaseId, checks: impl Future<Output = TestResult>) -> TestResult {
    let outcome = checks.await;
    let released = client.delete_lease(tenant()?, lease).await;
    outcome?;
    released?;
    Ok(())
}

fn lease_request(network_policy: Option<NetworkPolicy>) -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let image = stack_support::env_or("SANDBOX_SMOKE_K8S_IMAGE", DEFAULT_IMAGE);
    lease_request_for(stack_support::registry_image(&image), network_policy)
}

fn lease_request_for(
    image: SandboxImage,
    network_policy: Option<NetworkPolicy>,
) -> Result<LeaseRequest, Box<dyn std::error::Error>> {
    let mut parts = stack_support::spec_parts(image, 1, 128)?;
    parts.network_policy = network_policy;
    Ok(stack_support::lease_request(
        tenant()?,
        ProviderKind::K8sMicroVm,
        SandboxSpec::new(parts)?,
        600,
        "k8s-live",
    )?)
}

fn user_defined(rules: Vec<DomainRule>, allow_ranges: Vec<IpRange>) -> NetworkPolicy {
    NetworkPolicy::UserDefined {
        rules,
        allow_ranges: allow_ranges.into_iter().map(AllowRange::from).collect(),
        deny_ranges: Vec::new(),
        denied_domains: Vec::new(),
        opaque_tunnels: OpaqueTunnelPolicy::default(),
        request_rules: Vec::new(),
    }
}

/// A policy that allows only `ip` and no hostnames.
fn ip_allowlist(ip: &str) -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(user_defined(Vec::new(), vec![IpRange::new(format!("{ip}/32"))?]))
}

/// A policy made of one hostname rule and no IP ranges, which the worker cannot enforce.
fn domain_only_policy() -> Result<NetworkPolicy, Box<dyn std::error::Error>> {
    Ok(user_defined(
        vec![DomainRule {
            domain: DomainPattern::new("example.com")?,
            action: DomainRuleAction::PassThrough,
        }],
        Vec::new(),
    ))
}

fn options(timeout_seconds: u32) -> CommandOptions {
    CommandOptions {
        timeout_seconds,
        ..CommandOptions::default()
    }
}

/// Whether the sandbox can open a TCP connection to `ip` on port 80. A TCP probe, because an HTTP fetch can
/// redirect to a hostname, and an IP-only allowlist blocks the DNS lookup that follows.
async fn reaches(client: &SandboxClient, lease: &LeaseId, ip: &str) -> Result<bool, ClientError> {
    reaches_port(client, lease, ip, 80).await
}

async fn reaches_port(client: &SandboxClient, lease: &LeaseId, ip: &str, port: u16) -> Result<bool, ClientError> {
    let output = client
        .run_command(
            lease,
            stack_support::sh(&format!("nc -z -w 5 {ip} {port}")),
            options(20),
        )
        .await?;
    Ok(output.exit_code == 0)
}

async fn reaches_eventually(
    client: &SandboxClient,
    lease: &LeaseId,
    ip: &str,
    want: bool,
) -> Result<bool, ClientError> {
    // The CNI applies a NetworkPolicy change asynchronously.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let got = reaches(client, lease, ip).await?;
        if got == want || Instant::now() >= deadline {
            return Ok(got);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn start(client: &SandboxClient, lease: &LeaseId, script: &str) -> Result<ExecutionId, ClientError> {
    client
        .run_command_detached(
            lease,
            stack_support::sh(script),
            options(300),
            StdinMode::Closed,
            CommandDeadline::Timeout,
        )
        .await
}

async fn wait_running(client: &SandboxClient, lease: &LeaseId, execution_id: &ExecutionId) -> TestResult {
    // Give the exec time to show up in the pod so a signal finds it.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let polled = client.poll_command(lease, execution_id).await?;
    ensure(
        matches!(polled, PollCommandResponse::Running),
        format!("expected Running, got {polled:?}"),
    )
}

/// The trimmed stdout of `script` in `lease`, after checking that it exited cleanly.
async fn stdout_of(
    client: &SandboxClient,
    lease: &LeaseId,
    script: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let output = client
        .run_command(lease, stack_support::sh(script), options(20))
        .await?;
    ensure(output.exit_code == 0, format!("`{script}` failed: {output:?}"))?;
    Ok(stack_support::stdout(&output).trim().to_string())
}

/// Leases `image` until the worker hands out a warm pod. The first lease of a digest is cold and teaches the worker
/// the digest; its warm pool then tops up in the background.
async fn lease_warm(client: &SandboxClient, image: &SandboxImage) -> Result<LeaseId, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let granted = client
            .lease(&lease_request_for(image.clone(), None)?, Duration::from_secs(240))
            .await?;
        if granted.sandbox_id.starts_with("wp-") {
            return Ok(granted.lease_id);
        }
        client.delete_lease(tenant()?, &granted.lease_id).await?;
        ensure(Instant::now() < deadline, "no warm pod was claimed in time")?;
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn deny_all_and_policy_updates(client: &SandboxClient, lease: &LeaseId, ip: &str) -> TestResult {
    ensure(
        !reaches(client, lease, ip).await?,
        "a lease with no policy runs under deny all",
    )?;

    client.update_network_policy(lease, NetworkPolicy::AllowAll).await?;
    ensure(
        reaches_eventually(client, lease, ip, true).await?,
        "AllowAll update opens egress",
    )?;

    client.update_network_policy(lease, NetworkPolicy::DenyAll).await?;
    ensure(
        !reaches_eventually(client, lease, ip, false).await?,
        "DenyAll update closes egress",
    )?;

    client.update_network_policy(lease, ip_allowlist(ip)?).await?;
    ensure(
        reaches_eventually(client, lease, ip, true).await?,
        "the allowed range is reachable",
    )?;
    ensure(
        !reaches_port(client, lease, &other_ip(), 53).await?,
        "an address outside the allowed range stays blocked",
    )?;

    let domain_only = client.update_network_policy(lease, domain_only_policy()?).await;
    ensure(
        domain_only.is_err(),
        format!("a domain-only update is refused: {domain_only:?}"),
    )
}

#[tokio::test]
#[ignore = "requires a live stack with the k8s worker on a NetworkPolicy-enforcing cluster"]
async fn deny_all_blocks_egress_and_policy_updates_apply_in_place() -> TestResult {
    let client = stack_support::connect().await?;
    let ip = probe_ip();
    let granted = client.lease(&lease_request(None)?, Duration::from_secs(240)).await?;
    let lease = granted.lease_id;

    with_release(&client, &lease, deny_all_and_policy_updates(&client, &lease, &ip)).await
}

#[tokio::test]
#[ignore = "requires a live stack with the k8s worker on a NetworkPolicy-enforcing cluster"]
async fn a_domain_only_policy_is_refused_at_lease_time() -> TestResult {
    let client = stack_support::connect().await?;
    let refused = client
        .lease(&lease_request(Some(domain_only_policy()?))?, Duration::from_secs(60))
        .await;
    // The broker may refuse first, when no worker advertises a domain allowlist; otherwise the worker refuses the
    // policy itself.
    match refused {
        Err(
            ClientError::Denied(LeaseError::InvalidNetworkPolicy { .. })
            | ClientError::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. }),
        ) => Ok(()),
        Err(other) => Err(format!("expected InvalidNetworkPolicy, got {other:?}").into()),
        Ok(granted) => {
            client.delete_lease(tenant()?, &granted.lease_id).await?;
            Err("a domain-only policy was granted".into())
        }
    }
}

async fn exec_lifecycle(client: &SandboxClient, lease: &LeaseId) -> TestResult {
    // An id that was never started.
    let unknown = client
        .kill_command(lease, &ExecutionId::new("01J00000000000000000000000"), UnixSignal::Term)
        .await?;
    ensure(
        unknown == KillCommandResponse::NotFound,
        format!("kill of an unknown id: {unknown:?}"),
    )?;

    // SIGTERM ends a plain sleep within the grace period.
    let plain = start(client, lease, "sleep 300").await?;
    wait_running(client, lease, &plain).await?;
    let stopped = client.stop_command(lease, &plain, Some(5)).await?;
    ensure(
        stopped
            == StopCommandResponse::Stopped {
                signal: StopSignal::Term,
            },
        format!("a plain sleep stops on SIGTERM: {stopped:?}"),
    )?;

    // A command that ignores SIGTERM takes the SIGKILL.
    let stubborn = start(client, lease, "trap '' TERM; while :; do sleep 1; done").await?;
    wait_running(client, lease, &stubborn).await?;
    let started = Instant::now();
    let stopped = client.stop_command(lease, &stubborn, Some(2)).await?;
    ensure(
        stopped
            == StopCommandResponse::Stopped {
                signal: StopSignal::Kill,
            },
        format!("a command that ignores SIGTERM is killed: {stopped:?}"),
    )?;
    ensure(
        started.elapsed() >= Duration::from_secs(2),
        "the grace period passed before the kill",
    )?;

    // Restart runs the same command again under a new id.
    let first = start(client, lease, "sleep 300").await?;
    wait_running(client, lease, &first).await?;
    let restarted = client.restart_command(lease, &first, Some(5)).await?;
    let RestartCommandResponse::Restarted { execution_id } = restarted else {
        return Err(format!("expected Restarted, got {restarted:?}").into());
    };
    ensure(execution_id != first, "the restarted command has a new id")?;
    wait_running(client, lease, &execution_id).await?;
    let killed = client.kill_command(lease, &execution_id, UnixSignal::Kill).await?;
    ensure(
        killed == KillCommandResponse::Signaled,
        format!("kill of a running command: {killed:?}"),
    )?;

    // Output past the default 1 MiB cap is truncated, not buffered whole.
    let big = client
        .run_command(
            lease,
            stack_support::sh("head -c 3000000 /dev/zero | tr '\\0' a"),
            options(60),
        )
        .await?;
    ensure(big.truncated.is_truncated(), "3 MB of output is truncated")?;
    ensure(big.stdout.len() <= 1024 * 1024, "truncated output stays within 1 MiB")?;

    // A command past its deadline is killed and flagged.
    let started = Instant::now();
    let slow = client
        .run_command(lease, stack_support::sh("sleep 60"), options(3))
        .await?;
    ensure(
        slow.timed_out.is_timed_out(),
        format!("the slow command timed out: {slow:?}"),
    )?;
    ensure(
        slow.exit_code == trogon_sandbox_contract::CommandOutput::TIMED_OUT_EXIT_CODE,
        format!("a timed-out command reports the timeout exit code: {slow:?}"),
    )?;
    ensure(
        started.elapsed() < Duration::from_secs(30),
        "the deadline ended the command promptly",
    )
}

#[tokio::test]
#[ignore = "requires a live stack with the k8s worker"]
async fn exec_lifecycle_stop_restart_kill_cap_and_timeout() -> TestResult {
    let client = stack_support::connect().await?;
    let granted = client.lease(&lease_request(None)?, Duration::from_secs(240)).await?;
    let lease = granted.lease_id;

    with_release(&client, &lease, exec_lifecycle(&client, &lease)).await
}

/// The rootfs artifact the warm pool pulls from, named by the `SANDBOX_SMOKE_K8S_SNAPSHOT_*` variables, or `None`
/// when they are unset and the check is skipped.
fn warm_pool_image() -> Result<Option<SandboxImage>, Box<dyn std::error::Error>> {
    let (Ok(registry), Ok(digest)) = (
        stack_support::var("SANDBOX_SMOKE_K8S_SNAPSHOT_REGISTRY"),
        stack_support::var("SANDBOX_SMOKE_K8S_SNAPSHOT_DIGEST"),
    ) else {
        return Ok(None);
    };
    let repository = stack_support::env_or("SANDBOX_SMOKE_K8S_SNAPSHOT_REPOSITORY", DEFAULT_REPOSITORY);
    Ok(Some(SandboxImage::Snapshot {
        reference: OciArtifactRef {
            registry,
            repository,
            digest: Digest::new(digest)?,
            artifact_type: ArtifactType::SandboxRootfsV1,
        },
        parent: None,
    }))
}

async fn warm_pod_checks(client: &SandboxClient, lease: &LeaseId, ip: &str) -> TestResult {
    let marker = stdout_of(client, lease, "cat /workspace/marker.txt").await?;
    ensure(
        marker == "warm",
        format!("the warm pod restored the snapshot: {marker:?}"),
    )?;

    ensure(
        !reaches_eventually(client, lease, ip, false).await?,
        "a claimed warm pod runs under deny all",
    )?;
    client.update_network_policy(lease, NetworkPolicy::AllowAll).await?;
    ensure(
        reaches_eventually(client, lease, ip, true).await?,
        "AllowAll update opens the warm pod's egress",
    )?;
    client.update_network_policy(lease, NetworkPolicy::DenyAll).await?;
    ensure(
        !reaches_eventually(client, lease, ip, false).await?,
        "DenyAll update closes the warm pod's egress",
    )
}

#[tokio::test]
#[ignore = "requires a live stack with the k8s worker and its warm pool; skips itself when the SANDBOX_SMOKE_K8S_SNAPSHOT_* variables are unset"]
async fn a_claimed_warm_pod_runs_under_the_lease_policy() -> TestResult {
    let Some(image) = warm_pool_image()? else {
        eprintln!("skipped: SANDBOX_SMOKE_K8S_SNAPSHOT_REGISTRY and SANDBOX_SMOKE_K8S_SNAPSHOT_DIGEST are unset");
        return Ok(());
    };
    let client = stack_support::connect().await?;
    let ip = probe_ip();

    // The first lease of a digest is cold and teaches the worker the digest; its warm pool then tops up in the
    // background.
    let lease = lease_warm(&client, &image).await?;

    with_release(&client, &lease, warm_pod_checks(&client, &lease, &ip)).await
}

async fn write_marker_and_checkpoint(
    client: &SandboxClient,
    source: &LeaseId,
) -> Result<OciArtifactRef, Box<dyn std::error::Error>> {
    let wrote = client
        .run_command(
            source,
            stack_support::sh("printf checkpointed > /workspace/marker.txt"),
            options(20),
        )
        .await?;
    ensure(wrote.exit_code == 0, format!("wrote the marker: {wrote:?}"))?;

    let reference = match client.snapshot_lease(tenant()?, source).await? {
        SnapshotLeaseResponse::Created { receipt } => receipt.reference,
        other => return Err(format!("the running lease was not checkpointed: {other:?}").into()),
    };

    let still_running = client
        .run_command(source, stack_support::sh("true"), options(20))
        .await?;
    ensure(still_running.exit_code == 0, "the source keeps running")?;
    Ok(reference)
}

#[tokio::test]
#[ignore = "requires a live stack with the k8s worker, its warm pool and a snapshot registry shared by broker and worker"]
async fn a_running_lease_checkpoints_and_a_warm_pod_restores_it() -> TestResult {
    let client = stack_support::connect().await?;
    let granted = client.lease(&lease_request(None)?, Duration::from_secs(240)).await?;
    let source = granted.lease_id;

    let mut reference = None;
    let checkpoint = async {
        reference = Some(write_marker_and_checkpoint(&client, &source).await?);
        Ok(())
    };
    with_release(&client, &source, checkpoint).await?;

    let image = SandboxImage::Snapshot {
        reference: reference.ok_or("a checkpoint reference")?,
        parent: None,
    };
    let restored = lease_warm(&client, &image).await?;
    let check = async {
        let marker = stdout_of(&client, &restored, "cat /workspace/marker.txt").await?;
        ensure(
            marker == "checkpointed",
            format!("the warm pod restored the checkpoint: {marker:?}"),
        )
    };
    with_release(&client, &restored, check).await
}

#[tokio::test]
#[ignore = "requires a live stack with the k8s worker"]
async fn a_plain_alpine_lease_runs_non_root_and_releases_promptly() -> TestResult {
    let client = stack_support::connect().await?;
    let granted = client
        .lease(
            &lease_request_for(stack_support::registry_image("alpine:3.20"), None)?,
            Duration::from_secs(240),
        )
        .await?;
    let lease = granted.lease_id;

    let checks = async {
        let output = client
            .run_command(
                &lease,
                stack_support::sh("id -u && printf ok > /workspace/probe && cat /workspace/probe"),
                options(20),
            )
            .await?;
        ensure(output.exit_code == 0, format!("{output:?}"))?;
        let stdout = stack_support::stdout(&output);
        let mut lines = stdout.lines();
        ensure(lines.next() != Some("0"), format!("the agent is not root: {output:?}"))?;
        ensure(
            lines.next() == Some("ok"),
            format!("the workspace is writable: {output:?}"),
        )
    }
    .await;

    // The keep-alive exits on SIGTERM, so the pod does not sit out the kubelet's default 30 second grace period.
    let started = Instant::now();
    let released = client.delete_lease(tenant()?, &lease).await;
    let release_time = started.elapsed();
    checks?;
    released?;
    ensure(
        release_time < Duration::from_secs(20),
        format!("release took {release_time:?}"),
    )
}

async fn survives_restart(client: &SandboxClient, lease: &LeaseId, restart: &str) -> TestResult {
    let wrote = client
        .run_command(
            lease,
            stack_support::sh("printf survived > /workspace/marker.txt"),
            options(20),
        )
        .await?;
    ensure(wrote.exit_code == 0, format!("wrote the marker: {wrote:?}"))?;

    let status = std::process::Command::new("sh").arg("-c").arg(restart).status()?;
    ensure(status.success(), format!("the restart command failed: {status}"))?;

    // The new worker recovers the lease from the cluster before it serves, so retry until it answers.
    let deadline = Instant::now() + Duration::from_secs(90);
    let marker = loop {
        let attempt = client
            .run_command(lease, stack_support::sh("cat /workspace/marker.txt"), options(20))
            .await;
        match attempt {
            Ok(output) if output.exit_code == 0 => break output,
            other => {
                ensure(
                    Instant::now() < deadline,
                    format!("the restarted worker never served the lease: {other:?}"),
                )?;
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    };
    ensure(
        stack_support::stdout(&marker).trim() == "survived",
        format!("the marker survived the restart: {marker:?}"),
    )
}

#[tokio::test]
#[ignore = "requires a live stack with the k8s worker and a way to restart it; skips itself when SANDBOX_SMOKE_K8S_RESTART_WORKER is unset"]
async fn a_cold_lease_survives_a_worker_restart() -> TestResult {
    let Ok(restart) = stack_support::var("SANDBOX_SMOKE_K8S_RESTART_WORKER") else {
        eprintln!("skipped: SANDBOX_SMOKE_K8S_RESTART_WORKER is unset");
        return Ok(());
    };
    let client = stack_support::connect().await?;
    let granted = client.lease(&lease_request(None)?, Duration::from_secs(240)).await?;
    let lease = granted.lease_id;

    with_release(&client, &lease, survives_restart(&client, &lease, &restart)).await
}
