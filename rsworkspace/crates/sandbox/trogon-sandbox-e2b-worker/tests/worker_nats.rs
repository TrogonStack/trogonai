mod authority_support;
mod worker_nats_support;

use authority_support::{Api, lease_request};
use serde_json::{Value, json};
use std::sync::Arc;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CancelIntent, DispatchAttemptId, DispatchTarget, LeaseAttemptIdentity, LeaseEpoch, LeaseId, ProviderCleanupRequest,
    ProviderKind, WorkerId,
};
use trogon_sandbox_protocol::{AttemptLedger, Provider, subjects};
use worker_nats_support::{Boxed, Running, verb};

async fn running() -> Result<(Running, Api), Boxed> {
    let api = Api::start().await?;
    let provider = api.provider()?;
    let lease = LeaseId::new("held-lease");
    provider.lease(&lease_request()?, &lease).await?;
    let running = Running::start(
        JetStreamTestServer::start().await,
        provider,
        lease,
        WorkerId::new("nats-worker")?,
    )
    .await?;
    Ok((running, api))
}

async fn answer(running: &Running, name: &str, body: Value) -> Result<Value, Boxed> {
    running
        .ask(&verb(name, &running.lease), &serde_json::to_vec(&body)?)
        .await?
        .ok_or_else(|| format!("no reply to {name}").into())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_verbs_that_the_provider_cannot_do_are_refused_for_a_held_lease() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let lease = running.lease.as_str().to_owned();

    let user = answer(&running, "create_user", json!({"lease_id": lease, "username": "bob"})).await?;
    assert!(user.to_string().contains("not supported"), "{user}");
    let group = answer(
        &running,
        "create_group",
        json!({"lease_id": lease, "groupname": "devs"}),
    )
    .await?;
    assert!(group.to_string().contains("not supported"), "{group}");
    let membership = answer(
        &running,
        "modify_group_membership",
        json!({"lease_id": lease, "username": "bob", "groupname": "devs", "action": "add"}),
    )
    .await?;
    assert!(membership.to_string().contains("not supported"), "{membership}");
    let policy = answer(
        &running,
        "network_policy_update",
        json!({"lease_id": lease, "network_policy": "deny_all"}),
    )
    .await?;
    assert!(policy.to_string().contains("create time"), "{policy}");

    let suspend = answer(
        &running,
        "suspend",
        json!({"lease_id": lease, "generation": 1, "level": "disk"}),
    )
    .await?;
    assert_eq!(suspend, json!({"outcome": "unsupported"}));
    let wake = answer(
        &running,
        "wake",
        json!({"lease_id": lease, "generation": 1, "level": "disk"}),
    )
    .await?;
    assert_eq!(wake, json!({"outcome": "unsupported"}));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_suspend_and_wake_follow_the_pause_state_of_the_sandbox() -> Result<(), Boxed> {
    let (running, api) = running().await?;
    let lease = running.lease.as_str().to_owned();
    let suspended = answer(
        &running,
        "suspend",
        json!({"lease_id": lease, "generation": 1, "level": "memory"}),
    )
    .await?;
    assert!(suspended.to_string().contains("done"), "{suspended}");
    assert_eq!(api.control.paused("vm-1"), Some(true));
    let woken = answer(
        &running,
        "wake",
        json!({"lease_id": lease, "generation": 1, "level": "memory"}),
    )
    .await?;
    assert!(woken.to_string().contains("done"), "{woken}");
    assert_eq!(api.control.paused("vm-1"), Some(false));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_running_needs_a_native_name() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let lease = running.lease.as_str().to_owned();
    let target = json!({"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"});
    let unnamed = answer(
        &running,
        "checkpoint_running",
        json!({"lease_id": lease, "target": target}),
    )
    .await?;
    assert!(unnamed.to_string().contains("provider-native"), "{unnamed}");
    let named = answer(
        &running,
        "checkpoint_running",
        json!({"lease_id": lease, "target": target, "native_name": "snap"}),
    )
    .await?;
    assert!(named.is_object(), "{named}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_command_verbs_answer_for_a_held_lease() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let lease = running.lease.as_str().to_owned();
    let stdin = answer(
        &running,
        "write_stdin",
        json!({"lease_id": lease, "execution_id": "e1", "seq": 0, "data": [], "eof": true}),
    )
    .await?;
    assert!(stdin.is_object() || stdin.is_string(), "{stdin}");
    let files = answer(&running, "write_files", json!({"lease_id": lease, "files": []})).await?;
    assert!(!files.is_null(), "{files}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verb_for_another_lease_gets_no_answer() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let subject = verb("native_handle", &running.lease);
    let other = serde_json::to_vec(&json!({"lease_id": "someone-elses"}))?;
    assert!(running.ask_silent(&subject, &other).await?.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verb_that_does_not_decode_gets_no_answer() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let subject = verb("create_user", &running.lease);
    assert!(running.ask_silent(&subject, b"not json").await?.is_none());
    let headless = serde_json::to_vec(&json!({"lease_id": running.lease}))?;
    assert!(running.ask_silent(&subject, &headless).await?.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verb_with_a_broken_transfer_gets_no_answer() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let subject = verb("native_handle", &running.lease);
    let body = serde_json::to_vec(&json!({"lease_id": running.lease}))?;
    assert!(running.ask_with_unknown_transfer(&subject, &body).await?.is_none());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verb_without_a_reply_subject_is_dropped_and_the_worker_keeps_serving() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let subject = verb("native_handle", &running.lease);
    let body = serde_json::to_vec(&json!({"lease_id": running.lease}))?;
    running.tell(&subject, &body).await?;
    assert!(running.ask(&subject, &body).await?.is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_launch_that_does_not_decode_gets_no_answer() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    for name in ["run_command", "cancel_command_launch"] {
        let subject = verb(name, &running.lease);
        assert!(running.ask_silent(&subject, b"not json").await?.is_none(), "{name}");
        running.tell(&subject, b"{}").await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsigned_launch_cancellation_is_denied() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let lease = running.lease.as_str().to_owned();
    let denied = answer(
        &running,
        "cancel_command_launch",
        json!({"lease_id": lease, "launch_key": "launch-1"}),
    )
    .await?;
    assert!(denied.to_string().contains("verified assignment authority"), "{denied}");
    let unanswerable = serde_json::to_vec(&json!({"lease_id": lease, "launch_key": "launch-1"}))?;
    running
        .tell(&verb("cancel_command_launch", &running.lease), &unanswerable)
        .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_native_snapshot_deletion_is_answered_or_refused() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let subject = subjects::provider_native_snapshot_delete(ProviderKind::E2b);
    let valid = serde_json::to_vec(&json!({
        "tenant": "authority-fixture",
        "reference": {"provider": "e2b", "id": "snap-1"},
    }))?;
    assert!(running.ask(&subject, &valid).await?.is_some());
    let refused = running.ask(&subject, b"not json").await?.ok_or("no refusal")?;
    assert!(
        refused.to_string().contains("invalid native snapshot delete request"),
        "{refused}"
    );
    running.tell(&subject, &valid).await?;
    assert!(running.ask(&subject, &valid).await?.is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_owned_cleanup_is_answered_only_on_the_subject_of_its_lease() -> Result<(), Boxed> {
    let (running, _api) = running().await?;
    let subject = subjects::provider_cancel_owned(ProviderKind::E2b, &running.lease);
    let held = serde_json::to_vec(&json!({
        "tenant": "authority-fixture",
        "lease_id": running.lease,
        "sandbox_id": "vm-1",
    }))?;
    assert!(running.ask(&subject, &held).await?.is_some());
    assert!(running.ask_silent(&subject, b"not json").await?.is_none());
    let elsewhere = serde_json::to_vec(&json!({
        "tenant": "authority-fixture",
        "lease_id": "someone-elses",
        "sandbox_id": "vm-9",
    }))?;
    assert!(running.ask_silent(&subject, &elsewhere).await?.is_none());
    running.tell(&subject, &held).await?;
    assert!(running.ask(&subject, &held).await?.is_some());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsigned_targeted_cleanup_is_refused_and_a_broken_one_is_dropped() -> Result<(), Boxed> {
    let api = Api::start().await?;
    let provider = api.provider()?;
    let directory = tempfile::tempdir()?;
    let ledger = Arc::new(AttemptLedger::open_for_adoption(
        directory.path().join("dispatch.sqlite"),
        ProviderKind::E2b,
        None,
    )?);
    let worker_id = ledger.worker_id().clone();
    provider.configure_dispatch_ledger(ledger)?;
    let lease = LeaseId::new("held-lease");
    provider.lease(&lease_request()?, &lease).await?;
    let running = Running::start(
        JetStreamTestServer::start().await,
        provider,
        lease.clone(),
        worker_id.clone(),
    )
    .await?;
    let subject = subjects::worker_cleanup(ProviderKind::E2b, &worker_id);
    let request = ProviderCleanupRequest {
        identity: LeaseAttemptIdentity {
            lease_id: lease,
            provider: ProviderKind::E2b,
            target: DispatchTarget {
                worker_id,
                attempt_id: DispatchAttemptId::new("cleanup-attempt")?,
                epoch: Some(LeaseEpoch::FIRST),
            },
        },
        intent: CancelIntent::Release,
    };
    let refused = running
        .ask(&subject, &serde_json::to_vec(&request)?)
        .await?
        .ok_or("no refusal")?;
    assert!(
        refused.to_string().contains("exact verified assignment authority"),
        "{refused}"
    );
    assert!(running.ask_silent(&subject, b"not json").await?.is_none());
    assert!(running.ask_with_unknown_transfer(&subject, b"{}").await?.is_none());
    running.tell(&subject, &serde_json::to_vec(&request)?).await?;
    Ok(())
}
