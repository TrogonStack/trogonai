//! A lease id dispatched twice launches once.

use std::time::Duration;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::WorkerId;

use super::counting_provider::CountingProvider;
use super::lease_request::sample_lease_request;
use super::*;

async fn dispatch_reply(
    client: &Client,
    js: &jetstream::Context,
    provider: &Arc<dyn Provider>,
    active: &ActiveLeases,
    grants: &Arc<RedispatchGuard>,
    lease_id: &LeaseId,
) -> Result<ProviderLease, Box<dyn Error>> {
    let payload = ProviderDispatchPayload {
        lease_id: lease_id.clone(),
        request: sample_lease_request()?,
        correlation_id: None,
        resolved_drives: Vec::new(),
        target: None,
    };
    let inbox = client.new_inbox();
    let mut replies = client.subscribe(inbox.clone()).await?;
    let body = serde_json::to_vec(&payload)?;
    let msg = async_nats::Message {
        subject: "dispatch".into(),
        reply: Some(inbox.into()),
        length: body.len(),
        payload: body.into(),
        headers: None,
        status: None,
        description: None,
    };
    handle_lease_msg(
        msg,
        client.clone(),
        js.clone(),
        Arc::clone(provider),
        Arc::clone(active),
        Arc::clone(grants),
        LeaseWorkerContext {
            worker_id: WorkerId::new("docker-test-worker")?,
            current_leases: Arc::new(AtomicU32::new(0)),
            docker_provider: None,
        },
    )
    .await?;
    let reply = tokio::time::timeout(Duration::from_secs(5), replies.next())
        .await?
        .ok_or("reply subscription closed")?;
    Ok(serde_json::from_slice(&reply.payload)?)
}

#[tokio::test]
async fn redispatching_a_lease_id_returns_the_first_grant_without_launching_again() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let js = jetstream::new(client.clone());
    let counting = Arc::new(CountingProvider::default());
    let provider: Arc<dyn Provider> = counting.clone();
    let active: ActiveLeases = Arc::new(Mutex::new(HashMap::new()));
    let grants = Arc::new(RedispatchGuard::new());
    let lease_id = LeaseId::new("redispatch-lease");

    let first = dispatch_reply(&client, &js, &provider, &active, &grants, &lease_id).await?;
    let second = dispatch_reply(&client, &js, &provider, &active, &grants, &lease_id).await?;

    assert_eq!(first, second);
    assert_eq!(counting.launches(), 1, "the repeat must not start a second sandbox");

    grants.forget(&lease_id);
    let fresh = dispatch_reply(&client, &js, &provider, &active, &grants, &lease_id).await?;
    assert_ne!(fresh.sandbox_id, first.sandbox_id, "a released id launches afresh");
    Ok(())
}
