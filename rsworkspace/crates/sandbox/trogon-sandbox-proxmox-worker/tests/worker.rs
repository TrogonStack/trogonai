mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::Method;
use base64::Engine as _;
use futures::StreamExt;
use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{
    LeaseId, ProviderDispatchPayload, ProviderKind, WorkerAuthorityRequest, WorkerAuthorityResponse,
    WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, subjects};

fn command_json(command: &[&str]) -> Value {
    json!({"command": command, "timeout_seconds": 10})
}

async fn grant(running: &Running, lease: &str, attempt: &str) -> TestResult {
    let reply = running.dispatch(lease, attempt, request_json()).await?;
    assert!(reply.get("sandbox_id").is_some(), "not granted: {reply}");
    Ok(())
}

#[tokio::test]
async fn dispatch_grants_a_lease_and_clones_a_vm() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    assert!(running.env.fake.clone_exists());
    assert!(running.env.provider.holds_lease(&LeaseId::new("lease-1")).await);
    running.stop().await
}

#[tokio::test]
async fn the_worker_announces_targeted_dispatch_in_its_heartbeat() -> TestResult {
    let env = Env::start().await?;
    let mut running = Running::start_with(env).await?;
    let mut beats = running
        .client
        .subscribe(subjects::worker_heartbeat(ProviderKind::Proxmox, &running.worker_id))
        .await?;
    running.client.flush().await?;
    let message = tokio::time::timeout(Duration::from_secs(40), beats.next())
        .await?
        .ok_or("no heartbeat")?;
    let beat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
    assert_eq!(beat.dispatch_protocol, WorkerDispatchProtocol::TargetedV1);
    assert_eq!(beat.capacity.max_concurrent_leases, 2);
    running.stop().await
}

#[tokio::test]
async fn dispatch_without_a_target_fails_to_launch() -> TestResult {
    let mut running = Running::start().await?;
    let subject = subjects::worker_dispatch(ProviderKind::Proxmox, &running.worker_id);
    let reply = running
        .ask(subject, &json!({"lease_id": "lease-1", "request": request_json()}))
        .await?;
    assert_eq!(reply["provider_error"], "targeted dispatch identity is required");
    assert!(!running.env.fake.clone_exists());
    running.stop().await
}

#[tokio::test]
async fn an_undecodable_dispatch_is_rejected() -> TestResult {
    let mut running = Running::start().await?;
    let subject = subjects::worker_dispatch(ProviderKind::Proxmox, &running.worker_id);
    let reply = running.ask(subject, &json!({"unexpected": true})).await?;
    assert!(reply["decode_error"].is_string(), "{reply}");
    running.stop().await
}

#[tokio::test]
async fn a_provider_failure_fails_the_launch() -> TestResult {
    let mut running = Running::start().await?;
    running.env.fake.with(|host| host.fail_start = true);
    let reply = running.dispatch("lease-1", "attempt-1", request_json()).await?;
    assert!(reply["provider_error"].is_string(), "{reply}");
    assert!(!running.env.fake.clone_exists());
    running.stop().await
}

#[tokio::test]
async fn a_repeated_dispatch_returns_the_same_receipt() -> TestResult {
    let mut running = Running::start().await?;
    let first = running.dispatch("lease-1", "attempt-1", request_json()).await?;
    let second = running.dispatch("lease-1", "attempt-1", request_json()).await?;
    assert_eq!(first, second);
    assert_eq!(first["attempt_id"], "attempt-1");
    assert_eq!(running.env.fake.count(&Method::POST, "/clone"), 1);
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
    assert_eq!(reply["exit_code"], 0);
    running.stop().await
}

#[tokio::test]
async fn run_command_trims_a_reply_larger_than_the_bus_allows() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(vec![255u8; 400_000]);
    running
        .env
        .fake
        .with(|host| host.capture_reply = Some(format!("0\n400000\n{encoded}\n\n0\n\n\n")));
    let reply = running
        .operation("run_command", "lease-1", command_json(&["cat", "big"]))
        .await?;
    assert_eq!(reply["outcome"], "completed", "{}", reply["outcome"]);
    assert_eq!(reply["truncated"], true);
    running.stop().await
}

#[tokio::test]
async fn write_files_and_read_file_roundtrip_large_content() -> TestResult {
    let bytes = pattern(512 * 1024);
    let env = Env::with_host(Host {
        file_data: bytes.clone(),
        ..Default::default()
    })
    .await?;
    let mut running = Running::start_with(env).await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let files = json!({"files": [{"path": "data", "content": bytes, "mode": 0o600}]});
    let written = running.operation("write_files", "lease-1", files).await?;
    assert_eq!(written["outcome"], "written", "{written}");
    let read = running
        .operation("read_file", "lease-1", json!({"path": "data"}))
        .await?;
    assert_eq!(read["outcome"], "content", "{}", read["outcome"]);
    assert_eq!(serde_json::from_value::<Vec<u8>>(read["data"].clone())?, bytes);
    assert_eq!(running.env.fake.count(&Method::POST, "/clone"), 1);
    let mut written = Vec::new();
    for call in running.env.fake.calls() {
        let argv = call.argv();
        if call.path.ends_with("/agent/exec") && argv.iter().any(|arg| arg.contains("base64 -d >>")) {
            let encoded = argv.last().ok_or("empty argv")?;
            written.extend(base64::engine::general_purpose::STANDARD.decode(encoded)?);
        }
    }
    assert_eq!(written, bytes);
    running.stop().await
}

