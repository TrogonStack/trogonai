mod support;

use std::time::Duration;

use futures::StreamExt;
use serde_json::{Value, json};
use support::*;
use trogon_sandbox_contract::{LeaseId, LifecycleEvent, ProviderKind, WorkerId};
use trogon_sandbox_modal_worker::{ModalWorker, ModalWorkerError, NatsLifecycleEmitter};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, LifecycleEmitter, lifecycle_subject, subjects};

const WAIT: Duration = Duration::from_secs(20);

async fn eventually(mut done: impl AsyncFnMut() -> bool) -> TestResult {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !done().await {
        if tokio::time::Instant::now() > deadline {
            return Err("the condition never held".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// Sends a request and reports whether anything answered it quickly.
async fn silence(running: &Running, subject: String, payload: Vec<u8>) -> bool {
    tokio::time::timeout(
        Duration::from_millis(700),
        running.client.request(subject, payload.into()),
    )
    .await
    .is_err()
}

fn domain_subject(lease: &str) -> String {
    subjects::provider_get_domain(ProviderKind::Modal, &LeaseId::new(lease))
}

fn checkpoint_subject(lease: &str) -> String {
    subjects::provider_checkpoint_running(ProviderKind::Modal, &LeaseId::new(lease))
}

fn cancel_subject(lease: &str) -> String {
    subjects::provider_cancel(ProviderKind::Modal, &LeaseId::new(lease))
}

#[tokio::test]
async fn a_dispatched_lease_is_granted_once_and_reports_its_lifecycle() -> TestResult {
    let mut running = Running::start().await?;
    let mut events = running
        .client
        .subscribe(lifecycle_subject(&lease_id("lease-1")))
        .await?;
    running.client.flush().await?;

    let reply = running.dispatch("lease-1", request_json()).await?;
    assert_eq!(reply["sandbox_id"], "sb-1");
    let again = running.dispatch("lease-1", request_json()).await?;
    assert_eq!(again["sandbox_id"], "sb-1");
    assert_eq!(running.env.api.with(|state| state.created.len()), 1);

    let mut statuses = Vec::new();
    while statuses.len() < 4 {
        let event = tokio::time::timeout(WAIT, events.next()).await?.ok_or("no event")?;
        let event: Value = serde_json::from_slice(&event.payload)?;
        statuses.push(event["status"].as_str().unwrap_or_default().to_owned());
    }
    assert_eq!(statuses, ["started", "phase_started", "phase_completed", "completed"]);
    running.stop().await
}

#[tokio::test]
async fn a_failed_launch_is_reported_to_the_broker_and_the_lifecycle() -> TestResult {
    let mut running = Running::start().await?;
    let mut events = running
        .client
        .subscribe(lifecycle_subject(&lease_id("lease-1")))
        .await?;
    running.client.flush().await?;
    running
        .env
        .api
        .with(|state| state.create_failure = Some(Failure::Other));

    let reply = running.dispatch("lease-1", request_json()).await?;
    assert!(reply["provider_error"].is_string());
    let mut statuses = Vec::new();
    while statuses.len() < 4 {
        let event = tokio::time::timeout(WAIT, events.next()).await?.ok_or("no event")?;
        let event: Value = serde_json::from_slice(&event.payload)?;
        statuses.push(event["status"].as_str().unwrap_or_default().to_owned());
    }
    assert_eq!(statuses, ["started", "phase_started", "phase_failed", "failed"]);
    running.stop().await
}

#[tokio::test]
async fn a_lease_id_that_is_no_sandbox_id_skips_the_placeholder_events() -> TestResult {
    let mut running = Running::start().await?;
    running
        .env
        .api
        .with(|state| state.create_failure = Some(Failure::Other));
    let reply = running.dispatch("bad\u{1}lease", request_json()).await?;
    assert!(reply["provider_error"].is_string());
    running.env.api.with(|state| state.create_failure = None);
    let granted = running.dispatch("bad\u{1}lease", request_json()).await?;
    assert!(granted["sandbox_id"].is_string());
    running.stop().await
}

#[tokio::test]
async fn a_dispatch_that_cannot_be_decoded_is_rejected() -> TestResult {
    let mut running = Running::start().await?;
    let reply = running
        .ask("sandbox.provider.modal.lease".to_owned(), &json!({"unexpected": true}))
        .await?;
    assert!(reply["decode_error"].is_string());
    running
        .client
        .publish("sandbox.provider.modal.lease", "{}".into())
        .await?;
    assert_eq!(running.env.api.with(|state| state.created.len()), 0);
    running.stop().await
}

#[tokio::test]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let mut running = Running::start().await?;
    let payload = serde_json::to_vec(&json!({"lease_id": "lease-1", "request": request_json()}))?;
    running
        .client
        .publish("sandbox.provider.modal.lease", payload.into())
        .await?;
    eventually(async || running.env.provider.current_leases().await == 1).await?;
    running.stop().await
}

#[tokio::test]
async fn a_cancel_ends_the_lease_and_a_failed_cancel_keeps_it() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch("lease-1", request_json()).await?;
    running.dispatch("lease-2", request_json()).await?;

    running
        .env
        .api
        .with(|state| state.terminate_failure = Some(Failure::Other));
    running.client.publish(cancel_subject("lease-2"), "".into()).await?;
    running.client.flush().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(running.env.provider.current_leases().await, 2);

    running.env.api.with(|state| state.terminate_failure = None);
    running.client.publish(cancel_subject("lease-1"), "".into()).await?;
    running.client.publish(cancel_subject("unknown"), "".into()).await?;
    eventually(async || running.env.provider.current_leases().await == 1).await?;

    let regranted = running.dispatch("lease-1", request_json()).await?;
    assert!(regranted["sandbox_id"].is_string());
    running.stop().await
}

/// Pins today's behaviour, pending a decision on which one every worker should have: the cancel handler forgets the
/// grant before it cancels, so a redispatch after a failed cancel is not answered with the grant. The provider still
/// holds the sandbox it could not terminate, so it refuses to launch another and the redispatch gets a provider error.
/// The k8s, v8-wasm and kubevirt workers keep the grant until a cancel succeeds and answer with it instead.
#[tokio::test]
async fn a_redispatch_after_a_failed_cancel_is_refused_while_the_sandbox_is_held() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch("lease-stuck", request_json()).await?;
    running
        .env
        .api
        .with(|state| state.terminate_failure = Some(Failure::Other));

    running.client.publish(cancel_subject("lease-stuck"), "".into()).await?;
    running.client.flush().await?;
    // Redispatch only once the terminate has actually failed, so the cancel handler has forgotten the grant.
    tokio::time::timeout(Duration::from_secs(10), async {
        while running.env.api.with(|state| state.terminated.len()) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    let reply = running.dispatch("lease-stuck", request_json()).await?;

    assert!(
        reply["provider_error"]
            .as_str()
            .is_some_and(|error| error.contains("settle cleanup before reallocating")),
        "{reply}"
    );
    assert_eq!(running.env.provider.current_leases().await, 1);
    running.stop().await
}

#[tokio::test]
async fn a_held_port_is_resolved_and_a_stranger_gets_no_answer() -> TestResult {
    let mut running = Running::start().await?;
    let mut wire = request_json();
    wire["spec"]["exposed_ports"] = json!([8080]);
    running.dispatch("lease-1", wire).await?;
    running.env.api.with(|state| {
        state.tunnels = vec![trogon_sandbox_modal_worker::ModalTunnel {
            port: 8080,
            host: "tunnel.modal.host".to_owned(),
            tls_port: 443,
            url: "https://tunnel.modal.host".to_owned(),
        }]
    });

    let reply = running
        .ask(domain_subject("lease-1"), &json!({"lease_id": "lease-1", "port": 8080}))
        .await?;
    assert!(reply.to_string().contains("tunnel.modal.host"));
    assert!(
        silence(
            &running,
            domain_subject("other"),
            serde_json::to_vec(&json!({"lease_id": "other", "port": 8080}))?
        )
        .await
    );
    assert!(silence(&running, domain_subject("lease-1"), b"not json".to_vec()).await);
    running.client.publish(domain_subject("lease-1"), "".into()).await?;
    running.client.flush().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.stop().await
}

#[tokio::test]
async fn a_checkpoint_names_a_native_snapshot_or_is_refused() -> TestResult {
    let mut running = Running::start().await?;
    running.dispatch("lease-1", request_json()).await?;
    let target =
        json!({"registry": "zot.local", "repository": "snapshots/rootfs", "artifact_type": "sandbox_rootfs_v1"});

    let native = running
        .ask(
            checkpoint_subject("lease-1"),
            &json!({"lease_id": "lease-1", "target": target, "native_name": "snap"}),
        )
        .await?;
    assert!(native.to_string().contains("im-1"));
    let refused = running
        .ask(
            checkpoint_subject("lease-1"),
            &json!({"lease_id": "lease-1", "target": target}),
        )
        .await?;
    assert!(refused.to_string().contains("provider-native"));
    let stranger = json!({"lease_id": "other", "target": target, "native_name": "snap"});
    assert!(silence(&running, checkpoint_subject("other"), serde_json::to_vec(&stranger)?).await);
    assert!(silence(&running, checkpoint_subject("lease-1"), b"not json".to_vec()).await);
    running.client.publish(checkpoint_subject("lease-1"), "".into()).await?;
    running.client.flush().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.stop().await
}

#[tokio::test]
async fn the_worker_announces_itself_while_it_serves() -> TestResult {
    let nats = Nats::start().await;
    let watcher = nats.client().await?;
    let mut heartbeats = watcher
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Modal))
        .await?;
    watcher.flush().await?;
    let mut running = Running::start_on(nats, Env::start()?, None).await?;
    let beat = tokio::time::timeout(WAIT, heartbeats.next())
        .await?
        .ok_or("no heartbeat")?;
    let beat: Value = serde_json::from_slice(&beat.payload)?;
    assert_eq!(beat["provider"], "modal");
    assert_eq!(beat["worker_id"], "worker-1");
    assert_eq!(beat["capacity"]["max_concurrent_leases"], 3);
    running.stop().await
}

#[tokio::test]
async fn shutdown_abandons_requests_that_outlast_the_drain_timeout() -> TestResult {
    let env = Env::start()?;
    env.api.with(|state| state.never_running = true);
    let mut running = Running::start_with(env, Some(Duration::from_millis(200))).await?;
    let payload = serde_json::to_vec(&json!({"lease_id": "lease-1", "request": request_json()}))?;
    running
        .client
        .publish("sandbox.provider.modal.lease", payload.into())
        .await?;
    running.client.flush().await?;
    eventually(async || running.env.api.with(|state| !state.created.is_empty())).await?;
    running.stop().await
}

#[tokio::test]
async fn a_provider_bound_to_another_worker_cannot_be_served() -> TestResult {
    let env = Env::start()?;
    env.provider.bind_worker(&WorkerId::new("someone-else")?)?;
    let nats = Nats::start().await;
    let worker = ModalWorker::new(
        nats.client().await?,
        env.provider.clone(),
        WorkerId::new("worker-1")?,
        AuthorityConfig::new(AuthorityMode::Off),
    );
    let outcome = worker.run(std::future::pending()).await;
    assert!(matches!(outcome, Err(ModalWorkerError::Provider(_))));
    Ok(())
}

#[tokio::test]
async fn an_emitter_whose_client_is_gone_only_warns() -> TestResult {
    let nats = Nats::start().await;
    let client = nats.client().await?;
    let emitter = NatsLifecycleEmitter::new(client.clone(), &lease_id("lease-1"));
    client.drain().await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    emitter
        .emit(LifecycleEvent::Cancelled {
            sandbox_id: sandbox_id("sb-1")?,
        })
        .await;
    Ok(())
}
