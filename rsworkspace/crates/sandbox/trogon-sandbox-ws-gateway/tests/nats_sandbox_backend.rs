mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::kv;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use support::{Reply, ScriptedBus, TestResult, lease_request};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_client::{BusMessage, ClientError};
use trogon_sandbox_contract::{
    CancelOperationRequest, Digest, ExecutionId, GroupMembershipAction, LeaseError, LeaseId, LeaseName,
    NativeSnapshotAcceptance, NetworkPolicy, OperationId, ProcessName, ReadLogsRequest, ReadyPorts, RunCommandRequest,
    RunCommandResponse, SandboxFileWrite, SandboxGroupName, SandboxTags, SandboxUserName, SnapshotTagName, StdinChunk,
    StdinEof, StdinSeq, SuspendRequestLevel, TenantId, TerminalId, TerminalOp, UnixSignal,
};
use trogon_sandbox_protocol::subjects::{SNAPSHOT_KV_BUCKET, SNAPSHOT_LIVE_KV_BUCKET, SNAPSHOT_TAG_KV_BUCKET};
use trogon_sandbox_ws_gateway::{
    CapabilityRegistry, NatsObservability, NatsSandboxBackend, ObservabilityConfig, RunCommandOutcome, SandboxBackend,
};

const KEY: &str = "backend-test-key";
const WAIT: Duration = Duration::from_secs(5);

async fn backend_over(bus: ScriptedBus) -> TestResult<(NatsSandboxBackend, JetStreamTestServer)> {
    let server = JetStreamTestServer::start().await;
    let observability = NatsObservability::new(server.client().await, ObservabilityConfig::default());
    let backend = NatsSandboxBackend::from_bus(Arc::new(bus), CapabilityRegistry::new(), observability);
    Ok((backend, server))
}

fn parse<T: DeserializeOwned>(value: Value) -> TestResult<T> {
    Ok(serde_json::from_value(value)?)
}

fn reply(value: &Value) -> TestResult<Reply> {
    Ok(Reply::Bytes(serde_json::to_vec(value)?))
}

fn tenant() -> TestResult<TenantId> {
    Ok(TenantId::new("tenant-test")?)
}

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn execution() -> ExecutionId {
    ExecutionId::new("exec-1")
}

fn digest() -> TestResult<Digest> {
    Ok(Digest::new(format!("sha256:{}", "a".repeat(64)))?)
}

#[track_caller]
fn unanswered<T>(result: Result<T, ClientError>) -> TestResult {
    let error = result.err();
    assert!(
        matches!(error, Some(ClientError::Timeout(_))),
        "expected a timeout, got {error:?}"
    );
    Ok(())
}

