mod support;

use std::sync::Arc;
use std::time::Duration;

use async_nats::Client;
use bytes::Bytes;
use serde_json::{Value, json};
use support::TestResult;
use support::lease::{Lease, snapshot_image};
use support::provider::Fixture;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_aca_worker::{WorkerError, run_worker};
use trogon_sandbox_contract::{LeaseId, ProviderKind, ProviderLease, WorkerId};
use trogon_sandbox_protocol::subjects::{provider_cancel, provider_dispatch};

const WAIT: Duration = Duration::from_secs(15);

struct Harness {
    _server: JetStreamTestServer,
    client: Client,
    cancel: CancellationToken,
    worker: JoinHandle<Result<(), WorkerError>>,
    fixture: Fixture,
}

impl Harness {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let fixture = Fixture::new()?;
        let cancel = CancellationToken::new();
        let worker = tokio::spawn(run_worker(
            client.clone(),
            fixture.provider.clone(),
            WorkerId::new("worker-under-test")?,
            cancel.clone(),
        ));
        let harness = Self {
            _server: server,
            client,
            cancel,
            worker,
            fixture,
        };
        harness.wait_until_serving().await?;
        Ok(harness)
    }

    /// A malformed dispatch is answered only once the worker subscribed to dispatches.
    async fn wait_until_serving(&self) -> TestResult {
        tokio::time::timeout(WAIT, async {
            while tokio::time::timeout(
                Duration::from_millis(300),
                self.client.request(
                    provider_dispatch(ProviderKind::AzureContainerApps),
                    Bytes::from_static(b"probe"),
                ),
            )
            .await
            .is_err()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await?;
        // The cancel subscription follows the dispatch one, so give it a moment to settle.
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    }

    async fn request(&self, body: Vec<u8>) -> Result<Value, Box<dyn std::error::Error>> {
        let reply = tokio::time::timeout(
            WAIT,
            self.client
                .request(provider_dispatch(ProviderKind::AzureContainerApps), Bytes::from(body)),
        )
        .await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    async fn dispatch(&self, lease: &str, request: Value) -> Result<Value, Box<dyn std::error::Error>> {
        self.request(serde_json::to_vec(&json!({ "lease_id": lease, "request": request }))?)
            .await
    }

    async fn cancel_lease(&self, lease: &str) -> TestResult {
        self.client
            .publish(
                provider_cancel(ProviderKind::AzureContainerApps, &LeaseId::new(lease)),
                Bytes::new(),
            )
            .await?;
        Ok(())
    }

    async fn stop(self) -> TestResult {
        self.cancel.cancel();
        tokio::time::timeout(WAIT, self.worker).await???;
        Ok(())
    }
}

async fn eventually<F, Fut>(mut done: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(WAIT, async {
        while !done().await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

fn empty_lease() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(Lease::empty().json())
}

#[tokio::test]
async fn a_dispatched_lease_replies_with_the_grant_stamped_with_the_worker() -> TestResult {
    let harness = Harness::start().await?;

    let reply = harness.dispatch("lease-1", empty_lease()?).await?;

    let grant: ProviderLease = serde_json::from_value(reply)?;
    assert!(grant.sandbox_id.as_str().starts_with("aca-"));
    assert_eq!(grant.worker_id, Some(WorkerId::new("worker-under-test")?));
    assert_eq!(harness.fixture.provider.active_lease_count().await, 1);
    harness.stop().await
}

#[tokio::test]
async fn a_redispatched_lease_launches_once_until_it_is_cancelled() -> TestResult {
    let harness = Harness::start().await?;

    let first = harness.dispatch("lease-redispatch", empty_lease()?).await?;
    let second = harness.dispatch("lease-redispatch", empty_lease()?).await?;
    assert_eq!(first, second);
    assert_eq!(harness.fixture.aca.executed().len(), 2);

    let provider = harness.fixture.provider.clone();
    harness.cancel_lease("lease-redispatch").await?;
    eventually(|| {
        let provider = provider.clone();
        async move { provider.active_lease_count().await == 0 }
    })
    .await?;
    let third = harness.dispatch("lease-redispatch", empty_lease()?).await?;

    assert_ne!(first, third);
    harness.stop().await
}

#[tokio::test]
async fn a_failed_launch_is_reported_to_the_dispatcher() -> TestResult {
    let harness = Harness::start().await?;

    let reply = harness
        .dispatch("lease-fails", Lease::empty().image(snapshot_image()).json())
        .await?;

    assert!(
        reply["provider_error"]
            .as_str()
            .is_some_and(|error| error.contains("snapshot")),
        "{reply}"
    );
    harness.stop().await
}

#[tokio::test]
async fn a_malformed_dispatch_is_answered_with_a_decode_error() -> TestResult {
    let harness = Harness::start().await?;

    let reply = harness.request(b"not json".to_vec()).await?;

    assert!(reply.get("decode_error").is_some(), "{reply}");
    assert_eq!(harness.fixture.provider.active_lease_count().await, 0);
    harness.stop().await
}

#[tokio::test]
async fn a_dispatch_without_a_reply_subject_still_launches() -> TestResult {
    let harness = Harness::start().await?;
    let body = serde_json::to_vec(&json!({ "lease_id": "lease-silent", "request": empty_lease()? }))?;

    harness
        .client
        .publish(provider_dispatch(ProviderKind::AzureContainerApps), Bytes::from(body))
        .await?;
    harness
        .client
        .publish(
            provider_dispatch(ProviderKind::AzureContainerApps),
            Bytes::from_static(b"garbage"),
        )
        .await?;

    let provider = harness.fixture.provider.clone();
    eventually(|| {
        let provider = provider.clone();
        async move { provider.active_lease_count().await == 1 }
    })
    .await?;
    harness.stop().await
}

#[tokio::test]
async fn a_failed_cancel_keeps_the_lease_held() -> TestResult {
    let harness = Harness::start().await?;
    harness.dispatch("lease-stuck", empty_lease()?).await?;
    let before = harness.fixture.aca.sessions().len();
    harness.fixture.aca.script_delete(Err(500));

    harness.cancel_lease("lease-stuck").await?;

    let aca = Arc::clone(&harness.fixture.aca);
    eventually(|| {
        let aca = aca.clone();
        async move { aca.sessions().len() > before }
    })
    .await?;
    assert_eq!(harness.fixture.provider.active_lease_count().await, 1);
    harness.stop().await
}

/// Pins today's behaviour, pending a decision on which one every worker should have: the cancel handler forgets the
/// grant before it cancels, so a redispatch after a failed cancel is not answered with the grant. The provider still
/// holds the session it could not delete, so it refuses to launch another and the redispatch gets a provider error.
/// The k8s, v8-wasm and kubevirt workers keep the grant until a cancel succeeds and answer with it instead.
#[tokio::test]
async fn a_redispatch_after_a_failed_cancel_is_refused_while_the_session_is_held() -> TestResult {
    let harness = Harness::start().await?;
    harness.dispatch("lease-stuck", empty_lease()?).await?;
    let before = harness.fixture.aca.sessions().len();
    harness.fixture.aca.script_delete(Err(500));

    harness.cancel_lease("lease-stuck").await?;
    let aca = Arc::clone(&harness.fixture.aca);
    eventually(|| {
        let aca = aca.clone();
        async move { aca.sessions().len() > before }
    })
    .await?;
    let reply = harness.dispatch("lease-stuck", empty_lease()?).await?;

    assert!(
        reply["provider_error"]
            .as_str()
            .is_some_and(|error| error.contains("settle cleanup before reallocating")),
        "{reply}"
    );
    assert_eq!(harness.fixture.provider.active_lease_count().await, 1);
    harness.stop().await
}

#[tokio::test]
async fn a_provider_bound_to_another_worker_cannot_serve() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let fixture = Fixture::new()?;
    fixture.provider.bind_worker(&WorkerId::new("someone-else")?)?;

    let result = run_worker(
        server.client().await,
        fixture.provider.clone(),
        WorkerId::new("worker-under-test")?,
        CancellationToken::new(),
    )
    .await;

    assert!(matches!(result, Err(WorkerError::Bind(_))));
    Ok(())
}
