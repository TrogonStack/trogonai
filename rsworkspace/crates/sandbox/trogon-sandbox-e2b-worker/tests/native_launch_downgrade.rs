#![cfg(target_os = "linux")]

mod native_command_support;
mod native_storage_support;
mod test_support;

use std::sync::atomic::Ordering;

use native_command_support::{Guest, NativeEnvd};
use native_storage_support::{Boxed, Signing, TestResult, exec, lease_sandbox, run_request, scope};
use serde_json::json;
use test_support::LocalNats;
use trogon_sandbox_contract::{LeaseId, ProviderKind, RunCommandResponse};
use trogon_sandbox_protocol::subjects;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_weaker_retry_cannot_bypass_a_strong_launchs_durable_no_replay_fence() -> TestResult {
    let broker = LocalNats::start().await;
    let context = async_nats::jetstream::new(broker.client.clone());
    context.query_account().await?;
    let guest = Guest::create()?;
    let envd = NativeEnvd::start(guest.clone()).await?;
    let observed = async {
        let sandbox = lease_sandbox(&envd.url, "owned-downgrade-fixture")?;
        let exec = exec()?;
        exec.configure_command_launches(context.clone())?;
        let signing = Signing::new("downgrade-fixture", 9)?;
        let scope = scope(
            "downgrade-tenant",
            "downgrade-lease",
            "downgrade-worker",
            "downgrade-attempt",
        )?;
        let subject = subjects::provider_run_command(ProviderKind::E2b, &LeaseId::new("downgrade-lease"));
        let effect = guest.path("downgrade-effect");
        let request = run_request(json!({
            "lease_id": "downgrade-lease", "launch_key": "owned-launch", "require_launch_cancellation": true,
            "command": ["sleep", "60"], "detached": true, "deadline": "lease", "run_as_user": "root",
        }))?;
        let (verified, payload) = signing.verify(&subject, &request, &scope)?;
        let first = exec
            .handle_run_command_with_scope(&sandbox, &request, &verified, &payload)
            .await;
        let original = guest.last_binding()?;
        let allocations = guest.bootstrap_calls.load(Ordering::SeqCst);
        let established = allocations == 1 && !guest.group_members(original)?.is_empty();
        let weaker = run_request(json!({
            "lease_id": "downgrade-lease", "launch_key": "owned-launch", "require_launch_cancellation": false,
            "command": ["bash", "-c", "printf forbidden >\"$1\"; sleep 60", "downgrade", effect.to_string_lossy()],
            "detached": true, "deadline": "lease", "run_as_user": "root",
        }))?;
        let (verified, payload) = signing.verify(&subject, &weaker, &scope)?;
        let response = exec
            .handle_run_command_with_scope(&sandbox, &weaker, &verified, &payload)
            .await;
        let remaining = guest.group_members(original)?;
        let final_allocations = guest.bootstrap_calls.load(Ordering::SeqCst);
        Ok::<_, Boxed>((
            first,
            established,
            response,
            guest.effect("downgrade-effect")?,
            remaining,
            allocations,
            final_allocations,
        ))
    }
    .await;
    envd.shutdown().await;
    guest.cleanup()?;
    let (first, established, response, effect, remaining, allocations, final_allocations) = observed?;
    assert!(
        matches!(first, RunCommandResponse::Started(_)) && established,
        "the positive controlled launch must start exactly one real native group: {first:?}"
    );
    assert!(
        matches!(response, RunCommandResponse::Denied(_)) && effect.is_none() && allocations == final_allocations,
        "disabling a stronger flag must not turn an existing launch key into new effect authority: response={response:?}, effect={effect:?}, before={allocations}, after={final_allocations}"
    );
    assert!(
        !remaining.is_empty(),
        "refusing a downgraded request must preserve the existing owned command"
    );
    Ok(())
}
