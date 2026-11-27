mod support;

use std::time::Duration;

use serde_json::json;
use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, LeaseError, LeaseId, LifecycleEvent, ProviderKind, RunCommandResponse,
    WorkerId,
};
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityResourceId, FreezeOutcome, HeldLease, PurgeOutcome, ThawOutcome,
};
use trogon_sandbox_v8_wasm_worker::V8WasmProvider;

use support::lease::{Lease, command_request, eval_request, provider};
use support::{TestResult, cpu_permit};
use trogon_sandbox_protocol::{LifecycleEmitter, Provider};

async fn persistent_lease(
    provider: &V8WasmProvider,
    id: &str,
    script: &str,
) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), script)?;
    let request = Lease::mounted(dir.path(), "js").persistent().build()?;
    provider.lease(&request, &LeaseId::new(id)).await?;
    Ok(dir)
}

fn completed(response: Option<RunCommandResponse>) -> Result<CommandOutput, Box<dyn std::error::Error>> {
    match response {
        Some(RunCommandResponse::Completed(output)) => Ok(output),
        other => Err(format!("expected a completed command, got {other:?}").into()),
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn a_persistent_lease_shares_state_across_commands_until_cancelled() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let _dir = persistent_lease(&provider, "lease-persist", "globalThis.counter = 40;").await?;
    let bump = eval_request("lease-persist", "counter += 1; console.log(counter);")?;

    let first = completed(provider.run_session_command(&bump).await)?;
    let second = completed(provider.run_session_command(&bump).await)?;
    let failing = completed(
        provider
            .run_session_command(&eval_request("lease-persist", "throw new Error('boom');")?)
            .await,
    )?;

    assert_eq!((first.exit_code, text(&first.stdout).contains("41")), (0, true));
    assert!(text(&second.stdout).contains("42"));
    assert_eq!(failing.exit_code, 1);
    assert!(text(&failing.stderr).contains("boom"));
    assert_eq!(provider.active_lease_count().await, 1);
    provider.cancel(&LeaseId::new("lease-persist")).await?;
    assert!(provider.run_session_command(&bump).await.is_none());
    Ok(())
}

#[tokio::test]
async fn a_command_that_outlives_its_timeout_reports_the_timeout() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let _dir = persistent_lease(&provider, "lease-slow", "1;").await?;
    let request = command_request(
        "lease-slow",
        json!({ "command": ["eval", "while (true) {}"], "timeout_seconds": 1 }),
    )?;

    let output = completed(provider.run_session_command(&request).await)?;

    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    assert_eq!(output.exit_code, CommandOutput::TIMED_OUT_EXIT_CODE);
    Ok(())
}

#[tokio::test]
async fn command_output_is_capped_at_the_requested_size() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let _dir = persistent_lease(&provider, "lease-cap", "1;").await?;
    let request = command_request(
        "lease-cap",
        json!({ "command": ["eval", "console.log('x'.repeat(1000)); console.error('y'.repeat(1000));"], "max_output_bytes": 16 }),
    )?;

    let output = completed(provider.run_session_command(&request).await)?;

    assert!(output.stdout.len() < 1000 && output.stderr.len() < 1000);
    assert!(output.truncated.is_truncated());
    Ok(())
}

