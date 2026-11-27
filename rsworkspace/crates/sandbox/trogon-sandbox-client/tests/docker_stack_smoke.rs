//! End to end: `SandboxClient` to the broker to `trogon-sandbox-docker-worker` to a host `docker run`.
//!
//! Needs NATS and the broker from `devops/docker/compose/sandbox/compose.yaml` and a `trogon-sandbox-docker-worker`
//! on the host against them, started the way `mise run sandbox:smoke-docker` starts them and still
//! running, then run this lane with `--ignored`. See `stack_support` for how the lane finds NATS.

mod common;
mod stack_support;

use std::time::Duration;

use common::TestResult;
use trogon_sandbox_client::SandboxClient;
use trogon_sandbox_contract::{BootMode, HostMountAccess, LeaseGranted, ProviderKind, SandboxSpec, WorkspaceSource};

const IMAGE: &str = "alpine:3.20";

async fn lease_and_release(
    client: &SandboxClient,
    workspace: WorkspaceSource,
) -> Result<LeaseGranted, Box<dyn std::error::Error>> {
    let tenant = stack_support::smoke_tenant()?;
    let mut parts = stack_support::spec_parts(stack_support::registry_image(IMAGE), 1, 128)?;
    parts.workspace = workspace;
    // A plain alpine image has no long-running entrypoint, so the container would exit before the grant.
    parts.boot_mode = BootMode::Bare;
    let request = stack_support::lease_request(
        tenant.clone(),
        ProviderKind::LocalDocker,
        SandboxSpec::new(parts)?,
        120,
        "smoke-docker",
    )?;
    let granted = client.lease(&request, Duration::from_secs(45)).await?;
    // Release before judging the grant so the leak sweep sees a clean stack either way.
    client.delete_lease(tenant, &granted.lease_id).await?;
    Ok(granted)
}

#[tokio::test]
#[ignore = "requires the compose broker and a running trogon-sandbox-docker-worker"]
async fn lease_alpine_via_broker_and_docker_worker() -> TestResult {
    let client = stack_support::connect().await?;
    let granted = lease_and_release(&client, WorkspaceSource::empty()).await?;
    assert_eq!(granted.provider, ProviderKind::LocalDocker);
    assert!(!granted.sandbox_id.is_empty());
    assert!(!granted.evidence_subject.is_empty());
    Ok(())
}

#[tokio::test]
#[ignore = "requires the compose broker, a running trogon-sandbox-docker-worker and a host-mount allow-list entry"]
async fn lease_host_mount_via_broker_and_docker_worker() -> TestResult {
    let client = stack_support::connect().await?;
    let path = stack_support::env_or("SANDBOX_SMOKE_DOCKER_HOST_MOUNT", "/tmp/sandbox-smoke-workspace");
    let workspace = WorkspaceSource::HostMount {
        path,
        access: HostMountAccess::ReadOnly,
    };
    let granted = lease_and_release(&client, workspace).await?;
    assert_eq!(granted.provider, ProviderKind::LocalDocker);
    assert!(!granted.sandbox_id.is_empty());
    assert!(!granted.evidence_subject.is_empty());
    Ok(())
}
