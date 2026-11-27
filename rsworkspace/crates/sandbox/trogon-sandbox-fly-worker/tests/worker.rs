mod support;

use std::num::NonZeroU32;
use std::time::Duration;

use async_nats::client::{FlushErrorKind, PublishErrorKind, SubscribeErrorKind};
use base64::Engine as _;
use futures::StreamExt;
use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{LeaseId, ProviderKind};
use trogon_sandbox_fly_worker::{FlyProviderError, FlyWorker, FlyWorkerError, FlyWorkspaceVolume};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, DispatchDecodeError, NatsTransferError, subjects};

const SILENCE: Duration = Duration::from_millis(700);

fn operation_subject(verb: &str, lease: &str) -> String {
    format!("sandbox.provider.fly.{verb}.{lease}")
}

async fn grant(running: &Running, lease: &str, attempt: &str) -> TestResult {
    let reply = running.dispatch(lease, attempt, request_json()).await?;
    assert!(reply.get("sandbox_id").is_some(), "not granted: {reply}");
    Ok(())
}

/// Asserts that a request on `subject` gets no answer.
async fn silent(running: &Running, subject: &str, payload: &Value) -> TestResult {
    let outcome = tokio::time::timeout(
        SILENCE,
        running
            .client
            .request(subject.to_owned(), serde_json::to_vec(payload)?.into()),
    )
    .await;
    assert!(!matches!(outcome, Ok(Ok(_))), "unexpected reply on {subject}");
    Ok(())
}

async fn publish(running: &Running, subject: String, payload: &Value) -> TestResult {
    running
        .client
        .publish(subject, serde_json::to_vec(payload)?.into())
        .await?;
    running.client.flush().await?;
    Ok(())
}

async fn until(mut check: impl FnMut() -> bool) -> TestResult {
    for _ in 0..100 {
        if check() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("condition not reached".into())
}

fn cleanup_json(running: &Running, lease: &str, attempt: &str, intent: &str) -> Value {
    json!({
        "identity": {
            "lease_id": lease,
            "provider": "fly",
            "target": {"worker_id": running.worker_id.as_str(), "attempt_id": attempt},
        },
        "intent": intent,
    })
}

fn dispatch_payload(running: &Running, lease: &str, attempt: &str) -> Value {
    dispatch_payload_for(&running.worker_id, lease, attempt)
}

fn command_json(command: &[&str]) -> Value {
    json!({"command": command, "timeout_seconds": 10})
}

#[tokio::test]
async fn dispatch_grants_a_lease_and_creates_a_machine() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    assert_eq!(running.env.fly.machines().len(), 1);
    assert!(running.env.provider.holds_lease(&LeaseId::new("lease-1")).await);
    running.stop().await
}

#[tokio::test]
async fn dispatch_without_a_target_fails_to_launch() -> TestResult {
    let mut running = Running::start().await?;
    let subject = subjects::worker_dispatch(ProviderKind::Fly, &running.worker_id);
    let reply = running
        .ask(subject, &json!({"lease_id": "lease-1", "request": request_json()}))
        .await?;
    assert_eq!(reply["provider_error"], "targeted dispatch identity is required");
    assert!(running.env.fly.machines().is_empty());
    running.stop().await
}

#[tokio::test]
async fn an_undecodable_dispatch_is_rejected() -> TestResult {
    let mut running = Running::start().await?;
    let subject = subjects::worker_dispatch(ProviderKind::Fly, &running.worker_id);
    let reply = running.ask(subject, &json!({"unexpected": true})).await?;
    assert!(reply["decode_error"].is_string(), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn a_provider_failure_fails_the_launch() -> TestResult {
    let mut running = Running::start().await?;
    running.env.fly.with(|state| state.create_status = Some(500));
    let reply = running.dispatch("lease-1", "attempt-1", request_json()).await?;
    assert!(reply["provider_error"].is_string(), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    let subject = subjects::worker_dispatch(ProviderKind::Fly, &running.worker_id);
    publish(&running, subject, &dispatch_payload(&running, "lease-1", "attempt-1")).await?;
    let fly = &running.env.fly;
    until(|| fly.machines().len() == 1).await?;
    running.stop().await
}

#[tokio::test]
async fn cleanup_destroys_the_machine_of_an_attempt() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let subject = subjects::worker_cleanup(ProviderKind::Fly, &running.worker_id);
    let reply = running
        .ask(subject, &cleanup_json(&running, "lease-1", "attempt-1", "release"))
        .await?;
    assert_eq!(reply["outcome"], "cleaned", "{reply}");
    assert!(running.env.fly.machines().is_empty());
    running.stop().await
}

#[tokio::test]
async fn cleanup_without_a_reply_subject_still_cleans() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let subject = subjects::worker_cleanup(ProviderKind::Fly, &running.worker_id);
    publish(
        &running,
        subject,
        &cleanup_json(&running, "lease-1", "attempt-1", "purge"),
    )
    .await?;
    let fly = &running.env.fly;
    until(|| fly.machines().is_empty()).await?;
    running.stop().await
}

#[tokio::test]
async fn a_malformed_cleanup_is_dropped() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let subject = subjects::worker_cleanup(ProviderKind::Fly, &running.worker_id);
    silent(&running, &subject, &json!({"identity": 1})).await?;
    assert_eq!(running.env.fly.machines().len(), 1);
    running.stop().await
}

