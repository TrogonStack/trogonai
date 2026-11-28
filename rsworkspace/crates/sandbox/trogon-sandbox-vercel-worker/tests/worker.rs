mod nats_support;
mod support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_nats::client::{PublishErrorKind, SubscribeErrorKind};
use futures::StreamExt;
use nats_support::{Fake, Nats, Running, SILENCE, dispatch_json, request_json, until};
use serde_json::{Value, json};
use support::{Boxed, TestResult};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    LeaseError, LeaseId, LeaseRequest, LifecycleEvent, SnapshotExportError, SnapshotExportOutcome, WorkerId,
};
use trogon_sandbox_observability::ObservabilityError;
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, NatsAuthorityOracle, NatsConnectError, NatsTransferError, Provider,
    lifecycle_subject, subjects,
};
use trogon_sandbox_vercel_worker::{VercelApiError, VercelProviderError, VercelWorkerError};

const LEASE: &str = "lease-1";

/// Every per-lease verb with a request that names no lease, and the outcome a held lease answers with.
fn every_verb() -> Vec<(&'static str, Value, &'static str)> {
    let target = json!({"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"});
    vec![
        (
            "run_command",
            json!({"command": ["echo", "hi"], "timeout_seconds": 5}),
            "denied",
        ),
        ("poll_command", json!({"execution_id": "exec-1"}), "not_found"),
        (
            "kill_command",
            json!({"execution_id": "exec-1", "signal": "kill"}),
            "not_found",
        ),
        (
            "write_stdin",
            json!({"execution_id": "exec-1", "seq": 0, "data": [1], "eof": false}),
            "unsupported",
        ),
        (
            "write_files",
            json!({"files": [{"path": "/tmp/a", "content": [104, 105]}]}),
            "written",
        ),
        ("read_file", json!({"path": "/tmp/a"}), "content"),
        ("file_op", json!({"op": {"op": "stat", "path": "/"}}), "unsupported"),
        ("get_domain", json!({"port": 8080}), "available"),
        ("pause", json!({}), "done"),
        ("resume", json!({}), "done"),
        ("suspend", json!({"generation": 1, "level": "disk"}), "done"),
        ("wake", json!({"generation": 1, "level": "disk"}), "done"),
        ("native_handle", json!({}), "available"),
        (
            "checkpoint_running",
            json!({"target": target, "native_name": "snap"}),
            "native",
        ),
        ("network_policy_update", json!({"network_policy": "deny_all"}), "denied"),
        ("create_user", json!({"username": "alice"}), "denied"),
        ("create_group", json!({"groupname": "staff"}), "denied"),
        (
            "modify_group_membership",
            json!({"username": "alice", "groupname": "staff", "action": "add"}),
            "denied",
        ),
    ]
}

fn verb_subject(verb: &str, lease: &str) -> String {
    format!("sandbox.provider.vercel.{verb}.{lease}")
}

fn granted_sandbox(reply: &Value) -> Result<&str, Boxed> {
    Ok(reply["sandbox_id"].as_str().ok_or("the lease was not granted")?)
}

async fn events(subscription: &mut async_nats::Subscriber, count: usize) -> Result<Vec<LifecycleEvent>, Boxed> {
    let mut events = Vec::new();
    while events.len() < count {
        let message = tokio::time::timeout(Duration::from_secs(10), subscription.next())
            .await?
            .ok_or("the subscription ended")?;
        events.push(serde_json::from_slice(&message.payload)?);
    }
    Ok(events)
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_grants_a_lease_and_creates_a_sandbox() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running.dispatch(LEASE).await?;
    assert_eq!(granted_sandbox(&reply)?, "sbx-1");
    assert_eq!(reply["worker_id"], "worker-1");
    assert_eq!(running.fake.creates(), 1);
    assert_eq!(running.provider.active_lease_count().await, 1);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redelivered_dispatch_returns_the_first_grant() -> TestResult {
    let mut running = Running::start().await?;
    let first = running.dispatch(LEASE).await?;
    let second = running.dispatch(LEASE).await?;
    assert_eq!(first, second);
    assert_eq!(running.fake.creates(), 1);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_lease_id_launches_again_when_reused() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| fake.deletes() == 1).await?;
    let reply = running.dispatch(LEASE).await?;
    assert_eq!(granted_sandbox(&reply)?, "sbx-2");
    assert_eq!(running.fake.creates(), 2);
    running.stop().await
}

