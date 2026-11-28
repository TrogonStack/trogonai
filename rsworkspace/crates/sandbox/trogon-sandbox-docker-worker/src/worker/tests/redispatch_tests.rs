//! A lease id dispatched twice launches once, and launches afresh once a cancel released it.

use futures::FutureExt;
use futures::future::BoxFuture;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_conformance::{CountingProvider, ensure_launches_once, ensure_relaunches_after_cancel};
use trogon_sandbox_contract::{ProviderKind, WorkerId};

use super::*;

type BoxError = Box<dyn Error + Send + Sync>;

/// The worker's own lease handler over `provider`, as the redispatch checks drive it.
fn lease_handler(
    client: Client,
    provider: Arc<dyn Provider>,
    active: ActiveLeases,
    grants: Arc<RedispatchGuard>,
) -> Result<impl Fn(async_nats::Message) -> BoxFuture<'static, anyhow::Result<()>>, Box<dyn Error>> {
    let worker_id = WorkerId::new("docker-test-worker")?;
    let js = jetstream::new(client.clone());
    Ok(move |msg| {
        handle_lease_msg(
            msg,
            client.clone(),
            js.clone(),
            Arc::clone(&provider),
            Arc::clone(&active),
            Arc::clone(&grants),
            LeaseWorkerContext {
                worker_id: worker_id.clone(),
                current_leases: Arc::new(AtomicU32::new(0)),
                docker_provider: None,
            },
        )
        .boxed()
    })
}

#[tokio::test]
async fn redispatching_a_lease_id_returns_the_first_grant_without_launching_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let counting = CountingProvider::new(ProviderKind::LocalDocker);
    let handle = lease_handler(
        client.clone(),
        counting.clone(),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(RedispatchGuard::new()),
    )?;

    ensure_launches_once(&client, &counting, &LeaseId::new("redispatch-lease"), handle).await?;
    Ok(())
}

/// Drives the release a cancel takes once it is admitted; the admission itself needs a Docker daemon.
#[tokio::test]
async fn a_released_lease_id_launches_afresh_when_dispatched_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let counting = CountingProvider::new(ProviderKind::LocalDocker);
    let provider: Arc<dyn Provider> = counting.clone();
    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let grants = Arc::new(RedispatchGuard::new());
    let terminals = Arc::new(DockerTerminals::with_replay_bytes(0));
    let handle = lease_handler(
        client.clone(),
        Arc::clone(&provider),
        Arc::clone(&active),
        Arc::clone(&grants),
    )?;
    let release_client = client.clone();
    let release = move |msg: async_nats::Message| {
        let (provider, active, grants, terminals, client) = (
            Arc::clone(&provider),
            Arc::clone(&active),
            Arc::clone(&grants),
            Arc::clone(&terminals),
            release_client.clone(),
        );
        async move {
            let lease_id = extract_cancel_lease_id(msg.subject.as_str()).ok_or("the cancel subject names no lease")?;
            release_lease(&lease_id, &provider, &active, &grants, &terminals, &client).await;
            Ok::<(), BoxError>(())
        }
    };

    ensure_relaunches_after_cancel(&client, &counting, &LeaseId::new("redispatch-cancel"), handle, release).await?;
    Ok(())
}