#[tokio::test]
async fn checkpoint_running_without_a_name_is_unsupported() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let target = json!({"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"});
    let reply = running
        .operation("checkpoint_running", "lease-1", json!({"target": target}))
        .await?;
    assert!(reply.to_string().contains("unsupported"), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn checkpoint_running_with_a_name_needs_a_workspace_volume() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let target = json!({"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"});
    let payload = json!({"target": target, "native_name": "snap"});
    let reply = running.operation("checkpoint_running", "lease-1", payload).await?;
    assert!(reply.to_string().contains("workspace volume"), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn checkpoint_running_with_a_workspace_volume_returns_a_native_receipt() -> TestResult {
    let env = Env::with_parts(|settings| {
        settings.workspace_volume = Some(FlyWorkspaceVolume::new(
            NonZeroU32::MIN,
            Duration::from_millis(10),
            Duration::from_secs(2),
        )?);
        Ok(())
    })
    .await?;
    let mut running = Running::start_with(env).await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let target = json!({"registry": "r", "repository": "p", "artifact_type": "sandbox_rootfs_v1"});
    let payload = json!({"target": target, "native_name": "snap"});
    let reply = running.operation("checkpoint_running", "lease-1", payload).await?;
    assert!(reply.to_string().contains("vs_snap1"), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn snapshot_is_unsupported() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let reply = running.operation("snapshot", "lease-1", json!({})).await?;
    assert!(reply.to_string().contains("no Machine filesystem export"), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn get_domain_answers_for_a_held_lease() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let reply = running
        .operation("get_domain", "lease-1", json!({"port": 8080}))
        .await?;
    assert!(reply.is_object(), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn pause_and_resume_move_the_machine() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let paused = running.operation("pause", "lease-1", json!({})).await?;
    assert_eq!(paused["outcome"], "done", "{paused}");
    let resumed = running.operation("resume", "lease-1", json!({})).await?;
    assert_eq!(resumed["outcome"], "done", "{resumed}");
    running.stop().await
}

#[tokio::test]
async fn pause_and_resume_report_a_backend_denial() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    running.env.fly.with(|state| {
        state.suspend_ignored = true;
        state.start_fails = true;
    });
    let paused = running.operation("pause", "lease-1", json!({})).await?;
    let resumed = running.operation("resume", "lease-1", json!({})).await?;
    assert!(paused["outcome"].is_string() && resumed["outcome"].is_string());
    running.stop().await
}

#[tokio::test]
async fn suspend_and_wake_by_level() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let memory = json!({"generation": 1, "level": "memory"});
    let disk = json!({"generation": 1, "level": "disk"});
    let suspended = running.operation("suspend", "lease-1", memory.clone()).await?;
    assert_eq!(suspended["outcome"], "done", "{suspended}");
    let woken = running.operation("wake", "lease-1", memory).await?;
    assert_eq!(woken["outcome"], "done", "{woken}");
    let suspended = running.operation("suspend", "lease-1", disk.clone()).await?;
    assert_eq!(suspended["outcome"], "unsupported", "{suspended}");
    let woken = running.operation("wake", "lease-1", disk).await?;
    assert_eq!(woken["outcome"], "unsupported", "{woken}");
    running.stop().await
}

#[tokio::test]
async fn suspend_and_wake_by_memory_report_a_backend_denial() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    running.env.fly.with(|state| {
        state.suspend_ignored = true;
        state.start_fails = true;
    });
    let memory = json!({"generation": 2, "level": "memory"});
    let suspended = running.operation("suspend", "lease-1", memory.clone()).await?;
    let woken = running.operation("wake", "lease-1", memory).await?;
    assert!(suspended["outcome"].is_string() && woken["outcome"].is_string());
    running.stop().await
}

#[tokio::test]
async fn run_command_returns_the_captured_output() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let reply = running
        .operation("run_command", "lease-1", command_json(&["echo", "hi"]))
        .await?;
    assert_eq!(reply["outcome"], "completed", "{reply}");
    assert_eq!(reply["exit_code"], 7);
    running.stop().await
}

#[tokio::test]
async fn run_command_reports_a_backend_denial() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    running
        .env
        .fly
        .with(|state| state.exec_exit.push(("trogon-sandbox-capture".to_owned(), 1)));
    let reply = running
        .operation("run_command", "lease-1", command_json(&["echo", "hi"]))
        .await?;
    assert_eq!(reply["outcome"], "denied", "{reply}");
    running.stop().await
}

