//! Provider-agnostic lease, run and release against a live stack.
//!
//! `SANDBOX_SMOKE_PROVIDER` names the provider in its wire spelling (`daytona`, `modal`, `local_docker`, ...). The
//! test fails when it is unset, so a misconfigured scheduled job cannot pass by skipping. `SANDBOX_SMOKE_IMAGE`
//! overrides the image (default `alpine:3.20`). See `stack_support` for how the lane finds NATS and which tenant it
//! leases under.

mod common;
mod stack_support;

use std::time::Duration;

use common::TestResult;
use trogon_sandbox_client::CommandOptions;
use trogon_sandbox_contract::{ProviderKind, SandboxSpec};

#[tokio::test]
#[ignore = "requires a live stack and SANDBOX_SMOKE_PROVIDER naming the provider under test"]
async fn lease_run_and_release_on_the_named_provider() -> TestResult {
    let provider_name = stack_support::required_env("SANDBOX_SMOKE_PROVIDER")?;
    let provider: ProviderKind = serde_json::from_value(serde_json::Value::String(provider_name.clone()))
        .map_err(|error| format!("unknown provider {provider_name}: {error}"))?;
    let image = stack_support::env_or("SANDBOX_SMOKE_IMAGE", "alpine:3.20");
    let tenant = stack_support::smoke_tenant()?;
    let request = stack_support::lease_request(
        tenant.clone(),
        provider,
        SandboxSpec::new(stack_support::spec_parts(
            stack_support::registry_image(&image),
            1,
            128,
        )?)?,
        300,
        &format!("live-{provider_name}"),
    )?;

    let client = stack_support::connect().await?;
    let granted = client.lease(&request, Duration::from_secs(180)).await?;
    assert_eq!(granted.provider, provider);

    let options = CommandOptions {
        timeout_seconds: 60,
        ..CommandOptions::default()
    };
    let ran = client
        .run_command(
            &granted.lease_id,
            vec!["echo".to_string(), "live-ok".to_string()],
            options,
        )
        .await;

    // Always release, then judge the command, so a failing run does not leak.
    client.delete_lease(tenant, &granted.lease_id).await?;

    let output = ran?;
    assert_eq!(output.exit_code, 0);
    assert!(stack_support::stdout(&output).contains("live-ok"));
    Ok(())
}