/// Waits until the forwarding task has dropped its sender.
async fn forwarder_gone(probe: &mpsc::Sender<(String, Value)>) -> TestResult {
    tokio::time::timeout(WAIT, async {
        while probe.strong_count() > 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn every_forwarded_call_reaches_the_bus_and_surfaces_its_failure() -> TestResult {
    let (backend, _server) = backend_over(ScriptedBus::silent()).await?;
    let key = KEY;
    let lease = lease();
    let exec = execution();
    let drive: trogon_sandbox_contract::DriveId = parse(json!({"tenant": "tenant-test", "name": "drive-1"}))?;
    let operation = OperationId::new("op-1")?;
    let user = SandboxUserName::new("alice")?;
    let group = SandboxGroupName::new("staff")?;
    let tag = SnapshotTagName::new("stable")?;
    let name = LeaseName::new("named")?;
    let logs: ReadLogsRequest = parse(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?;

    unanswered(backend.operation_capabilities(key, tenant()?).await)?;
    unanswered(
        backend
            .cancel_operation(
                key,
                CancelOperationRequest {
                    tenant: tenant()?,
                    operation_id: operation.clone(),
                    compensate_created: Default::default(),
                },
            )
            .await,
    )?;
    unanswered(backend.get_operation(key, tenant()?, &operation).await)?;
    unanswered(
        backend
            .lease(key, &parse(lease_request("tenant-test"))?, Duration::from_millis(20))
            .await,
    )?;
    backend.cancel(key, &lease).await?;
    unanswered(
        backend
            .update_tags(key, &lease, SandboxTags::new(BTreeMap::new())?)
            .await,
    )?;
    unanswered(backend.wait_released(key, &lease, Duration::from_millis(20)).await)?;
    unanswered(backend.stop_lease(key, tenant()?, &lease).await)?;
    unanswered(backend.pause_lease(key, tenant()?, &lease, Some(5)).await)?;
    unanswered(
        backend
            .suspend_lease(key, tenant()?, &lease, SuspendRequestLevel::Auto, None)
            .await,
    )?;
    unanswered(backend.wake_lease(key, tenant()?, &lease).await)?;
    unanswered(
        backend
            .snapshot_lease(key, tenant()?, &lease, NativeSnapshotAcceptance::Declined)
            .await,
    )?;
    unanswered(backend.snapshot_delete(key, tenant()?, digest()?).await)?;
    unanswered(
        backend
            .native_snapshot_delete(key, tenant()?, parse(json!({"provider": "e2b", "id": "native-1"}))?)
            .await,
    )?;
    unanswered(backend.resume_lease(key, tenant()?, &lease).await)?;
    unanswered(backend.lease_usage(key, tenant()?, &lease).await)?;
    unanswered(backend.tenant_usage(key, tenant()?).await)?;
    unanswered(backend.account_limits(key, tenant()?).await)?;
    unanswered(backend.delete_lease(key, tenant()?, &lease).await)?;
    unanswered(backend.extend_lease_timeout(key, &lease, 30).await)?;
    unanswered(
        backend
            .fork_lease(key, tenant()?, &lease, 60, "fork-key".to_string())
            .await,
    )?;
    unanswered(backend.update_network_policy(key, &lease, NetworkPolicy::DenyAll).await)?;
    unanswered(
        backend
            .create_drive(
                key,
                tenant()?,
                drive.clone(),
                Some(1),
                trogon_sandbox_contract::DriveStorage::S3 {
                    bucket: "bucket".to_string(),
                },
            )
            .await,
    )?;
    unanswered(backend.list_drives(key, tenant()?).await)?;
    unanswered(backend.delete_drive(key, tenant()?, drive).await)?;
    unanswered(backend.list_processes(key, &lease).await)?;
    unanswered(backend.wait_ready(key, &lease, &exec, 1).await)?;
    unanswered(
        backend
            .wait_sandbox_ready(key, &lease, ReadyPorts::new(vec![80])?, 1)
            .await,
    )?;
    unanswered(
        backend
            .kill_process(key, &lease, &ProcessName::new("web")?, UnixSignal::Term)
            .await,
    )?;
    unanswered(backend.poll_command(key, &lease, &exec).await)?;
    unanswered(backend.kill_command(key, &lease, &exec, UnixSignal::Kill).await)?;
    unanswered(
        backend
            .cancel_command_launch(key, parse(json!({"lease_id": "lease-1", "launch_key": "launch-1"}))?)
            .await,
    )?;
    unanswered(
        backend
            .stop_command(key, parse(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?)
            .await,
    )?;
    unanswered(
        backend
            .restart_command(key, parse(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?)
            .await,
    )?;
    unanswered(backend.read_logs(key, logs).await)?;
    unanswered(
        backend
            .write_stdin(
                key,
                &lease,
                &exec,
                StdinSeq::new(0),
                StdinChunk::new(vec![1])?,
                StdinEof::Keep,
            )
            .await,
    )?;
    unanswered(
        backend
            .terminal(key, &lease, TerminalId::new("term-1")?, TerminalOp::Close)
            .await,
    )?;
    unanswered(
        backend
            .write_files(
                key,
                &lease,
                vec![SandboxFileWrite {
                    path: "a.txt".to_string(),
                    content: vec![1],
                    mode: None,
                }],
                Some(user.clone()),
            )
            .await,
    )?;
    unanswered(backend.read_file(key, &lease, "a.txt".to_string(), None).await)?;
    unanswered(
        backend
            .file_op(
                key,
                parse(json!({"lease_id": "lease-1", "op": {"op": "stat", "path": "a.txt"}}))?,
            )
            .await,
    )?;
    unanswered(backend.create_user(key, &lease, user.clone()).await)?;
    unanswered(backend.create_group(key, &lease, group.clone()).await)?;
    unanswered(
        backend
            .modify_group_membership(key, &lease, user, group, GroupMembershipAction::Add)
            .await,
    )?;
    unanswered(backend.get_sandbox_domain(key, &lease, 8080).await)?;
    unanswered(backend.native_handle(key, &lease).await)?;
    unanswered(backend.ssh_access(key, &lease, Some(60)).await)?;
    unanswered(
        backend
            .get_entity(
                key,
                tenant()?,
                trogon_sandbox_contract::EntityRef::Name("n".to_string()),
            )
            .await,
    )?;
    unanswered(
        backend
            .list_entities(key, parse(json!({"tenant": "tenant-test"}))?)
            .await,
    )?;
    unanswered(
        backend
            .delete_entity(
                key,
                tenant()?,
                trogon_sandbox_contract::EntityRef::Name("n".to_string()),
            )
            .await,
    )?;
    unanswered(backend.get_lease_by_name(key, tenant()?, name.clone()).await)?;
    unanswered(backend.delete_named_sandbox(key, tenant()?, name).await)?;
    unanswered(backend.snapshot_get(key, tenant()?, digest()?).await)?;
    unanswered(backend.snapshot_tree(key, tenant()?, digest()?).await)?;
    unanswered(backend.set_snapshot_tag(key, tenant()?, tag.clone(), digest()?).await)?;
    unanswered(backend.delete_snapshot_tag(key, tenant()?, tag.clone()).await)?;
    unanswered(backend.snapshot_tag_history(key, tenant()?, tag).await)?;
    unanswered(
        backend
            .reap_leases(key, parse(json!({"tenant": "tenant-test", "older_than_seconds": 60}))?)
            .await,
    )?;
    Ok(())
}

#[tokio::test]
async fn run_command_maps_the_worker_answer_to_the_requested_mode() -> TestResult {
    let attached: RunCommandRequest = parse(json!({"lease_id": "lease-1", "command": ["ls"]}))?;
    let detached: RunCommandRequest = parse(json!({"lease_id": "lease-1", "command": ["ls"], "detached": true}))?;
    let started = json!({"outcome": "started", "execution_id": "exec-9"});
    let denied = serde_json::to_value(RunCommandResponse::Denied(LeaseError::LeaseWaking {
        lease_id: lease(),
        retry_after_ms: 10,
    }))?;

    let (backend, _server) = backend_over(ScriptedBus::answering(reply(&started)?)).await?;
    match backend.run_command(KEY, detached.clone()).await? {
        RunCommandOutcome::Started { execution_id, .. } => assert_eq!(execution_id.as_str(), "exec-9"),
        RunCommandOutcome::Completed(output) => return Err(format!("unexpected {output:?}").into()),
    }
    let mismatch = backend.run_command(KEY, attached.clone()).await.err();
    assert!(matches!(mismatch, Some(ClientError::Transport(_))), "{mismatch:?}");

    let (backend, _server) = backend_over(ScriptedBus::answering(reply(&denied)?)).await?;
    let refused = backend.run_command(KEY, attached).await.err();
    assert!(matches!(refused, Some(ClientError::Denied(_))), "{refused:?}");
    Ok(())
}

#[tokio::test]
async fn an_unknown_operation_reads_as_absent_and_other_failures_surface() -> TestResult {
    let operation = OperationId::new("op-1")?;
    let (backend, _server) = backend_over(ScriptedBus::answering(reply(&json!({"outcome": "not_found"}))?)).await?;
    assert!(backend.get_operation(KEY, tenant()?, &operation).await?.is_none());
    let (backend, _server) = backend_over(ScriptedBus::answering(reply(
        &json!({"outcome": "denied", "reason": "x"}),
    )?))
    .await?;
    assert!(backend.get_operation(KEY, tenant()?, &operation).await.is_err());
    Ok(())
}

#[tokio::test]
async fn terminal_output_is_forwarded_until_the_exit_frame() -> TestResult {
    let frame = |value: Value| -> TestResult<BusMessage> {
        Ok(BusMessage {
            subject: "terminal.output".to_string(),
            payload: Bytes::from(serde_json::to_vec(&value)?),
        })
    };
    let bus = ScriptedBus::silent().publishing(vec![
        frame(json!({"type": "data", "seq": 0, "data": "aGk="}))?,
        frame(json!({"type": "exit", "seq": 1, "exit_code": 0}))?,
        frame(json!({"type": "data", "seq": 2, "data": "aGk="}))?,
    ]);
    let (backend, _server) = backend_over(bus).await?;
    let (tx, mut rx) = mpsc::channel(8);
    backend
        .subscribe_terminal(TerminalId::new("term-1")?, tx, CancellationToken::new())
        .await?;
    let first = tokio::time::timeout(WAIT, rx.recv()).await?.ok_or("no first frame")?;
    assert_eq!(first.0, "term-1");
    assert_eq!(first.1["type"], "data");
    let second = tokio::time::timeout(WAIT, rx.recv()).await?.ok_or("no exit frame")?;
    assert_eq!(second.1["type"], "exit");
    assert!(
        tokio::time::timeout(WAIT, rx.recv()).await?.is_none(),
        "forwarding stops at exit"
    );
    Ok(())
}

#[tokio::test]
async fn a_cancelled_or_abandoned_terminal_stops_forwarding() -> TestResult {
    let message = BusMessage {
        subject: "terminal.output".to_string(),
        payload: Bytes::from_static(br#"{"type":"data","seq":0,"data":"aGk="}"#),
    };
    let (backend, _server) = backend_over(ScriptedBus::silent().publishing(vec![message.clone(), message])).await?;
    let stop = CancellationToken::new();
    stop.cancel();
    let (tx, mut rx) = mpsc::channel(8);
    backend.subscribe_terminal(TerminalId::new("term-1")?, tx, stop).await?;
    while tokio::time::timeout(WAIT, rx.recv()).await?.is_some() {}

    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let probe = tx.clone();
    backend
        .subscribe_terminal(TerminalId::new("term-2")?, tx, CancellationToken::new())
        .await?;
    forwarder_gone(&probe).await?;
    Ok(())
}

#[tokio::test]
async fn a_failing_subscription_is_a_backend_error() -> TestResult {
    let (backend, _server) = backend_over(ScriptedBus::silent().failing_subscriptions()).await?;
    let (tx, _rx) = mpsc::channel(1);
    assert!(
        backend
            .subscribe_terminal(TerminalId::new("term-1")?, tx.clone(), CancellationToken::new())
            .await
            .is_err()
    );
    assert!(backend.subscribe_evidence(KEY.to_string(), lease(), tx).await.is_err());
    Ok(())
}

#[tokio::test]
async fn evidence_forwarding_stops_when_the_receiver_is_gone() -> TestResult {
    let message = BusMessage {
        subject: "sandbox.evidence.lease-1.lifecycle".to_string(),
        payload: Bytes::from_static(b"started"),
    };
    let (backend, _server) = backend_over(ScriptedBus::silent().publishing(vec![message.clone(), message])).await?;
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let probe = tx.clone();
    backend.subscribe_evidence(KEY.to_string(), lease(), tx).await?;
    forwarder_gone(&probe).await?;
    Ok(())
}

#[tokio::test]
async fn evidence_frames_are_forwarded_as_text_or_base64() -> TestResult {
    let bus = ScriptedBus::silent().publishing(vec![
        BusMessage {
            subject: "sandbox.evidence.lease-1.lifecycle".to_string(),
            payload: Bytes::from_static(b"started"),
        },
        BusMessage {
            subject: "sandbox.evidence.lease-1.blob".to_string(),
            payload: Bytes::from_static(&[0xff, 0xfe]),
        },
    ]);
    let (backend, _server) = backend_over(bus).await?;
    let (tx, mut rx) = mpsc::channel(8);
    backend.subscribe_evidence(KEY.to_string(), lease(), tx).await?;
    let first = tokio::time::timeout(WAIT, rx.recv()).await?.ok_or("no first frame")?;
    assert_eq!(first.0, "lease-1");
    assert_eq!(first.1, json!({"subject_suffix": "lifecycle", "payload": "started"}));
    let second = tokio::time::timeout(WAIT, rx.recv()).await?.ok_or("no second frame")?;
    assert_eq!(second.1["payload"], "base64://4=");
    Ok(())
}

#[tokio::test]
async fn follow_logs_forwards_the_follower_events() -> TestResult {
    let (backend, _server) = backend_over(ScriptedBus::answering(reply(&json!({"outcome": "unsupported"}))?)).await?;
    let (tx, mut rx) = mpsc::channel(8);
    let request: ReadLogsRequest = parse(json!({"lease_id": "lease-1", "execution_id": "exec-1"}))?;
    backend.follow_logs(KEY.to_string(), request.clone(), tx).await?;
    let event = tokio::time::timeout(WAIT, rx.recv()).await?.ok_or("no event")?;
    assert_eq!(event.1["type"], "unsupported");

    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let probe = tx.clone();
    backend.follow_logs(KEY.to_string(), request, tx).await?;
    forwarder_gone(&probe).await?;
    Ok(())
}

#[tokio::test]
async fn dashboard_reads_and_watchers_go_through_the_observability_client() -> TestResult {
    let (backend, server) = backend_over(ScriptedBus::silent()).await?;
    let jetstream = server.jetstream().await;
    for bucket in [
        "sandbox-active-leases",
        SNAPSHOT_KV_BUCKET,
        SNAPSHOT_LIVE_KV_BUCKET,
        SNAPSHOT_TAG_KV_BUCKET,
    ] {
        jetstream
            .create_key_value(kv::Config {
                bucket: bucket.to_string(),
                ..Default::default()
            })
            .await?;
    }
    let tenant = tenant()?;
    assert_eq!(
        backend.list_leases(&tenant, &BTreeMap::new()).await?,
        json!({"leases": []})
    );
    assert_eq!(backend.list_snapshots(&tenant).await?["snapshots"], json!([]));
    assert_eq!(backend.list_tags(&tenant).await?["tags"], json!([]));
    assert!(serde_json::to_value(backend.list_capabilities().await)?["providers"].is_array());

    let cancel = CancellationToken::new();
    let (tx, _rx) = mpsc::channel(8);
    backend.spawn_dashboard_watchers(tenant, tx, cancel.clone());
    cancel.cancel();
    Ok(())
}
