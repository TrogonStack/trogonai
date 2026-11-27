//! End to end: `SandboxClient` to the broker to `trogon-sandbox-substrate-worker` to an Agent Substrate actor.
//!
//! Needs a substrate worker running against a real Agent Substrate deployment, with its agent image configured
//! (`SANDBOX_SUBSTRATE_AGENT_IMAGE`), and `trogon-sandbox-broker` on the same NATS (see `stack_support` for how the
//! lane finds it). `SANDBOX_SMOKE_SUBSTRATE_IMAGE` names the digest-pinned tenant image the actor runs, and
//! `SANDBOX_SMOKE_TENANT` the tenant (default `trogon-sandbox`).
//!
//! The lane walks one named lease through run, files, stop and resume, delete, and `delete_named_sandbox`.

mod common;
mod stack_support;

use std::time::Duration;

use common::TestResult;
use trogon_sandbox_client::{CommandOptions, SandboxClient};
use trogon_sandbox_contract::{
    DeleteNamedSandboxResponse, LeaseGranted, LeaseName, LeaseRequest, ProviderKind, SandboxFileWrite, SandboxSpec,
    TenantId,
};

const FILE: &str = "/tmp/substrate-smoke.txt";
const LEASE_TIMEOUT: Duration = Duration::from_secs(180);

/// The client cannot observe actor state, and the worker suspends or deletes an actor after the broker has already
/// answered. Waiting lets that land, so the next lease of the name sees its result instead of the old actor.
const SETTLE: Duration = Duration::from_secs(20);

/// `request` again under a fresh idempotency key, so the broker treats it as a new lease of the same name.
async fn lease_again(
    client: &SandboxClient,
    request: &LeaseRequest,
    label: &str,
) -> Result<LeaseGranted, Box<dyn std::error::Error>> {
    let again = LeaseRequest {
        operation_id: None,
        idempotency_key: stack_support::request_key(label)?,
        ..request.clone()
    };
    Ok(client.lease(&again, LEASE_TIMEOUT).await?)
}

async fn write(client: &SandboxClient, lease: &LeaseGranted, content: &[u8]) -> TestResult {
    let file = SandboxFileWrite {
        path: FILE.to_string(),
        content: content.to_vec(),
        mode: None,
    };
    client.write_files(&lease.lease_id, vec![file], None).await?;
    Ok(())
}

async fn lifecycle(client: &SandboxClient, tenant: &TenantId, name: &LeaseName, request: &LeaseRequest) -> TestResult {
    let granted = client.lease(request, LEASE_TIMEOUT).await?;
    assert_eq!(granted.provider, ProviderKind::Substrate);
    assert!(!granted.sandbox_id.is_empty());

    let output = client
        .run_command(
            &granted.lease_id,
            vec!["echo".to_string(), "hello from substrate".to_string()],
            CommandOptions {
                timeout_seconds: 30,
                ..CommandOptions::default()
            },
        )
        .await?;
    assert!(
        stack_support::stdout(&output).contains("hello from substrate"),
        "unexpected stdout: {:?}",
        output.stdout
    );

    write(client, &granted, b"written by substrate_stack_smoke").await?;
    let read_back = client.read_file(&granted.lease_id, FILE.to_string(), None).await?;
    assert_eq!(read_back, b"written by substrate_stack_smoke");

    // A stopped named lease resumes into the same actor with its files.
    client.stop_lease(tenant.clone(), &granted.lease_id).await?;
    tokio::time::sleep(SETTLE).await;
    let resumed = lease_again(client, request, "substrate-resume").await?;
    assert_eq!(resumed.sandbox_id, granted.sandbox_id);
    let survived = client.read_file(&resumed.lease_id, FILE.to_string(), None).await?;
    assert_eq!(survived, b"written by substrate_stack_smoke");

    // A deleted lease frees the name and takes its files with it.
    client.delete_lease(tenant.clone(), &resumed.lease_id).await?;
    tokio::time::sleep(SETTLE).await;
    let fresh = lease_again(client, request, "substrate-after-delete").await?;
    assert!(
        client.read_file(&fresh.lease_id, FILE.to_string(), None).await.is_err(),
        "a deleted lease must not hand its files to the next lease under the same name"
    );
    client.delete_lease(tenant.clone(), &fresh.lease_id).await?;

    // `delete_lease` needs a running lease to find anything, so it cannot reach a named sandbox that `stop_lease`
    // left suspended with no active lease. `delete_named_sandbox` finds it by name instead.
    let stopped = lease_again(client, request, "substrate-stopped").await?;
    write(client, &stopped, b"written before delete_named_sandbox").await?;
    client.stop_lease(tenant.clone(), &stopped.lease_id).await?;
    tokio::time::sleep(SETTLE).await;
    let response = client.delete_named_sandbox(tenant.clone(), name.clone()).await?;
    assert!(
        matches!(response, DeleteNamedSandboxResponse::Deleted),
        "expected Deleted, got {response:?}"
    );
    tokio::time::sleep(SETTLE).await;

    let after_named_delete = lease_again(client, request, "substrate-after-named-delete").await?;
    assert!(
        client
            .read_file(&after_named_delete.lease_id, FILE.to_string(), None)
            .await
            .is_err(),
        "delete_named_sandbox must not leave the old actor's files behind"
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires a live Agent Substrate deployment with the substrate worker and SANDBOX_SMOKE_SUBSTRATE_IMAGE"]
async fn lease_lifecycle_via_broker_and_substrate_worker() -> TestResult {
    let image = stack_support::required_env("SANDBOX_SMOKE_SUBSTRATE_IMAGE")?;
    let tenant = TenantId::new(stack_support::env_or("SANDBOX_SMOKE_TENANT", "trogon-sandbox"))?;
    let name = LeaseName::new(stack_support::unique("smoke"))?;
    let mut request = stack_support::lease_request(
        tenant.clone(),
        ProviderKind::Substrate,
        SandboxSpec::new(stack_support::spec_parts(
            stack_support::registry_image(&image),
            60,
            2048,
        )?)?,
        600,
        "substrate-create",
    )?;
    request.name = Some(name.clone());

    let client = stack_support::connect().await?;
    let walked = lifecycle(&client, &tenant, &name, &request).await;
    // Always tear the name down, then judge the walk, so a failed step does not leave an actor behind.
    let deleted = client.delete_named_sandbox(tenant, name).await;
    walked?;
    deleted?;
    Ok(())
}
