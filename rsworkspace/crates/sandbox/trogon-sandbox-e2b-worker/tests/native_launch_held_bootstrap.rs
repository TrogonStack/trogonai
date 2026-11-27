#![cfg(target_os = "linux")]

mod held_bootstrap_support;
mod launch_scope_support;
mod native_command_support;
mod test_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use held_bootstrap_support::HeldBootstrap;
use launch_scope_support::{TENANT, configured, plain, verified};
use native_command_support::command_deadline_support::{TestResult, lease_sandbox, run_request};
use native_command_support::{Guest, NativeEnvd};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, LaunchKey, LeaseId, ProviderKind, RunCommandResponse,
    TenantId,
};
use trogon_sandbox_protocol::{subjects, tenant_scoped_kv_key};

const LEASE: &str = "held-bootstrap-lease";

/// Whether the cancellation intent became durable while the launch was still preparing, within two seconds.
async fn durable_intent_before_release(store: &async_nats::jetstream::kv::Store, key: &str) -> TestResult<bool> {
    let observed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let bytes = store.get(key).await?.ok_or("launch custody disappeared")?;
            let record: Value = serde_json::from_slice(&bytes)?;
            assert!(
                record["phase"] == "preparing" && record["execution"].is_null(),
                "a held native bootstrap must retain pending identity custody"
            );
            if record["cancel_requested"] == true {
                return Ok::<(), Box<dyn std::error::Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    Ok(observed.ok().transpose()?.is_some())
}

fn cancel_request() -> TestResult<CancelCommandLaunchRequest> {
    Ok(CancelCommandLaunchRequest {
        lease_id: LeaseId::new(LEASE),
        launch_key: LaunchKey::new("held-start")?,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_intent_is_durable_while_the_native_bootstrap_reply_is_held() -> TestResult {
    let broker = test_support::LocalNats::start().await;
    let context = async_nats::jetstream::new(broker.client.clone());
    context.query_account().await?;
    let guest = plain(Guest::create())?;
    let envd = plain(NativeEnvd::start(guest.clone()).await)?;
    let proxy = HeldBootstrap::start(envd.url.clone()).await?;
    let observed = async {
        let sandbox = lease_sandbox(&proxy.url, "owned-held-bootstrap-fixture")?;
        let direct = lease_sandbox(&envd.url, "owned-held-bootstrap-fixture")?;
        let exec = Arc::new(configured(context.clone())?);
        let sentinel_body = json!({
            "lease_id": LEASE, "command": ["sleep", "60"],
            "detached": true, "deadline": "lease", "run_as_user": "root",
        });
        let sentinel = run_request(sentinel_body)?;
        let sentinel_started = exec.handle_run_command(&direct, &sentinel).await;
        let sentinel = plain(guest.last_binding())?;
        let effect = guest.path("held-bootstrap-effect");
        let request_body = json!({
            "lease_id": LEASE, "launch_key": "held-start",
            "command": ["bash", "-c", "printf forbidden >\"$1\"; sleep 60", "effect", effect.to_string_lossy()],
            "require_launch_cancellation": true,
            "detached": true, "deadline": "lease", "run_as_user": "root",
        });
        let request = run_request(request_body)?;
        let run_subject = subjects::provider_run_command(ProviderKind::E2b, &LeaseId::new(LEASE));
        let (scope, payload) = verified(&request, &run_subject, LEASE)?;
        let start_exec = Arc::clone(&exec);
        let start_sandbox = sandbox.clone();
        let start = tokio::spawn(async move {
            start_exec
                .handle_run_command_with_scope(&start_sandbox, &request, &scope, &payload)
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), proxy.state.reached.notified()).await?;
        let target = plain(guest.last_binding())?;
        let control = guest.bootstrap_calls.load(Ordering::SeqCst) == 2
            && target.pid != sentinel.pid
            && !plain(guest.group_members(target))?.is_empty();
        let prior_effect = plain(guest.effect("held-bootstrap-effect"))?;
        let store = context.get_key_value("sandbox-e2b-command-custody").await?;
        let key = tenant_scoped_kv_key(&TenantId::new(TENANT)?, &serde_json::to_string(&(LEASE, "held-start"))?);
        let cancel_subject = subjects::provider_cancel_command_launch(ProviderKind::E2b, &LeaseId::new(LEASE));
        let cancel_body = cancel_request()?;
        let (scope, payload) = verified(&cancel_body, &cancel_subject, LEASE)?;
        let cancel_exec = Arc::clone(&exec);
        let cancel_sandbox = sandbox.clone();
        let cancel = tokio::spawn(async move {
            cancel_exec
                .handle_cancel_command_launch_with_scope(&cancel_sandbox, &cancel_body, &scope, &payload)
                .await
        });
        let intent = durable_intent_before_release(&store, &key).await;
        proxy.state.release.cancel();
        let started = tokio::time::timeout(Duration::from_secs(5), start).await??;
        let canceled = tokio::time::timeout(Duration::from_secs(5), cancel).await??;
        let intent = intent?;
        let (scope, payload) = verified(&cancel_request()?, &cancel_subject, LEASE)?;
        let settled = exec
            .handle_cancel_command_launch_with_scope(&sandbox, &cancel_request()?, &scope, &payload)
            .await;
        let written = plain(guest.effect("held-bootstrap-effect"))?;
        let remaining = plain(guest.group_members(target))?;
        let unrelated = plain(guest.group_members(sentinel))?;
        let bootstraps = guest.bootstrap_calls.load(Ordering::SeqCst);
        Ok::<_, Box<dyn std::error::Error>>((
            matches!(sentinel_started, RunCommandResponse::Started(_)) && control && prior_effect.is_none(),
            intent,
            started,
            canceled,
            settled,
            written.is_none(),
            remaining,
            unrelated,
            bootstraps,
        ))
    }
    .await;
    proxy.shutdown().await;
    envd.shutdown().await;
    plain(guest.cleanup())?;
    let (control, intent, started, canceled, settled, no_effect, remaining, unrelated, bootstraps) = observed?;
    assert!(
        control,
        "positive control must establish a second exact live native group behind the release gate"
    );
    assert!(
        intent && matches!(started, RunCommandResponse::Denied(_)) && no_effect,
        "cancellation intent must persist before the held bootstrap reply is released and fence user effects: intent_before_release={intent}, start={started:?}, cancel={canceled:?}, no_effect={no_effect}"
    );
    assert!(
        matches!(settled, CancelCommandLaunchResponse::Canceled { execution_id: Some(_) }) && remaining.is_empty(),
        "the exact native target must settle after the pending bootstrap is released: {settled:?}, remaining={remaining:?}"
    );
    assert!(
        !unrelated.is_empty(),
        "the unrelated sentinel must survive cancellation"
    );
    assert!(
        bootstraps == 2,
        "cancellation must not bootstrap another native command"
    );
    Ok(())
}