/// Pins today's behaviour, pending a decision on which one every worker should have: the cancel handler forgets the
/// grant before it cancels, so a redispatch after a failed cancel is not answered with the grant. The provider still
/// holds the sandbox it could not delete, so it refuses to launch another and the redispatch gets a provider error.
/// The k8s, v8-wasm and kubevirt workers keep the grant until a cancel succeeds and answer with it instead.
#[tokio::test(flavor = "multi_thread")]
async fn a_redispatch_after_a_failed_cancel_is_refused_while_the_sandbox_is_held() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.fake.delete_status(500);

    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| fake.deletes() == 1).await?;
    let reply = running.dispatch(LEASE).await?;

    assert!(
        reply["provider_error"]
            .as_str()
            .is_some_and(|error| error.contains("settle cleanup before reallocating")),
        "{reply}"
    );
    assert_eq!(running.fake.creates(), 1, "no second sandbox was created");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_that_arrives_during_a_launch_waits_for_it_and_releases_the_sandbox() -> TestResult {
    let mut running = Running::start().await?;
    running.fake.create_delay(Duration::from_millis(800));
    let fake = &running.fake;
    let (reply, cancelled) = tokio::join!(running.dispatch(LEASE), async {
        until(|| fake.creates() == 1).await?;
        running.cancel_lease(LEASE).await
    });
    cancelled?;
    assert_eq!(granted_sandbox(&reply?)?, "sbx-1");
    until(|| fake.deletes() == 1).await?;
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_release_leaves_the_lease_held() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.fake.delete_status(500);
    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| fake.deletes() == 1).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(running.provider.active_lease_count().await, 1);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_undecodable_dispatch_is_rejected() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running
        .ask("sandbox.provider.vercel.lease", &json!({"unexpected": true}))
        .await?;
    assert!(reply["decode_error"].is_string(), "{reply}");
    running
        .publish("sandbox.provider.vercel.lease".to_owned(), &json!({"unexpected": true}))
        .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(running.fake.creates(), 0);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_failure_is_reported_in_the_reply() -> TestResult {
    let mut running = Running::start().await?;
    running.fake.create_status(500);
    let reply = running.dispatch(LEASE).await?;
    assert!(reply["provider_error"].is_string(), "{reply}");
    assert_eq!(running.provider.active_lease_count().await, 0);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    running
        .publish("sandbox.provider.vercel.lease".to_owned(), &dispatch_json(LEASE))
        .await?;
    let fake = &running.fake;
    until(|| fake.creates() == 1).await?;
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_id_the_bus_cannot_publish_events_on_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running.dispatch("lease with spaces").await?;
    assert_eq!(granted_sandbox(&reply)?, "sbx-1");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_subject_the_worker_cannot_publish_to_does_not_stop_it() -> TestResult {
    let mut running = Running::start().await?;
    let sender = running.nats.unchecked_client().await?;
    sender
        .publish_with_reply(
            "sandbox.provider.vercel.lease",
            "reply..empty",
            serde_json::to_vec(&dispatch_json(LEASE))?.into(),
        )
        .await?;
    sender.flush().await?;
    let fake = &running.fake;
    until(|| fake.creates() == 1).await?;
    let reply = running.dispatch("lease-2").await?;
    assert_eq!(granted_sandbox(&reply)?, "sbx-2");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_publishes_its_lifecycle_evidence() -> TestResult {
    let mut running = Running::start().await?;
    let mut subscription = running
        .client
        .subscribe(lifecycle_subject(&LeaseId::new(LEASE)))
        .await?;
    running.dispatch(LEASE).await?;
    let events = events(&mut subscription, 4).await?;
    assert!(
        matches!(
            events.as_slice(),
            [
                LifecycleEvent::Started { .. },
                LifecycleEvent::PhaseStarted { .. },
                LifecycleEvent::PhaseCompleted { .. },
                LifecycleEvent::Completed { .. },
            ]
        ),
        "{events:?}"
    );
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_launch_publishes_a_failure_event() -> TestResult {
    let mut running = Running::start().await?;
    running.fake.create_status(500);
    let mut subscription = running
        .client
        .subscribe(lifecycle_subject(&LeaseId::new(LEASE)))
        .await?;
    running.dispatch(LEASE).await?;
    let events = events(&mut subscription, 4).await?;
    assert!(
        matches!(
            events.as_slice(),
            [
                LifecycleEvent::Started { .. },
                LifecycleEvent::PhaseStarted { .. },
                LifecycleEvent::PhaseFailed { .. },
                LifecycleEvent::Failed { .. },
            ]
        ),
        "{events:?}"
    );
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_id_longer_than_a_sandbox_id_skips_the_started_and_failed_events() -> TestResult {
    let mut running = Running::start().await?;
    let long = "l".repeat(600);
    let mut subscription = running
        .client
        .subscribe(lifecycle_subject(&LeaseId::new(&long)))
        .await?;
    running.dispatch(&long).await?;
    let events_seen = events(&mut subscription, 3).await?;
    assert!(
        matches!(
            events_seen.as_slice(),
            [
                LifecycleEvent::PhaseStarted { .. },
                LifecycleEvent::PhaseCompleted { .. },
                LifecycleEvent::Completed { .. },
            ]
        ),
        "{events_seen:?}"
    );
    running.fake.create_status(500);
    let other = "m".repeat(600);
    let mut failing = running
        .client
        .subscribe(lifecycle_subject(&LeaseId::new(&other)))
        .await?;
    running.dispatch(&other).await?;
    let events_seen = events(&mut failing, 2).await?;
    assert!(
        matches!(
            events_seen.as_slice(),
            [LifecycleEvent::PhaseStarted { .. }, LifecycleEvent::PhaseFailed { .. }]
        ),
        "{events_seen:?}"
    );
    let late = tokio::time::timeout(SILENCE, failing.next()).await;
    assert!(late.is_err(), "a failure event named a sandbox id the protocol refuses");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_verb_stays_silent_when_the_lease_is_not_held() -> TestResult {
    let mut running = Running::start().await?;
    let worker = &running;
    let requests = every_verb().into_iter().map(|(verb, mut payload, _)| {
        payload["lease_id"] = json!("lease-9");
        async move { worker.silent(&verb_subject(verb, "lease-9"), &payload).await }
    });
    futures::future::try_join_all(requests).await?;
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_verb_replies_when_the_lease_is_held() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    for (verb, payload, outcome) in every_verb() {
        let reply = running.verb(verb, LEASE, payload).await?;
        assert_eq!(reply["outcome"], outcome, "{verb}: {reply}");
    }
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn suspend_and_wake_support_only_the_disk_level() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    let memory = json!({"generation": 1, "level": "memory"});
    let suspended = running.verb("suspend", LEASE, memory.clone()).await?;
    assert_eq!(suspended["outcome"], "unsupported", "{suspended}");
    let woken = running.verb("wake", LEASE, memory).await?;
    assert_eq!(woken["outcome"], "unsupported", "{woken}");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_suspend_or_wake_the_platform_refuses_is_denied() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    let disk = json!({"generation": 1, "level": "disk"});
    running.fake.stop_status(500);
    let suspended = running.verb("suspend", LEASE, disk.clone()).await?;
    assert_eq!(suspended["outcome"], "denied", "{suspended}");
    running.fake.get_status(500);
    let woken = running.verb("wake", LEASE, disk).await?;
    assert_eq!(woken["outcome"], "denied", "{woken}");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_running_without_a_native_name_is_unsupported() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    let target = json!({"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"});
    let reply = running
        .verb("checkpoint_running", LEASE, json!({"target": target}))
        .await?;
    let outcome: SnapshotExportOutcome = serde_json::from_value(reply)?;
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Unsupported { .. })
    ));
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_write_travels_in_chunks() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    let files = json!({"files": [{"path": "/tmp/big", "content": vec![7u8; 2_000_000]}]});
    let reply = running.verb("write_files", LEASE, files).await?;
    assert_eq!(reply["outcome"], "written", "{reply}");
    assert!(running.fake.written_bytes() > 0);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_read_travels_in_chunks() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.fake.read_content(vec![7u8; 2_000_000]);
    let reply = running.verb("read_file", LEASE, json!({"path": "/tmp/big"})).await?;
    assert_eq!(reply["outcome"], "content");
    assert_eq!(reply["data"].as_array().map(Vec::len), Some(2_000_000));
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verb_without_a_reply_subject_is_ignored() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running
        .publish(verb_subject("pause", LEASE), &json!({"lease_id": LEASE}))
        .await?;
    let reply = running.verb("native_handle", LEASE, json!({})).await?;
    assert_eq!(reply["outcome"], "available");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_undecodable_verb_gets_no_reply() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    let request = running.client.request(verb_subject("pause", LEASE), "not json".into());
    assert!(!matches!(tokio::time::timeout(SILENCE, request).await, Ok(Ok(_))));
    running
        .silent(
            &verb_subject("run_command", LEASE),
            &json!({"lease_id": LEASE, "command": 5}),
        )
        .await?;
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_transfer_header_gets_no_reply() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Sandbox-Transfer", "chunks-v0");
    let request = running
        .client
        .request_with_headers(verb_subject("pause", LEASE), headers, "{}".into());
    assert!(!matches!(tokio::time::timeout(SILENCE, request).await, Ok(Ok(_))));
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_export_is_answered_as_unsupported() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running.ask(&running.snapshot_export_subject(), &json!({})).await?;
    let outcome: SnapshotExportOutcome = serde_json::from_value(reply)?;
    assert!(matches!(
        outcome,
        SnapshotExportOutcome::Err(SnapshotExportError::Unsupported { .. })
    ));
    running.publish(running.snapshot_export_subject(), &json!({})).await?;
    let again = running.ask(&running.snapshot_export_subject(), &json!({})).await?;
    assert!(again.is_object());
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_worker_in_enforce_mode_purges_a_lease_the_broker_disowns() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await;
    let fake = Fake::start().await?;
    let provider = fake.provider()?;
    let worker_id = WorkerId::new("worker-1")?;
    provider.bind_worker(&worker_id)?;
    let request: LeaseRequest = serde_json::from_value(request_json())?;
    provider.lease(&request, &LeaseId::new(LEASE)).await?;
    let mut checks = client.subscribe(subjects::worker_authority_check(&worker_id)).await?;
    let answerer = client.clone();
    let broker = tokio::spawn(async move {
        while let Some(message) = checks.next().await {
            if let Some(reply) = message.reply {
                let _ = answerer.publish(reply, r#"{"owned":[]}"#.into()).await;
            }
        }
    });
    let mut config = AuthorityConfig::new(AuthorityMode::Enforce);
    config.interval = Duration::from_millis(100);
    config.partition_after = Duration::from_millis(500);
    config.grace = Duration::from_millis(200);
    config.min_lease_age = Duration::ZERO;
    config.confirmations = 2;
    config.request_timeout = Duration::from_millis(500);
    let oracle = Arc::new(NatsAuthorityOracle::new(client.clone(), worker_id.clone()));
    let guard = AuthorityGuard::new(config, worker_id, oracle, provider.clone());
    let cancel = CancellationToken::new();
    let task = tokio::spawn(guard.run(cancel.clone()));
    until(|| fake.deletes() == 1).await?;
    cancel.cancel();
    task.await?;
    broker.abort();
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

#[tokio::test]
async fn every_worker_error_has_a_message() -> TestResult {
    let errors: Vec<VercelWorkerError> = vec![
        async_nats::client::SubscribeError::new(SubscribeErrorKind::Other).into(),
        async_nats::client::PublishError::new(PublishErrorKind::Send).into(),
        serde_json::from_str::<Value>("{")
            .err()
            .ok_or("expected a parse failure")?
            .into(),
        NatsTransferError::UnknownVersion.into(),
        "bogus"
            .parse::<AuthorityMode>()
            .err()
            .ok_or("expected a parse failure")?
            .into(),
        WorkerId::new("").err().ok_or("expected a refusal")?.into(),
        VercelProviderError::BoundToAnotherWorker.into(),
        NatsConnectError::InvalidUrl(url::ParseError::EmptyHost).into(),
        LeaseError::Backend {
            message: "refused".to_owned(),
        }
        .into(),
        VercelApiError::NotFound("gone".to_owned()).into(),
        ObservabilityError::Listener {
            addr: SocketAddr::from(([127, 0, 0, 1], 1)),
            source: std::io::Error::other("refused"),
        }
        .into(),
    ];
    for error in &errors {
        assert!(!error.to_string().is_empty());
    }
    assert_eq!(errors.len(), 11);
    Ok(())
}

/// Pins today's behaviour: a cancel whose delete failed is not retried, and the broker forgets a lease once it has
/// published the cancel, so only another cancel deletes the sandbox.
#[tokio::test]
async fn a_failed_cancel_is_not_retried_until_another_cancel_arrives() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.fake.delete_status(500);
    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| fake.deletes() == 1).await?;

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(running.fake.deletes(), 1, "nothing retried the delete");
    assert_eq!(running.provider.active_lease_count().await, 1);
    // The fake reads the status after it logs the DELETE, so it only recovers once the first delete has long failed.
    running.fake.delete_status(200);

    running.cancel_lease(LEASE).await?;
    let provider = Arc::clone(&running.provider);
    tokio::time::timeout(Duration::from_secs(10), async {
        while provider.active_lease_count().await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    running.dispatch(LEASE).await?;
    assert_eq!(running.fake.creates(), 2);
    running.stop().await
}
