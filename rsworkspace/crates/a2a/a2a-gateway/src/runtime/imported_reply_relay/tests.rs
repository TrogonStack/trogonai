use super::*;
use futures::StreamExt;
use trogon_nats::test_support::CoreTestServer;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn released(relay: &ImportedReplyRelay) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        while relay.permits.available_permits() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn capacity_refuses_another_request_and_shutdown_releases_the_slot() -> TestResult {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut agent = client.subscribe("agent").await?;
    client.flush().await?;
    let relay = ImportedReplyRelay::new(1);
    let shutdown = CancellationToken::new();
    relay.forward(
        client.clone(),
        "agent".into(),
        "caller".into(),
        Request::new(),
        Duration::from_secs(30),
        shutdown.clone(),
    )?;
    let _message = tokio::time::timeout(Duration::from_secs(5), agent.next())
        .await?
        .ok_or("agent subscription ended")?;
    assert!(matches!(
        relay.forward(
            client.clone(),
            "agent".into(),
            "caller".into(),
            Request::new(),
            Duration::from_secs(30),
            shutdown.clone()
        ),
        Err(TryAcquireError::NoPermits)
    ));
    shutdown.cancel();
    released(&relay).await
}

#[tokio::test]
async fn expiry_and_transport_failure_release_the_slot() -> TestResult {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let _agent = client.subscribe("agent").await?;
    client.flush().await?;
    let relay = ImportedReplyRelay::new(1);
    relay.forward(
        client.clone(),
        "agent".into(),
        "caller".into(),
        Request::new(),
        Duration::from_millis(10),
        CancellationToken::new(),
    )?;
    released(&relay).await?;
    relay.forward(
        client,
        "missing".into(),
        "caller".into(),
        Request::new(),
        Duration::from_secs(5),
        CancellationToken::new(),
    )?;
    released(&relay).await
}