#[tokio::test]
async fn read_file_reports_a_missing_file() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    running.env.fake.with(|host| host.file_missing = true);
    let missing = running
        .operation("read_file", "lease-1", json!({"path": "/tmp/a"}))
        .await?;
    assert_eq!(missing["outcome"], "not_found", "{missing}");
    running.stop().await
}

#[tokio::test]
async fn file_op_is_unsupported() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let reply = running
        .operation("file_op", "lease-1", json!({"op": {"op": "stat", "path": "/"}}))
        .await?;
    assert_ne!(reply["outcome"], "completed", "{reply}");
    running.stop().await
}

#[tokio::test]
async fn an_unknown_verb_is_denied() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let reply = running.operation("teleport", "lease-1", json!({})).await?;
    assert_eq!(reply["outcome"], "denied", "{reply}");
    running.stop().await
}

#[tokio::test]
async fn an_operation_on_a_lease_the_worker_does_not_hold_gets_no_answer() -> TestResult {
    let mut running = Running::start().await?;
    let outcome = tokio::time::timeout(
        Duration::from_millis(700),
        running.ask(
            "sandbox.provider.proxmox.run_command.stranger".to_owned(),
            &json!({"lease_id": "stranger", "command": ["true"], "timeout_seconds": 5}),
        ),
    )
    .await;
    assert!(!matches!(outcome, Ok(Ok(_))));
    running.stop().await
}

#[tokio::test]
async fn targeted_cleanup_releases_the_vm_over_nats() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    let cleanup = json!({
        "identity": {
            "lease_id": "lease-1",
            "provider": "proxmox",
            "target": {"worker_id": running.worker_id.as_str(), "attempt_id": "attempt-1"},
        },
        "intent": "release",
    });
    let subject = subjects::worker_cleanup(ProviderKind::Proxmox, &running.worker_id);
    for _ in 0..2 {
        let reply = running.ask(subject.clone(), &cleanup).await?;
        assert_eq!(reply["outcome"], "cleaned", "{reply}");
    }
    assert!(!running.env.fake.clone_exists());
    assert_eq!(running.env.provider.current_leases().await, 0);
    running.stop().await
}

#[tokio::test]
async fn stopping_the_worker_destroys_every_vm_it_holds() -> TestResult {
    let mut running = Running::start().await?;
    grant(&running, "lease-1", "attempt-1").await?;
    grant(&running, "lease-2", "attempt-2").await?;
    running.stop().await?;
    assert!(!running.env.fake.clone_exists());
    assert!(running.env.fake.with(|host| host.extra_vms.is_empty()));
    assert_eq!(running.env.provider.current_leases().await, 0);
    Ok(())
}

#[tokio::test]
async fn the_authority_check_reclaims_the_exact_vm_after_a_heartbeat_confirmed_it() -> TestResult {
    let env = Env::start().await?;
    let owner = env.provider.dispatch_ledger().worker_id().clone();
    let lease = LeaseId::new("authority-reclaim");
    let payload: ProviderDispatchPayload = serde_json::from_value(json!({
        "lease_id": lease.as_str(),
        "request": request_json(),
        "target": {"worker_id": owner.as_str(), "attempt_id": "authority-reclaim-attempt", "epoch": 7},
    }))?;
    env.provider.lease_dispatch(&payload).await?;
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let mut heartbeats = client
        .subscribe(subjects::worker_heartbeat(ProviderKind::Proxmox, &owner))
        .await?;
    let mut queries = client.subscribe(subjects::worker_authority_check(&owner)).await?;
    client.flush().await?;
    let checks = Arc::new(AtomicUsize::new(0));
    let (confirm, confirmed) = tokio::sync::oneshot::channel::<()>();
    let oracle = tokio::spawn({
        let (client, owner, checks) = (client.clone(), owner.clone(), checks.clone());
        async move {
            if confirmed.await.is_err() {
                return;
            }
            while let Some(message) = queries.next().await {
                let Ok(query) = serde_json::from_slice::<WorkerAuthorityRequest>(&message.payload) else {
                    continue;
                };
                if query.worker_id != owner {
                    continue;
                }
                checks.fetch_add(1, Ordering::SeqCst);
                let verdict = WorkerAuthorityResponse {
                    owned: Vec::new(),
                    not_assigned: query.lease_ids,
                };
                if let (Some(reply), Ok(body)) = (message.reply, serde_json::to_vec(&verdict)) {
                    let _ = client.publish(reply, body.into()).await;
                }
            }
        }
    });
    let mut authority = AuthorityConfig::new(AuthorityMode::Enforce);
    authority.interval = Duration::from_millis(10);
    authority.min_lease_age = Duration::ZERO;
    authority.confirmations = 1;
    authority.grace = Duration::ZERO;
    authority.request_timeout = Duration::from_millis(500);
    let mut running = Running::serve(env, nats, authority).await?;
    let message = tokio::time::timeout(Duration::from_secs(20), heartbeats.next())
        .await?
        .ok_or("heartbeat feed closed")?;
    let beat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
    assert_eq!(beat.worker_id, owner);
    assert_eq!(
        beat.capacity.current_leases, 1,
        "the worker reports its allocation before any verdict"
    );
    let _ = confirm.send(());
    let ledger = running.env.provider.dispatch_ledger().clone();
    tokio::time::timeout(Duration::from_secs(20), async {
        while running.env.provider.holds_lease(&lease).await || !ledger.try_states().await?.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    assert!(checks.load(Ordering::SeqCst) > 0);
    assert!(!running.env.fake.clone_exists());
    oracle.abort();
    running.stop().await
}
