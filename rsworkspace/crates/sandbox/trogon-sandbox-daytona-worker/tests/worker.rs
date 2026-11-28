mod nats_support;
mod support;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nats_support::{Nats, Running, SILENCE, dispatch_json, until};
use serde_json::{Value, json};
use support::{Boxed, Fake, Reply, SANDBOX, TestResult, record, request_json};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, LifecycleEvent, SnapshotExportError, SnapshotExportOutcome, WorkerId,
};
use trogon_sandbox_protocol::{
    AuthorityConfig, AuthorityGuard, AuthorityMode, NatsAuthorityOracle, Provider, lifecycle_subject,
};

const LEASE: &str = "lease-1";

fn verb_subject(verb: &str, lease: &str) -> String {
    format!("sandbox.provider.daytona.{verb}.{lease}")
}

fn granted_sandbox(reply: &Value) -> Result<&str, Boxed> {
    Ok(reply["sandbox_id"].as_str().ok_or("the lease was not granted")?)
}

fn deletes(fake: &Fake) -> usize {
    fake.count("DELETE", &format!("/sandbox/{SANDBOX}"))
}

fn creates(fake: &Fake) -> usize {
    fake.count("POST", "/sandbox")
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

/// Every per-lease verb with a request that names no lease.
fn every_verb() -> Vec<(&'static str, Value)> {
    let target = json!({"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"});
    vec![
        ("run_command", json!({"command": ["echo", "hi"], "timeout_seconds": 5})),
        ("poll_command", json!({"execution_id": "exec-1"})),
        ("kill_command", json!({"execution_id": "exec-1", "signal": "kill"})),
        (
            "write_stdin",
            json!({"execution_id": "exec-1", "seq": 0, "data": [1], "eof": false}),
        ),
        (
            "write_files",
            json!({"files": [{"path": "/tmp/a", "content": [104, 105]}]}),
        ),
        ("read_file", json!({"path": "/tmp/a"})),
        ("file_op", json!({"op": {"op": "stat", "path": "/"}})),
        ("get_domain", json!({"port": 8080})),
        ("pause", json!({})),
        ("resume", json!({})),
        ("suspend", json!({"generation": 1, "level": "disk"})),
        ("wake", json!({"generation": 1, "level": "disk"})),
        ("native_handle", json!({})),
        ("ssh_access", json!({})),
        ("checkpoint_running", json!({"target": target, "native_name": "snap"})),
        ("network_policy_update", json!({"network_policy": "deny_all"})),
        ("create_user", json!({"username": "alice"})),
        ("create_group", json!({"groupname": "staff"})),
        (
            "modify_group_membership",
            json!({"username": "alice", "groupname": "staff", "action": "add"}),
        ),
    ]
}

fn script_toolbox(fake: &Fake) {
    let get = format!("/sandbox/{SANDBOX}");
    fake.executes(&[(0, String::new())]);
    fake.json("POST", &format!("/box/{SANDBOX}/files/bulk-upload"), 200, json!({}));
    let download = |text: &[u8]| Reply::Complete(200, text.to_vec());
    fake.script(
        "GET",
        &format!("/box/{SANDBOX}/files/download"),
        vec![
            download(b"completed"),
            download(b"hi\n"),
            download(b""),
            download(b"hello"),
        ],
    );
    fake.json(
        "GET",
        &format!("{get}/port-preview/8080"),
        200,
        json!({"url": "https://8080-abc.proxy.example", "token": "tok"}),
    );
    fake.json("POST", &format!("{get}/stop"), 200, json!({}));
    fake.json("POST", &format!("{get}/start"), 200, json!({}));
}

#[tokio::test(flavor = "multi_thread")]
async fn dispatch_grants_a_lease_and_creates_a_sandbox() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running.dispatch(LEASE).await?;
    assert_eq!(granted_sandbox(&reply)?, SANDBOX);
    assert_eq!(reply["worker_id"], "worker-1");
    assert_eq!(creates(&running.fake), 1);
    assert_eq!(running.provider.active_lease_count().await, 1);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redelivered_dispatch_returns_the_first_grant() -> TestResult {
    let mut running = Running::start().await?;
    let first = running.dispatch(LEASE).await?;
    let second = running.dispatch(LEASE).await?;
    assert_eq!(first, second);
    assert_eq!(creates(&running.fake), 1);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_lease_id_launches_again_when_reused() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| deletes(fake) == 1).await?;
    let reply = running.dispatch(LEASE).await?;
    assert_eq!(granted_sandbox(&reply)?, SANDBOX);
    assert_eq!(creates(&running.fake), 2);
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
    running
        .fake
        .json("DELETE", &format!("/sandbox/{SANDBOX}"), 500, json!({}));

    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| deletes(fake) == 1).await?;
    let reply = running.dispatch(LEASE).await?;

    assert!(
        reply["provider_error"]
            .as_str()
            .is_some_and(|error| error.contains("settle cleanup before reallocating")),
        "{reply}"
    );
    assert_eq!(creates(&running.fake), 1, "no second sandbox was created");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_that_arrives_during_a_launch_waits_for_it_and_releases_the_sandbox() -> TestResult {
    let mut running = Running::start().await?;
    running.fake.script(
        "POST",
        "/sandbox",
        vec![Reply::Delayed(
            Duration::from_millis(800),
            200,
            record("started").to_string().into_bytes(),
        )],
    );
    let fake = &running.fake;
    let (reply, cancelled) = tokio::join!(running.dispatch(LEASE), async {
        until(|| creates(fake) == 1).await?;
        running.cancel_lease(LEASE).await
    });
    cancelled?;
    assert_eq!(granted_sandbox(&reply?)?, SANDBOX);
    until(|| deletes(fake) == 1).await?;
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_release_leaves_the_lease_held() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running
        .fake
        .json("DELETE", &format!("/sandbox/{SANDBOX}"), 500, json!({}));
    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| deletes(fake) >= 1).await?;
    assert_eq!(running.provider.active_lease_count().await, 1);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_undecodable_dispatch_is_rejected() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running
        .ask("sandbox.provider.daytona.lease", &json!({"unexpected": true}))
        .await?;
    assert!(reply["decode_error"].is_string(), "{reply}");
    running
        .publish(
            "sandbox.provider.daytona.lease".to_owned(),
            &json!({"unexpected": true}),
        )
        .await?;
    running.silent(&verb_subject("pause", "none"), &json!({})).await?;
    assert_eq!(creates(&running.fake), 0);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_failure_is_reported_in_the_reply() -> TestResult {
    let mut running = Running::start().await?;
    running.fake.json("POST", "/sandbox", 500, json!({"message": "down"}));
    let reply = running.dispatch(LEASE).await?;
    assert!(reply["provider_error"].is_string(), "{reply}");
    assert_eq!(running.provider.active_lease_count().await, 0);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    running
        .publish("sandbox.provider.daytona.lease".to_owned(), &dispatch_json(LEASE))
        .await?;
    let fake = &running.fake;
    until(|| creates(fake) == 1).await?;
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_id_the_bus_cannot_publish_events_on_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running.dispatch("lease with spaces").await?;
    assert_eq!(granted_sandbox(&reply)?, SANDBOX);
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_subject_the_worker_cannot_publish_to_does_not_stop_it() -> TestResult {
    let mut running = Running::start().await?;
    let sender = running.nats.unchecked_client().await?;
    sender
        .publish_with_reply(
            "sandbox.provider.daytona.lease",
            "reply..empty",
            serde_json::to_vec(&dispatch_json(LEASE))?.into(),
        )
        .await?;
    sender.flush().await?;
    let fake = &running.fake;
    until(|| creates(fake) == 1).await?;
    let reply = running.dispatch("lease-2").await?;
    assert_eq!(granted_sandbox(&reply)?, SANDBOX);
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
    let seen = events(&mut subscription, 4).await?;
    assert!(
        matches!(
            seen.as_slice(),
            [
                LifecycleEvent::Started { .. },
                LifecycleEvent::PhaseStarted { .. },
                LifecycleEvent::PhaseCompleted { .. },
                LifecycleEvent::Completed { .. },
            ]
        ),
        "{seen:?}"
    );
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_launch_publishes_a_failure_event() -> TestResult {
    let mut running = Running::start().await?;
    running.fake.json("POST", "/sandbox", 500, json!({"message": "down"}));
    let mut subscription = running
        .client
        .subscribe(lifecycle_subject(&LeaseId::new(LEASE)))
        .await?;
    running.dispatch(LEASE).await?;
    let seen = events(&mut subscription, 4).await?;
    assert!(
        matches!(
            seen.as_slice(),
            [
                LifecycleEvent::Started { .. },
                LifecycleEvent::PhaseStarted { .. },
                LifecycleEvent::PhaseFailed { .. },
                LifecycleEvent::Failed { .. },
            ]
        ),
        "{seen:?}"
    );
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_verb_stays_silent_when_the_lease_is_not_held() -> TestResult {
    let mut running = Running::start().await?;
    let worker = &running;
    let requests = every_verb().into_iter().map(|(verb, mut payload)| {
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
    script_toolbox(&running.fake);
    let get = format!("/sandbox/{SANDBOX}");
    let expected = [
        ("run_command", "completed"),
        ("poll_command", "not_found"),
        ("kill_command", "not_found"),
        ("write_stdin", "not_found"),
        ("write_files", "written"),
        ("read_file", "content"),
        ("file_op", "unsupported"),
        ("get_domain", "available"),
        ("pause", "done"),
        ("resume", "done"),
        ("suspend", "done"),
        ("wake", "done"),
        ("native_handle", "available"),
        ("ssh_access", "denied"),
        ("checkpoint_running", "err"),
        ("network_policy_update", "denied"),
        ("create_user", "denied"),
        ("create_group", "denied"),
        ("modify_group_membership", "denied"),
    ];
    for (verb, payload) in every_verb() {
        match verb {
            "pause" | "suspend" => running.fake.script(
                "GET",
                &get,
                vec![Reply::json(200, record("started")), Reply::json(200, record("stopped"))],
            ),
            "resume" | "wake" => running.fake.script(
                "GET",
                &get,
                vec![Reply::json(200, record("stopped")), Reply::json(200, record("started"))],
            ),
            _ => {}
        }
        let reply = running.verb(verb, LEASE, payload).await?;
        let outcome = expected
            .iter()
            .find(|(name, _)| *name == verb)
            .map(|(_, outcome)| *outcome);
        assert_eq!(reply["outcome"].as_str(), outcome, "{verb}: {reply}");
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
    running
        .fake
        .json("POST", &format!("/sandbox/{SANDBOX}/stop"), 500, json!({}));
    let suspended = running.verb("suspend", LEASE, disk.clone()).await?;
    assert_eq!(suspended["outcome"], "denied", "{suspended}");
    running.fake.json("GET", &format!("/sandbox/{SANDBOX}"), 500, json!({}));
    let woken = running.verb("wake", LEASE, disk).await?;
    assert_eq!(woken["outcome"], "denied", "{woken}");
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_running_is_unsupported() -> TestResult {
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
    running
        .fake
        .json("POST", &format!("/box/{SANDBOX}/files/bulk-upload"), 200, json!({}));
    let files = json!({"files": [{"path": "/tmp/big", "content": vec![7u8; 2_000_000]}]});
    let reply = running.verb("write_files", LEASE, files).await?;
    assert_eq!(reply["outcome"], "written", "{reply}");
    let uploads = running.fake.calls("POST", &format!("/box/{SANDBOX}/files/bulk-upload"));
    assert_eq!(uploads.len(), 1);
    assert!(uploads.iter().all(|upload| upload.body.len() > 1_000_000));
    running.stop().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_large_read_travels_in_chunks() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.fake.raw(
        "GET",
        &format!("/box/{SANDBOX}/files/download"),
        200,
        &vec![7u8; 2_000_000],
    );
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
    let fake = Fake::with_sandbox().await?;
    let provider = Arc::new(fake.provider()?);
    let worker_id = WorkerId::new("worker-1")?;
    provider.bind_worker(&worker_id)?;
    let request: LeaseRequest = serde_json::from_value(request_json())?;
    provider.lease(&request, &LeaseId::new(LEASE)).await?;
    let mut checks = client
        .subscribe(trogon_sandbox_protocol::subjects::worker_authority_check(&worker_id))
        .await?;
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
    until(|| deletes(&fake) == 1).await?;
    cancel.cancel();
    task.await?;
    broker.abort();
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}

/// Pins today's behaviour: a cancel whose delete failed is not retried, and the broker forgets a lease once it has
/// published the cancel, so only another cancel deletes the sandbox.
#[tokio::test]
async fn a_failed_cancel_is_not_retried_until_another_cancel_arrives() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch(LEASE).await?;
    running.fake.script(
        "DELETE",
        &format!("/sandbox/{SANDBOX}"),
        vec![Reply::json(500, json!({})), Reply::json(200, json!({}))],
    );
    running.cancel_lease(LEASE).await?;
    let fake = &running.fake;
    until(|| deletes(fake) == 1).await?;

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(deletes(&running.fake), 1, "nothing retried the delete");
    assert_eq!(running.provider.active_lease_count().await, 1);

    running.cancel_lease(LEASE).await?;
    let provider = Arc::clone(&running.provider);
    until(|| deletes(&running.fake) == 2).await?;
    tokio::time::timeout(Duration::from_secs(10), async {
        while provider.active_lease_count().await != 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    running.dispatch(LEASE).await?;
    assert_eq!(creates(&running.fake), 2);
    running.stop().await
}
