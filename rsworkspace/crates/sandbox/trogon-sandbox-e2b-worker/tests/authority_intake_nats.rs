mod authority_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use authority_support::{Api, Boxed, lease_request};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LeaseId, ProviderKind, ProviderLease, WorkerId};
use trogon_sandbox_e2b_worker::terminal::{E2bGuestShell, E2bTerminals};
use trogon_sandbox_e2b_worker::{E2bWorkerError, Envd, run};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode, Provider, lifecycle_subject, nats_transfer, subjects};

struct Worker {
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), E2bWorkerError>>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl Worker {
    async fn stop(&mut self) -> Result<(), Boxed> {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task).await???;
        }
        Ok(())
    }
}

#[tokio::test]
async fn one_dispatch_allocates_once_across_two_worker_replicas() -> Result<(), Boxed> {
    let nats = JetStreamTestServer::start().await;
    let client = nats.client().await;
    let js = async_nats::jetstream::new(client.clone());
    let id = LeaseId::new("one-dispatch");
    js.create_stream(async_nats::jetstream::stream::Config {
        name: "intake_lifecycle".into(),
        subjects: vec![lifecycle_subject(&id)],
        ..Default::default()
    })
    .await?;
    let mut api = Api::start().await?;
    api.control.block_creates.store(true, Ordering::SeqCst);
    let providers = [api.provider()?, api.provider()?];
    let mut workers = Vec::new();
    for (index, provider) in providers.iter().enumerate() {
        let worker_client = nats.client().await;
        let envd = Envd::new(Duration::from_secs(3))?;
        let terminals = Arc::new(E2bTerminals::new(
            Arc::new(E2bGuestShell::new(provider.clone(), envd)),
            worker_client.clone(),
        ));
        let worker_id = WorkerId::new(format!("intake-worker-{index}"))?;
        let authority = AuthorityConfig::new(AuthorityMode::Off);
        let cancel = CancellationToken::new();
        let (provider, stop) = (provider.clone(), cancel.clone());
        let task = tokio::spawn(async move {
            run(
                worker_client.clone(),
                async_nats::jetstream::new(worker_client),
                provider,
                &worker_id,
                stop,
                terminals,
                authority,
            )
            .await
        });
        workers.push(Worker {
            cancel,
            task: Some(task),
        });
    }
    for provider in &providers {
        tokio::time::timeout(Duration::from_secs(15), provider.wait_ready()).await?;
    }

    let dispatch = serde_json::json!({"lease_id": id, "request": lease_request()?});
    let subject = subjects::provider_dispatch(ProviderKind::E2b);
    let pending = tokio::spawn({
        let client = client.clone();
        let payload = serde_json::to_vec(&dispatch)?;
        async move { nats_transfer::request(&client, &subject, payload.into(), None, Duration::from_secs(30)).await }
    });
    tokio::time::timeout(Duration::from_secs(5), api.control.first_create.notified()).await?;
    let duplicated = tokio::time::timeout(Duration::from_secs(2), api.control.second_create.notified())
        .await
        .is_ok();
    api.control.block_creates.store(false, Ordering::SeqCst);
    api.control.release_creates.notify_waiters();
    let response = tokio::time::timeout(Duration::from_secs(10), pending).await???;
    let grant: ProviderLease = serde_json::from_slice(&response)?;
    assert!(grant.sandbox_id.as_str().starts_with("vm-"));
    let allocated = api.control.creates.load(Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let held = providers[0].active_lease_count().await + providers[1].active_lease_count().await;
            if held == allocated {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    for provider in &providers {
        provider.cancel(&id).await?;
    }
    assert_eq!(
        api.control.live_count(),
        0,
        "every backend resource this dispatch created is removed"
    );
    for worker in &mut workers {
        worker.stop().await?;
    }
    api.shutdown().await;
    assert!(
        !duplicated,
        "CREATE must be consumed once even when both worker replicas are subscribed"
    );
    assert_eq!(allocated, 1);
    Ok(())
}
