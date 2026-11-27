mod nats_common;
mod support;

use std::time::Duration;

use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::{Value, json};
use support::{Rig, TestResult, cleanup_request, dispatch_json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, subjects};
use trogon_sandbox_space_worker::{SpaceWorker, SpaceWorkerError};

const WAIT: Duration = Duration::from_secs(10);
const SILENT: Duration = Duration::from_millis(400);

struct Running {
    _server: nats_common::Server,
    client: Client,
    stop: CancellationToken,
    task: JoinHandle<Result<(), SpaceWorkerError>>,
}

impl Running {
    async fn start(rig: &Rig, drain: Duration) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with(rig, drain, AuthorityConfig::new(AuthorityMode::Off)).await
    }

    async fn start_with(
        rig: &Rig,
        drain: Duration,
        authority: AuthorityConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (server, client) = nats_common::start_nats().await?;
        let worker_client = server.client().await?;
        let heartbeat = subjects::worker_heartbeat(ProviderKind::Space, rig.provider.dispatch_ledger().worker_id());
        let mut beats = client.subscribe(heartbeat).await?;
        client.flush().await?;
        let stop = CancellationToken::new();
        let worker = SpaceWorker::new(worker_client, rig.provider.clone(), authority).with_drain(drain);
        let task = tokio::spawn(worker.run(stop.clone()));
        tokio::time::timeout(WAIT, beats.next()).await?.ok_or("no heartbeat")?;
        Ok(Self {
            _server: server,
            client,
            stop,
            task,
        })
    }

    async fn ask(&self, subject: String, body: &Value) -> Result<Value, Box<dyn std::error::Error>> {
        let reply = tokio::time::timeout(
            WAIT,
            self.client.request(subject, Bytes::from(serde_json::to_vec(body)?)),
        )
        .await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    async fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        self.stop.cancel();
        tokio::time::timeout(WAIT, self.task).await???;
        Ok(())
    }
}

#[tokio::test]
async fn serves_dispatches_cleanups_and_operations() -> TestResult {
    let rig = Rig::start().await?;
    let running = Running::start(&rig, Duration::from_secs(5)).await?;
    let worker = rig.worker();
    let id = rig.provider.dispatch_ledger().worker_id().clone();

    let granted = running
        .ask(
            subjects::worker_dispatch(ProviderKind::Space, &id),
            &dispatch_json("lease-1", Some((&worker, "attempt-1"))),
        )
        .await?;
    assert!(granted.to_string().contains("lease-1"));

    let paused = running
        .ask(
            "sandbox.provider.space.pause.lease-1".into(),
            &json!({"lease_id": "lease-1"}),
        )
        .await?;
    assert!(paused.to_string().to_lowercase().contains("done"));

    let cleaned = running
        .ask(
            subjects::worker_cleanup(ProviderKind::Space, &id),
            &serde_json::to_value(cleanup_request(&rig, "lease-1", "attempt-1", "release")?)?,
        )
        .await?;
    assert!(cleaned.to_string().to_lowercase().contains("clean"));
    running.finish().await
}

#[tokio::test]
async fn ignores_requests_it_cannot_or_need_not_answer() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease("lease-1").await?;
    let running = Running::start(&rig, Duration::from_secs(5)).await?;
    let id = rig.provider.dispatch_ledger().worker_id().clone();
    let worker = rig.worker();

    let client = &running.client;
    let body = |value: &Value| -> Result<Bytes, serde_json::Error> { Ok(serde_json::to_vec(value)?.into()) };
    client
        .publish(
            subjects::worker_dispatch(ProviderKind::Space, &id),
            body(&dispatch_json("lease-9", Some((&worker, "attempt-9"))))?,
        )
        .await?;
    client
        .publish(
            subjects::worker_cleanup(ProviderKind::Space, &id),
            body(&serde_json::to_value(cleanup_request(
                &rig,
                "lease-1",
                "attempt-1",
                "release",
            )?)?)?,
        )
        .await?;
    client
        .publish(
            "sandbox.provider.space.pause.lease-1",
            body(&json!({"lease_id": "lease-1"}))?,
        )
        .await?;
    client
        .publish(
            "sandbox.provider.space.pause.lease-2",
            body(&json!({"lease_id": "lease-2"}))?,
        )
        .await?;

    let garbage = tokio::time::timeout(
        SILENT,
        client.request(
            subjects::worker_cleanup(ProviderKind::Space, &id),
            Bytes::from_static(b"nope"),
        ),
    )
    .await;
    drop(garbage);
    let mismatch = tokio::time::timeout(
        SILENT,
        client.request(
            "sandbox.provider.space.pause.lease-1".to_owned(),
            body(&json!({"lease_id": "other"}))?,
        ),
    )
    .await;
    drop(mismatch);
    let unserved = tokio::time::timeout(
        SILENT,
        client.request("sandbox.provider.space.pause.lease-2".to_owned(), body(&json!({}))?),
    )
    .await;
    drop(unserved);
    running.finish().await
}

#[tokio::test]
async fn abandons_requests_that_outlive_the_drain() -> TestResult {
    let rig = Rig::start().await?;
    rig.space.state().create_delay = Duration::from_secs(3);
    let running = Running::start(&rig, Duration::from_millis(50)).await?;
    let id = rig.provider.dispatch_ledger().worker_id().clone();
    let worker = rig.worker();
    running
        .client
        .publish(
            subjects::worker_dispatch(ProviderKind::Space, &id),
            serde_json::to_vec(&dispatch_json("lease-1", Some((&worker, "attempt-1"))))?.into(),
        )
        .await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    running.finish().await
}

#[tokio::test]
async fn cleans_leases_it_has_lost_authority_over() -> TestResult {
    let rig = Rig::start().await?;
    rig.lease_targeted("lease-1", "attempt-1").await?;
    let authority = AuthorityConfig {
        interval: Duration::from_millis(50),
        partition_after: Duration::from_millis(100),
        grace: Duration::from_millis(100),
        min_lease_age: Duration::ZERO,
        confirmations: 1,
        request_timeout: Duration::from_millis(200),
        ..AuthorityConfig::new(AuthorityMode::Enforce)
    };
    let running = Running::start_with(&rig, Duration::from_secs(5), authority).await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !rig.space.state().sandboxes.is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "the lease was never cleaned");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    running.finish().await
}