#[tokio::test]
async fn run_command_trims_a_reply_larger_than_the_bus_allows() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let encoded = base64_of(&vec![255u8; 400_000]);
    running.env.fly.with(|state| {
        state.capture_reply = Some(format!("0\n400000\n{encoded}\n\n0\n\n\n"));
    });
    let reply = running
        .operation("run_command", "lease-1", command_json(&["cat", "big"]))
        .await?;
    assert_eq!(reply["outcome"], "completed", "{}", reply["outcome"]);
    assert_eq!(reply["truncated"], true);
    running.stop().await
}

fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[tokio::test]
async fn write_files_writes_and_reports_denial() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let files = json!({"files": [{"path": "/tmp/a", "content": [104, 105]}]});
    let written = running.operation("write_files", "lease-1", files.clone()).await?;
    assert_eq!(written["outcome"], "written", "{written}");
    running
        .env
        .fly
        .with(|state| state.exec_http_fail.push("mktemp".to_owned()));
    let denied = running.operation("write_files", "lease-1", files).await?;
    assert_eq!(denied["outcome"], "denied", "{denied}");
    running.stop().await
}

#[tokio::test]
async fn read_file_found_missing_and_failing() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let request = json!({"path": "/tmp/a"});
    let found = running.operation("read_file", "lease-1", request.clone()).await?;
    assert_eq!(found["outcome"], "content", "{found}");
    running.env.fly.with(|state| state.read_exit_code = 3);
    let missing = running.operation("read_file", "lease-1", request.clone()).await?;
    assert_eq!(missing["outcome"], "not_found", "{missing}");
    running.env.fly.with(|state| state.read_exit_code = 1);
    let failed = running.operation("read_file", "lease-1", request).await?;
    assert_eq!(failed["outcome"], "denied", "{failed}");
    running.stop().await
}

#[tokio::test]
async fn file_op_is_unsupported() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let reply = running
        .operation("file_op", "lease-1", json!({"op": {"op": "stat", "path": "/"}}))
        .await?;
    assert!(reply.to_string().contains("unsupported"), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn an_unknown_verb_is_denied() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let reply = running.operation("frobnicate", "lease-1", json!({})).await?;
    assert_eq!(reply["outcome"], "denied");
    assert_eq!(reply["kind"], "invalid_spec");
    assert!(
        reply["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("frobnicate"))
    );
    running.stop().await
}

#[tokio::test]
async fn cancel_and_purge_verbs_are_ignored() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    for verb in ["cancel", "purge"] {
        silent(
            &running,
            &operation_subject(verb, "lease-1"),
            &json!({"lease_id": "lease-1"}),
        )
        .await?;
    }
    assert!(running.env.provider.holds_lease(&LeaseId::new("lease-1")).await);
    running.stop().await
}

#[tokio::test]
async fn an_operation_on_a_lease_the_worker_does_not_hold_is_ignored() -> TestResult {
    let mut running = Running::start().await?;
    silent(
        &running,
        &operation_subject("pause", "lease-9"),
        &json!({"lease_id": "lease-9"}),
    )
    .await?;
    running.stop().await
}

#[tokio::test]
async fn an_operation_without_a_reply_subject_is_ignored() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    publish(
        &running,
        operation_subject("pause", "lease-1"),
        &json!({"lease_id": "lease-1"}),
    )
    .await?;
    let reply = running.operation("snapshot", "lease-1", json!({})).await?;
    assert!(reply.is_object());
    running.stop().await
}

