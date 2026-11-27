#![cfg(target_os = "linux")]

mod launch_scope_support;
mod native_command_support;
mod test_support;

use std::sync::atomic::Ordering;

use launch_scope_support::{configured, plain, verified};
use native_command_support::command_deadline_support::{TestResult, lease_sandbox, run_request};
use native_command_support::{Binding, Guest, NativeEnvd};
use serde_json::json;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, LaunchKey, LeaseId, ProviderKind, RunCommandRequest,
    RunCommandResponse,
};
use trogon_sandbox_e2b_worker::{E2bExec, LeaseSandbox};
use trogon_sandbox_protocol::subjects;

const LEASE: &str = "native-launch-cancellation";

fn command(key: Option<&str>, argv: &[&str]) -> TestResult<RunCommandRequest> {
    run_request(json!({
        "lease_id": LEASE, "command": argv,
        "detached": true, "deadline": "lease", "run_as_user": "root", "launch_key": key,
        "require_launch_cancellation": key.is_some(),
    }))
}

async fn run(exec: &E2bExec, sandbox: &LeaseSandbox, request: &RunCommandRequest) -> TestResult<RunCommandResponse> {
    let subject = subjects::provider_run_command(ProviderKind::E2b, &LeaseId::new(LEASE));
    let (scope, payload) = verified(request, &subject, LEASE)?;
    Ok(exec
        .handle_run_command_with_scope(sandbox, request, &scope, &payload)
        .await)
}

async fn cancel(
    exec: &E2bExec,
    sandbox: &LeaseSandbox,
    request: &CancelCommandLaunchRequest,
) -> TestResult<CancelCommandLaunchResponse> {
    let subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &LeaseId::new(LEASE));
    let (scope, payload) = verified(request, &subject, LEASE)?;
    Ok(exec
        .handle_cancel_command_launch_with_scope(sandbox, request, &scope, &payload)
        .await)
}

fn cancel_request(key: &str) -> TestResult<CancelCommandLaunchRequest> {
    Ok(CancelCommandLaunchRequest {
        lease_id: LeaseId::new(LEASE),
        launch_key: LaunchKey::new(key)?,
    })
}

async fn launch(
    exec: &E2bExec,
    sandbox: &LeaseSandbox,
    guest: &Guest,
    request: &RunCommandRequest,
) -> TestResult<Binding> {
    let response = run(exec, sandbox, request).await?;
    assert!(
        matches!(response, RunCommandResponse::Started(_)),
        "native launch must establish process ownership before losing its reply: {response:?}"
    );
    plain(guest.last_binding())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_start_reply_can_be_canceled_after_execution_registry_reconstruction() -> TestResult {
    let broker = test_support::LocalNats::start().await;
    let context = async_nats::jetstream::new(broker.client.clone());
    context.query_account().await?;
    let guest = plain(Guest::create())?;
    let envd = plain(NativeEnvd::start(guest.clone()).await)?;
    let observed = async {
        let sandbox = lease_sandbox(&envd.url, "owned-launch-cancellation-fixture")?;
        let exec = configured(context.clone())?;
        let sentinel = launch(&exec, &sandbox, &guest, &command(None, &["sleep", "60"])?).await?;
        let target = launch(
            &exec,
            &sandbox,
            &guest,
            &command(Some("lost-start"), &["bash", "-c", "sleep 60 & wait"])?,
        )
        .await?;
        let live = plain(guest.group_members(target))?;
        let allocations = guest.bootstrap_calls.load(Ordering::SeqCst);
        drop(exec);
        let rebuilt = configured(context.clone())?;
        let request = cancel_request("lost-start")?;
        let canceled = cancel(&rebuilt, &sandbox, &request).await?;
        let remaining = plain(guest.group_members(target))?;
        let unrelated = plain(guest.group_members(sentinel))?;
        drop(rebuilt);
        let replay = cancel(&configured(context.clone())?, &sandbox, &request).await?;
        let final_allocations = guest.bootstrap_calls.load(Ordering::SeqCst);
        Ok::<_, Box<dyn std::error::Error>>((
            live,
            canceled,
            replay,
            remaining,
            unrelated,
            allocations,
            final_allocations,
        ))
    }
    .await;
    envd.shutdown().await;
    plain(guest.cleanup())?;
    let (live, canceled, replay, remaining, unrelated, allocations, final_allocations) = observed?;
    assert!(
        !live.is_empty(),
        "the exact native target must be live before cancellation"
    );
    assert!(
        allocations == 2,
        "the fixture must count the sentinel and controlled bootstrap exactly"
    );
    let canceled_with_execution = matches!(
        canceled,
        CancelCommandLaunchResponse::Canceled { execution_id: Some(_) }
    );
    assert!(
        canceled_with_execution && remaining.is_empty(),
        "a lost start acknowledgement must retain exact native cancellation authority through reconstruction: response={canceled:?}, remaining={remaining:?}"
    );
    assert!(
        canceled == replay,
        "a lost cancellation reply must replay the same settled authority: first={canceled:?}, replay={replay:?}"
    );
    assert!(
        !unrelated.is_empty(),
        "cancellation by launch key must preserve the unrelated live sentinel"
    );
    assert!(
        allocations == final_allocations,
        "reconciliation must not dispatch another native process"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_before_start_retains_a_no_effect_fence_after_execution_registry_reconstruction() -> TestResult {
    let broker = test_support::LocalNats::start().await;
    let context = async_nats::jetstream::new(broker.client.clone());
    context.query_account().await?;
    let guest = plain(Guest::create())?;
    let envd = plain(NativeEnvd::start(guest.clone()).await)?;
    let observed = async {
        let sandbox = lease_sandbox(&envd.url, "owned-launch-cancellation-fixture")?;
        let exec = configured(context.clone())?;
        let sentinel = launch(&exec, &sandbox, &guest, &command(None, &["sleep", "60"])?).await?;
        let canceled = cancel(&exec, &sandbox, &cancel_request("cancel-first")?).await?;
        drop(exec);
        let rebuilt = configured(context.clone())?;
        let effect = guest.path("cancel-first-effect");
        let script = "printf forbidden >\"$1\"; sleep 60";
        let argv = ["bash", "-c", script, "effect", &effect.to_string_lossy()];
        let request = command(Some("cancel-first"), &argv)?;
        let first = run(&rebuilt, &sandbox, &request).await?;
        let replay = run(&rebuilt, &sandbox, &request).await?;
        let allocations = guest.bootstrap_calls.load(Ordering::SeqCst);
        let written = plain(guest.effect("cancel-first-effect"))?;
        let unrelated = plain(guest.group_members(sentinel))?;
        Ok::<_, Box<dyn std::error::Error>>((canceled, first, replay, allocations, written, unrelated))
    }
    .await;
    envd.shutdown().await;
    plain(guest.cleanup())?;
    let (canceled, first, replay, allocations, written, unrelated) = observed?;
    assert!(
        canceled == CancelCommandLaunchResponse::Canceled { execution_id: None }
            && matches!(first, RunCommandResponse::Denied(_))
            && matches!(replay, RunCommandResponse::Denied(_))
            && allocations == 1
            && written.is_none(),
        "a durable cancellation arriving before its launch must prevent all later user effects: canceled={canceled:?}, first={first:?}, replay={replay:?}, allocations={allocations}, effect={written:?}"
    );
    assert!(
        !unrelated.is_empty(),
        "the unrelated sentinel must survive cancel-before-start"
    );
    Ok(())
}
