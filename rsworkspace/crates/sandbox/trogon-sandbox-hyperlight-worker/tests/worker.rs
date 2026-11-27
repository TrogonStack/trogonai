mod support;

use std::time::Duration;

use async_nats::Client;
use async_nats::jetstream::stream;
use bytes::Bytes;
use futures::StreamExt;
use serde_json::Value;
use support::{Rig, TestResult, dispatch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ProviderKind, WorkerId};
use trogon_sandbox_hyperlight_worker::{HyperlightWorker, HyperlightWorkerError};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, subjects};

const WAIT: Duration = Duration::from_secs(10);

struct Running {
    _server: JetStreamTestServer,
    client: Client,
    rig: Rig,
    stop: CancellationToken,
    task: JoinHandle<Result<(), HyperlightWorkerError>>,
}

impl Running {
    async fn start(with_stream: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        if with_stream {
            server
                .jetstream()
                .await
                .create_stream(stream::Config {
                    name: "EVIDENCE".into(),
                    subjects: vec!["sandbox.evidence.>".into()],
                    ..Default::default()
                })
                .await?;
        }
        let rig = Rig::start().await?;
        let worker_id = WorkerId::new("worker-1")?;
        let mut beats = client
            .subscribe(subjects::worker_heartbeat(ProviderKind::Hyperlight, &worker_id))
            .await?;
        client.flush().await?;
        let stop = CancellationToken::new();
        let worker = HyperlightWorker::new(
            server.client().await,
            rig.provider.clone(),
            AuthorityConfig::new(AuthorityMode::Off),
            worker_id,
        );
        let task = tokio::spawn(worker.run(stop.clone()));
        tokio::time::timeout(WAIT, beats.next()).await?.ok_or("no heartbeat")?;
        Ok(Self {
            _server: server,
            client,
            rig,
            stop,
            task,
        })
    }

    async fn ask(&self, subject: String, body: Bytes) -> Result<Value, Box<dyn std::error::Error>> {
        let reply = tokio::time::timeout(WAIT, self.client.request(subject, body)).await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    async fn dispatch(&self, id: &str, name: &str) -> Result<Value, Box<dyn std::error::Error>> {
        let body = Bytes::from(serde_json::to_vec(&dispatch(id, name)?)?);
        self.ask(subjects::provider_dispatch(ProviderKind::Hyperlight), body)
            .await
    }

    async fn finish(self) -> Result<(), Box<dyn std::error::Error>> {
        self.stop.cancel();
        tokio::time::timeout(WAIT, self.task).await???;
        Ok(())
    }
}

async fn until(mut done: impl FnMut() -> bool) -> TestResult {
    tokio::time::timeout(WAIT, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn serves_leases_cancels_and_redispatches() -> TestResult {
    let running = Running::start(true).await?;
    let hypervisor = running.rig.hypervisor.clone();
    let provider = running.rig.provider.clone();

    let mut lifecycle = running.client.subscribe("sandbox.evidence.l1.lifecycle").await?;
    running.client.flush().await?;
    let granted = running.dispatch("l1", "echo").await?;
    assert_eq!(granted["sandbox_id"], "hl-l1");
    let again = running.dispatch("l1", "echo").await?;
    assert_eq!(again, granted);
    assert_eq!(hypervisor.loads(), 1);
    let started = tokio::time::timeout(WAIT, lifecycle.next())
        .await?
        .ok_or("no lifecycle event")?;
    assert!(String::from_utf8_lossy(&started.payload).contains("\"l1\""));

    running
        .client
        .publish("sandbox.provider.hyperlight.cancel.l1", Bytes::new())
        .await?;
    until(|| provider.active_lease_count() == 0).await?;
    let relaunched = running.dispatch("l1", "echo").await?;
    assert_eq!(relaunched["sandbox_id"], "hl-l1");
    assert_eq!(hypervisor.loads(), 2);

    let missing = running.dispatch("l2", "missing").await?;
    assert!(
        missing["provider_error"]
            .as_str()
            .is_some_and(|text| text.contains("not found"))
    );
    let garbage = running
        .ask(
            subjects::provider_dispatch(ProviderKind::Hyperlight),
            Bytes::from_static(b"nope"),
        )
        .await?;
    assert!(garbage["decode_error"].is_string());

    hypervisor.fail_unload(true);
    running
        .client
        .publish("sandbox.provider.hyperlight.cancel.l1", Bytes::new())
        .await?;
    until(|| provider.active_lease_count() == 0).await?;
    running.finish().await
}

#[tokio::test]
async fn keeps_serving_when_lifecycle_events_have_nowhere_to_go() -> TestResult {
    let running = Running::start(false).await?;
    let granted = running.dispatch("l1", "echo").await?;
    assert_eq!(granted["sandbox_id"], "hl-l1");
    running.finish().await
}

#[tokio::test]
async fn ignores_a_dispatch_nobody_waits_on() -> TestResult {
    let running = Running::start(true).await?;
    let body = Bytes::from(serde_json::to_vec(&dispatch("l1", "echo")?)?);
    running
        .client
        .publish(subjects::provider_dispatch(ProviderKind::Hyperlight), body)
        .await?;
    until(|| running.rig.provider.active_lease_count() == 1).await?;
    running.finish().await
}

#[tokio::test]
async fn refuses_to_serve_for_a_second_worker() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let rig = Rig::start().await?;
    rig.provider.bind_worker(&WorkerId::new("first")?)?;
    let worker = HyperlightWorker::new(
        server.client().await,
        rig.provider.clone(),
        AuthorityConfig::new(AuthorityMode::Off),
        WorkerId::new("second")?,
    );
    let failed = worker.run(CancellationToken::new()).await;
    assert!(matches!(failed, Err(HyperlightWorkerError::Bind(_))));
    Ok(())
}

#[tokio::test]
async fn a_cancel_that_lands_mid_launch_waits_for_the_grant() -> TestResult {
    let running = Running::start(true).await?;
    running.rig.hypervisor.delay_load(600);
    let body = Bytes::from(serde_json::to_vec(&dispatch("l1", "echo")?)?);
    let pending = running
        .client
        .request(subjects::provider_dispatch(ProviderKind::Hyperlight), body);
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        running
            .client
            .publish("sandbox.provider.hyperlight.cancel.l1", Bytes::new())
            .await
    };
    let (reply, cancelled) = tokio::join!(pending, cancel);
    cancelled?;
    assert!(String::from_utf8_lossy(&reply?.payload).contains("hl-l1"));
    let provider = running.rig.provider.clone();
    until(|| provider.active_lease_count() == 0).await?;
    running.finish().await
}