#[tokio::test]
async fn a_payload_lease_id_that_differs_from_the_subject_gets_no_reply() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    silent(
        &running,
        &operation_subject("pause", "lease-1"),
        &json!({"lease_id": "lease-2"}),
    )
    .await?;
    let reply = running.operation("snapshot", "lease-1", json!({})).await?;
    assert!(reply.is_object());
    running.stop().await
}

#[tokio::test]
async fn undecodable_operation_payloads_get_no_reply() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let subject = operation_subject("pause", "lease-1");
    let outcome = tokio::time::timeout(SILENCE, running.client.request(subject, "not json".into())).await;
    assert!(!matches!(outcome, Ok(Ok(_))));
    silent(
        &running,
        &operation_subject("run_command", "lease-1"),
        &json!({"lease_id": "lease-1", "command": 5}),
    )
    .await?;
    running.stop().await
}

#[tokio::test]
async fn a_poisoned_transfer_descriptor_gets_no_reply() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Sandbox-Transfer", "chunks-v0");
    let outcome = tokio::time::timeout(
        SILENCE,
        running
            .client
            .request_with_headers(operation_subject("pause", "lease-1"), headers, "{}".into()),
    )
    .await;
    assert!(!matches!(outcome, Ok(Ok(_))));
    running.stop().await
}

#[tokio::test]
async fn a_pool_tick_creates_a_warm_machine() -> TestResult {
    let env = Env::with_parts(|settings| {
        settings.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    let mut running = Running::start_with(env).await?;
    let fly = &running.env.fly;
    until(|| fly.machines().len() == 1).await?;
    assert_eq!(fly.machines()[0]["state"], "created");
    running.stop().await
}

#[tokio::test]
async fn a_failing_pool_tick_does_not_stop_the_worker() -> TestResult {
    let env = Env::with_parts(|settings| {
        settings.warm_pool_size = 1;
        Ok(())
    })
    .await?;
    env.fly.with(|state| state.create_status = Some(500));
    let mut running = Running::start_with(env).await?;
    let reply = running.operation("snapshot", "lease-9", json!({})).await;
    assert!(reply.is_err());
    running.stop().await
}

#[tokio::test]
async fn the_worker_publishes_a_heartbeat() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let mut beats = running
        .client
        .subscribe(subjects::worker_heartbeat(ProviderKind::Fly, &running.worker_id))
        .await?;
    let message = tokio::time::timeout(Duration::from_secs(20), beats.next())
        .await?
        .ok_or("heartbeat stream ended")?;
    let beat: Value = serde_json::from_slice(&message.payload)?;
    assert_eq!(beat["worker_id"], running.worker_id.as_str());
    assert_eq!(beat["capacity"]["current_leases"], 1);
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 2);
    running.stop().await
}

#[tokio::test]
async fn stopping_the_worker_destroys_the_machines_it_holds() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    grant(&running, "lease-2", "attempt-2").await?;
    assert_eq!(running.env.fly.machines().len(), 2);
    running.stop().await?;
    assert!(running.env.fly.machines().is_empty());
    Ok(())
}

