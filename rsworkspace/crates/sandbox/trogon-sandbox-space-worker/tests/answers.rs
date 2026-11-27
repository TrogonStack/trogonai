mod support;

use serde_json::{Value, json};
use support::{Plan, Rig, TestResult, cleanup_request, dispatch_json, exit_plan};
use trogon_sandbox_contract::{ProviderCleanupResponse, ProviderDispatchReply, WorkerDispatchProtocol};
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_space_worker::{OperationError, OperationSubject, OperationVerb};

fn subject(verb: &str, lease: &str) -> Result<OperationSubject, Box<dyn std::error::Error>> {
    Ok(OperationSubject::parse(&format!(
        "sandbox.provider.space.{verb}.{lease}"
    ))?)
}

async fn ask(rig: &Rig, verb: &str, lease: &str, body: Value) -> Result<Value, Box<dyn std::error::Error>> {
    Ok(rig
        .provider
        .answer_operation(&subject(verb, lease)?, &serde_json::to_vec(&body)?)
        .await?)
}

#[tokio::test]
async fn grants_a_verified_targeted_dispatch() -> TestResult {
    let rig = Rig::start().await?;
    let worker = rig.worker();
    let body = serde_json::to_vec(&dispatch_json("lease-1", Some((&worker, "attempt-1"))))?;
    let reply = rig.provider.answer_dispatch(&body).await;
    assert!(matches!(reply, ProviderDispatchReply::Granted(_)));
    Ok(())
}

#[tokio::test]
async fn rejects_a_dispatch_it_cannot_decode() -> TestResult {
    let rig = Rig::start().await?;
    let reply = rig.provider.answer_dispatch(b"not json").await;
    assert!(matches!(reply, ProviderDispatchReply::DispatchRejected { .. }));
    Ok(())
}

#[tokio::test]
async fn refuses_a_dispatch_without_a_target() -> TestResult {
    let rig = Rig::start().await?;
    let body = serde_json::to_vec(&dispatch_json("lease-1", None))?;
    let reply = rig.provider.answer_dispatch(&body).await;
    assert!(matches!(reply, ProviderDispatchReply::LaunchFailed { .. }));
    Ok(())
}

#[tokio::test]
async fn reports_a_launch_that_failed() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_status = Some(400);
    let worker = rig.worker();
    let body = serde_json::to_vec(&dispatch_json("lease-1", Some((&worker, "attempt-1"))))?;
    let reply = rig.provider.answer_dispatch(&body).await;
    assert!(matches!(reply, ProviderDispatchReply::LaunchFailed { .. }));
    Ok(())
}

#[tokio::test]
async fn answers_a_cleanup_and_refuses_garbage() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease_targeted("lease-1", "attempt-1").await?;
    let body = serde_json::to_vec(&cleanup_request(&rig, "lease-1", "attempt-1", "release")?)?;
    let response = rig.provider.answer_cleanup(&body).await?;
    assert!(matches!(response, ProviderCleanupResponse::Cleaned { .. }));
    assert!(rig.provider.answer_cleanup(b"{").await.is_err());
    Ok(())
}

#[tokio::test]
async fn the_heartbeat_counts_the_leases_it_holds() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let heartbeat = rig.provider.heartbeat().await;
    assert_eq!(heartbeat.capacity.current_leases, 1);
    assert_eq!(heartbeat.capacity.max_concurrent_leases, 4);
    assert_eq!(heartbeat.worker_id.as_str(), rig.worker());
    assert!(matches!(
        heartbeat.dispatch_protocol,
        WorkerDispatchProtocol::TargetedV1
    ));
    Ok(())
}

#[tokio::test]
async fn serves_requests_for_held_leases_only() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    assert!(rig.provider.serves_operation(&subject("pause", "lease-1")?).await);
    assert!(!rig.provider.serves_operation(&subject("pause", "lease-2")?).await);
    assert!(!rig.provider.serves_operation(&subject("cancel", "lease-1")?).await);
    assert!(!rig.provider.serves_operation(&subject("purge", "lease-1")?).await);
    Ok(())
}

#[tokio::test]
async fn parses_and_names_every_verb() -> TestResult {
    for token in [
        "checkpoint_running",
        "snapshot",
        "get_domain",
        "pause",
        "resume",
        "suspend",
        "wake",
        "run_command",
        "write_files",
        "read_file",
        "file_op",
        "cancel",
        "purge",
        "mystery",
    ] {
        assert_eq!(OperationVerb::parse(token).as_str(), token);
    }
    assert_eq!(subject("pause", "lease-1")?.lease().as_str(), "lease-1");
    assert!(OperationSubject::parse("sandbox.provider.space.pause").is_err());
    assert!(OperationSubject::parse("sandbox.provider.space..lease").is_err());
    assert!(OperationSubject::parse("sandbox.provider.space.pause.").is_err());
    assert!(OperationSubject::parse("sandbox.provider.space.pause.a.b").is_err());
    Ok(())
}