#[tokio::test]
async fn a_lease_without_a_persistent_runtime_has_no_session() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), "globalThis.x = 1;")?;
    let (_, provider) = provider();
    provider
        .lease(
            &Lease::mounted(dir.path(), "js").build()?,
            &LeaseId::new("lease-oneshot"),
        )
        .await?;

    assert!(
        provider
            .run_session_command(&eval_request("lease-oneshot", "1")?)
            .await
            .is_none()
    );
    assert!(
        provider
            .run_session_command(&eval_request("no-such-lease", "1")?)
            .await
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn malformed_session_commands_are_denied() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let _dir = persistent_lease(&provider, "lease-bad", "1;").await?;
    let malformed = [
        json!({ "command": ["sh", "-c", "ls"] }),
        json!({ "command": ["eval", "1"], "detached": true }),
        json!({ "command": ["eval", "1"], "cwd": "/tmp" }),
        json!({ "command": ["eval", "1"], "env": { "A": "b" } }),
        json!({ "command": ["eval", "1"], "stream_output": true }),
        json!({ "command": ["eval", "1"], "run_as_user": "someone" }),
        json!({ "command": ["eval", "1"], "require_launch_cancellation": true, "launch_key": "launch-1" }),
    ];

    for fields in malformed {
        let denied = provider
            .run_session_command(&command_request("lease-bad", fields.clone())?)
            .await;
        assert!(
            matches!(denied, Some(RunCommandResponse::Denied(LeaseError::InvalidSpec { .. }))),
            "{fields}: {denied:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn cancel_ends_an_eval_that_still_holds_the_session() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let _dir = persistent_lease(&provider, "lease-held", "1;").await?;
    let id = LeaseId::new("lease-held");
    let request = command_request(
        "lease-held",
        json!({ "command": ["eval", "while (true) {}"], "timeout_seconds": 30 }),
    )?;
    let eval = provider.run_session_command(&request);
    tokio::pin!(eval);
    assert!(futures::poll!(eval.as_mut()).is_pending());

    provider.cancel(&id).await?;

    let output = completed(tokio::time::timeout(Duration::from_secs(10), eval).await?)?;
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(output.exit_code, -1);
    assert!(
        provider
            .run_session_command(&eval_request("lease-held", "1")?)
            .await
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn a_command_on_a_cancelled_but_unsettled_lease_is_denied() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let _dir = persistent_lease(&provider, "lease-cancelling", "1;").await?;
    let slow = command_request(
        "lease-cancelling",
        json!({ "command": ["eval", "while (true) {}"], "timeout_seconds": 30 }),
    )?;
    let eval = provider.run_session_command(&slow);
    tokio::pin!(eval);
    assert!(futures::poll!(eval.as_mut()).is_pending());
    let cancelling = LeaseId::new("lease-cancelling");
    let cancel = provider.cancel(&cancelling);
    tokio::pin!(cancel);
    assert!(futures::poll!(cancel.as_mut()).is_pending());

    let denied = provider
        .run_session_command(&eval_request("lease-cancelling", "1")?)
        .await;

    assert!(matches!(
        denied,
        Some(RunCommandResponse::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    cancel.await?;
    completed(eval.await)?;
    Ok(())
}

#[tokio::test]
async fn the_authority_host_purges_the_exact_runtime_and_not_a_replacement() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let id = LeaseId::new("owned-runtime");
    let _first = persistent_lease(&provider, id.as_str(), "globalThis.counter = 41;").await?;
    let held = provider.held_leases().await?.remove(0);
    assert_eq!(provider.freeze(&held).await?, FreezeOutcome::Unsupported);
    assert_eq!(provider.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    let slow = command_request(
        id.as_str(),
        json!({ "command": ["eval", "while (true) {}"], "timeout_seconds": 30 }),
    )?;
    let eval = provider.run_session_command(&slow);
    tokio::pin!(eval);
    assert!(futures::poll!(eval.as_mut()).is_pending());

    assert_eq!(
        AuthorityHost::purge(provider.as_ref(), &held).await?,
        PurgeOutcome::Purged
    );

    assert_eq!(completed(eval.await)?.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert!(provider.held_leases().await?.is_empty());
    assert_eq!(provider.thaw(&held).await?, ThawOutcome::NoLongerOwned);
    let _second = persistent_lease(&provider, id.as_str(), "globalThis.counter = 100;").await?;
    assert_eq!(
        AuthorityHost::purge(provider.as_ref(), &held).await?,
        PurgeOutcome::Settled
    );
    let HeldLease::Provider { resource_id, .. } = &held else {
        return Err("expected a provider lease".into());
    };
    let forged = HeldLease::Provider {
        lease_id: id.clone(),
        provider: ProviderKind::V8Wasm,
        resource_id: AuthorityResourceId::new(format!("{}-forged", resource_id.as_str()))?,
    };
    assert_eq!(
        AuthorityHost::purge(provider.as_ref(), &forged).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(provider.freeze(&forged).await?, FreezeOutcome::Stale);
    assert_eq!(provider.thaw(&forged).await?, ThawOutcome::Stale);
    let output = completed(
        provider
            .run_session_command(&eval_request(id.as_str(), "console.log(counter);")?)
            .await,
    )?;
    assert!(text(&output.stdout).contains("100"));
    provider.cancel(&id).await?;
    Ok(())
}

#[tokio::test]
async fn the_authority_host_ignores_other_providers_leases() -> TestResult {
    let (_, provider) = provider();
    let other = HeldLease::Provider {
        lease_id: LeaseId::new("elsewhere"),
        provider: ProviderKind::Mock,
        resource_id: AuthorityResourceId::new("resource-1")?,
    };
    let unrelated = HeldLease::Provider {
        lease_id: LeaseId::new("elsewhere"),
        provider: ProviderKind::V8Wasm,
        resource_id: AuthorityResourceId::new("resource-1")?,
    };

    assert_eq!(provider.freeze(&other).await?, FreezeOutcome::Stale);
    assert_eq!(provider.thaw(&other).await?, ThawOutcome::Stale);
    assert_eq!(
        AuthorityHost::purge(provider.as_ref(), &other).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(
        AuthorityHost::purge(provider.as_ref(), &unrelated).await?,
        PurgeOutcome::Stale
    );
    assert_eq!(provider.freeze(&unrelated).await?, FreezeOutcome::Stale);
    assert_eq!(provider.thaw(&unrelated).await?, ThawOutcome::Stale);
    Ok(())
}

#[tokio::test]
async fn shutdown_stops_every_guest_still_held() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let _first = persistent_lease(&provider, "held-1", "1;").await?;
    let _second = persistent_lease(&provider, "held-2", "1;").await?;
    assert_eq!(provider.active_lease_count().await, 2);

    provider.shutdown().await?;

    assert_eq!(provider.active_lease_count().await, 0);
    assert!(
        provider
            .run_session_command(&eval_request("held-1", "1")?)
            .await
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn binding_a_worker_is_remembered_in_the_grant() -> TestResult {
    let _permit = cpu_permit().await?;
    let (_, provider) = provider();
    let worker = WorkerId::new("worker-bound")?;
    provider.bind_worker(&worker)?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), "1;")?;

    let grant = provider
        .lease(&Lease::mounted(dir.path(), "js").build()?, &LeaseId::new("lease-bound"))
        .await?;

    assert_eq!(grant.worker_id, Some(worker));
    Ok(())
}

/// Holds the lease open after its script finished, so a caller can abandon it at that point.
struct SlowCompletion;

#[async_trait::async_trait]
impl LifecycleEmitter for SlowCompletion {
    async fn emit(&self, event: LifecycleEvent) {
        if matches!(event, LifecycleEvent::Completed { .. }) {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }
}

#[tokio::test]
async fn a_persistent_lease_abandoned_after_its_script_ran_releases_its_runtime() -> TestResult {
    let _permit = cpu_permit().await?;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("index.js"), "globalThis.ready = true;")?;
    let provider = V8WasmProvider::with_emitter(std::sync::Arc::new(SlowCompletion));
    let request = Lease::mounted(dir.path(), "js").persistent().build()?;

    let abandoned = tokio::time::timeout(
        Duration::from_millis(200),
        provider.lease(&request, &LeaseId::new("lease-abandoned")),
    )
    .await;
    let released = tokio::time::timeout(Duration::from_secs(10), async {
        while provider.active_lease_count().await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;

    assert!(abandoned.is_err());
    assert!(released.is_ok());
    Ok(())
}