/// The error a subscription reports when it unsubscribes after its connection was drained.
async fn unsubscribe_failure() -> Boxed<async_nats::UnsubscribeError> {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let mut subscription = client.subscribe("closed.soon").await?;
    client.drain().await?;
    for _ in 0..50 {
        match subscription.unsubscribe().await {
            Err(error) => return Ok(error),
            Ok(()) => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    Err("the connection never closed".into())
}

#[tokio::test]
async fn every_worker_error_has_a_message() -> TestResult {
    let parse = serde_json::from_str::<Value>("{")
        .err()
        .ok_or("expected a parse failure")?;
    let errors: Vec<FlyWorkerError> = vec![
        FlyWorkerError::Subject,
        FlyWorkerError::LeaseMismatch,
        async_nats::client::SubscribeError::new(SubscribeErrorKind::Other).into(),
        unsubscribe_failure().await?.into(),
        async_nats::client::FlushError::new(FlushErrorKind::FlushError).into(),
        async_nats::client::PublishError::new(PublishErrorKind::Send).into(),
        serde_json::from_str::<Value>("{")
            .err()
            .ok_or("expected a parse failure")?
            .into(),
        DispatchDecodeError::Json(parse).into(),
        NatsTransferError::UnknownVersion.into(),
        FlyProviderError::NotHeld.into(),
    ];
    for error in &errors {
        assert!(!error.to_string().is_empty());
    }
    assert_eq!(errors.len(), 10);
    Ok(())
}

async fn trimmed_reply(stdout: usize, stderr: usize) -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let out = base64_of(&vec![255u8; stdout]);
    let err = base64_of(&vec![255u8; stderr]);
    running.env.fly.with(|state| {
        state.capture_reply = Some(format!("0\n{stdout}\n{out}\n\n{stderr}\n{err}\n\n"));
    });
    let reply = running
        .operation("run_command", "lease-1", command_json(&["cat", "big"]))
        .await?;
    assert_eq!(reply["outcome"], "completed");
    assert_eq!(reply["truncated"], true);
    running.stop().await
}

#[tokio::test]
async fn a_small_stdout_leaves_the_budget_to_a_large_stderr() -> TestResult {
    trimmed_reply(5, 400_000).await
}

#[tokio::test]
async fn two_large_streams_split_the_budget() -> TestResult {
    trimmed_reply(400_000, 400_000).await
}

#[tokio::test]
async fn a_worker_in_enforce_mode_purges_a_lease_the_broker_disowns() -> TestResult {
    let env = Env::start().await?;
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let worker_id = env.provider.dispatch_ledger().worker_id().clone();
    let mut checks = client.subscribe(subjects::worker_authority_check(&worker_id)).await?;
    let answerer = client.clone();
    let broker = tokio::spawn(async move {
        while let Some(message) = checks.next().await {
            if let Some(reply) = message.reply {
                let _ = answerer.publish(reply, r#"{"owned":[]}"#.into()).await;
            }
        }
    });
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let mut authority = AuthorityConfig::new(AuthorityMode::Enforce);
    authority.interval = Duration::from_millis(100);
    authority.partition_after = Duration::from_millis(500);
    authority.grace = Duration::from_millis(200);
    authority.min_lease_age = Duration::ZERO;
    authority.confirmations = 2;
    authority.request_timeout = Duration::from_millis(500);
    let worker = FlyWorker::new(client.clone(), env.provider.clone(), authority);
    let task = tokio::spawn(async move {
        worker
            .run(async {
                let _ = stopped.await;
            })
            .await
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    let payload = dispatch_payload_for(&worker_id, "lease-1", "attempt-1");
    let reply = trogon_sandbox_protocol::nats_transfer::request(
        &client,
        &subjects::worker_dispatch(ProviderKind::Fly, &worker_id),
        serde_json::to_vec(&payload)?.into(),
        None,
        Duration::from_secs(20),
    )
    .await?;
    let reply: Value = serde_json::from_slice(&reply)?;
    assert!(reply.get("sandbox_id").is_some(), "{reply}");
    // The guard judges a lease only after it has aged, twice in a row, and then waits out a grace period.
    let fly = &env.fly;
    for _ in 0..300 {
        if fly.machines().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(fly.machines().is_empty());
    let _ = stop.send(());
    task.await??;
    broker.abort();
    Ok(())
}

fn dispatch_payload_for(worker_id: &trogon_sandbox_contract::WorkerId, lease: &str, attempt: &str) -> Value {
    json!({
        "lease_id": lease,
        "request": request_json(),
        "target": {"worker_id": worker_id.as_str(), "attempt_id": attempt},
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_abandons_a_request_that_outlasts_the_drain_period() -> TestResult {
    let mut running = Running::start_draining(Env::start().await?, Duration::from_secs(1)).await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let before = running.env.fly.count("POST", "/exec");
    running
        .env
        .fly
        .with(|state| state.exec_sleep = Some(Duration::from_secs(10)));
    let payload = json!({"lease_id": "lease-1", "command": ["sleep", "400"], "timeout_seconds": 600});
    let body: Vec<u8> = serde_json::to_vec(&payload)?;
    let client = running.client.clone();
    let stuck = tokio::spawn(async move {
        client
            .request(operation_subject("run_command", "lease-1"), body.into())
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    let fly = &running.env.fly;
    until(|| fly.count("POST", "/exec") > before).await?;
    running.stop().await?;
    assert!(running.env.fly.machines().is_empty());
    stuck.abort();
    Ok(())
}