#[tokio::test]
async fn refuses_a_body_for_another_lease_or_that_is_not_json() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let mismatch = rig
        .provider
        .answer_operation(&subject("pause", "lease-1")?, br#"{"lease_id":"other"}"#)
        .await;
    assert!(matches!(mismatch, Err(OperationError::LeaseMismatch)));
    let garbage = rig
        .provider
        .answer_operation(&subject("pause", "lease-1")?, b"nope")
        .await;
    assert!(matches!(garbage, Err(OperationError::Payload(_))));
    Ok(())
}

#[tokio::test]
async fn answers_the_verbs_that_have_a_fixed_reply() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let body = json!({"lease_id": "lease-1"});
    for verb in ["checkpoint_running", "snapshot"] {
        let reply = ask(&rig, verb, "lease-1", body.clone()).await?;
        assert!(reply.to_string().contains("unsupported"));
    }
    let domain = ask(&rig, "get_domain", "lease-1", body.clone()).await?;
    assert!(domain.to_string().contains("port_not_exposed"));
    for verb in ["cancel", "purge", "mystery"] {
        let reply = ask(&rig, verb, "lease-1", body.clone()).await?;
        assert_eq!(reply["outcome"], "denied");
    }
    Ok(())
}

#[tokio::test]
async fn pauses_and_resumes_through_the_operations() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let body = json!({"lease_id": "lease-1"});
    let paused = ask(&rig, "pause", "lease-1", body.clone()).await?;
    assert!(paused.to_string().to_lowercase().contains("done"));
    let resumed = ask(&rig, "resume", "lease-1", body).await?;
    assert!(resumed.to_string().to_lowercase().contains("done"));
    Ok(())
}

#[tokio::test]
async fn suspends_and_wakes_only_at_memory_level() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let memory = json!({"lease_id": "lease-1", "generation": 1, "level": "memory"});
    let disk = json!({"lease_id": "lease-1", "generation": 1, "level": "disk"});
    let suspended = ask(&rig, "suspend", "lease-1", memory.clone()).await?;
    assert!(suspended.to_string().to_lowercase().contains("done"));
    let woken = ask(&rig, "wake", "lease-1", memory).await?;
    assert!(woken.to_string().to_lowercase().contains("done"));
    assert!(
        ask(&rig, "suspend", "lease-1", disk.clone())
            .await?
            .to_string()
            .contains("unsupported")
    );
    assert!(
        ask(&rig, "wake", "lease-1", disk)
            .await?
            .to_string()
            .contains("unsupported")
    );
    Ok(())
}

#[tokio::test]
async fn runs_commands_and_moves_files_through_the_operations() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    rig.space.state().next_plans.push_back(exit_plan(b"", 0));
    let run = ask(
        &rig,
        "run_command",
        "lease-1",
        json!({"lease_id": "lease-1", "command": ["true"], "timeout_seconds": 5}),
    )
    .await?;
    assert!(run.to_string().contains("exit"));
    let written = ask(
        &rig,
        "write_files",
        "lease-1",
        json!({"lease_id": "lease-1", "files": [{"path": "/a", "content": [1, 2, 3]}]}),
    )
    .await?;
    assert!(written.to_string().to_lowercase().contains("written"));
    let read = ask(
        &rig,
        "read_file",
        "lease-1",
        json!({"lease_id": "lease-1", "path": "/a"}),
    )
    .await?;
    assert_eq!(read["data"], json!([1, 2, 3]));
    let missing = ask(
        &rig,
        "read_file",
        "lease-1",
        json!({"lease_id": "lease-1", "path": "/nope"}),
    )
    .await?;
    assert!(missing.to_string().to_lowercase().contains("not"));
    let file_op = ask(
        &rig,
        "file_op",
        "lease-1",
        json!({"lease_id": "lease-1", "op": {"op": "stat", "path": "/a"}}),
    )
    .await?;
    assert!(file_op.to_string().contains("unsupported"));
    Ok(())
}

#[tokio::test]
async fn reports_a_read_the_backend_refused() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    {
        let mut state = rig.space.state();
        state.fs.insert("/a".into(), vec![1]);
        state.garbage_read = true;
    }
    let read = ask(
        &rig,
        "read_file",
        "lease-1",
        json!({"lease_id": "lease-1", "path": "/a"}),
    )
    .await?;
    assert_eq!(read["outcome"], "denied");
    assert_eq!(rig.provider.kind(), trogon_sandbox_contract::ProviderKind::Space);
    Ok(())
}

#[tokio::test]
async fn reports_a_write_the_backend_refused() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    rig.space.state().fail_script = Some("base64 -d");
    let written = ask(
        &rig,
        "write_files",
        "lease-1",
        json!({"lease_id": "lease-1", "files": [{"path": "/a", "content": [1, 2, 3]}]}),
    )
    .await?;
    assert!(written.to_string().to_lowercase().contains("denied"));
    Ok(())
}

#[allow(dead_code)]
fn unused(_: Plan) {}
