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
    let (completed, mut completion) = tokio::sync::mpsc::unbounded_channel();
    relay.forward(
        client.clone(),
        "agent".into(),
        "caller".into(),
        Request::new(),
        Duration::from_secs(30),
        shutdown.clone(),
        move |result| async move {
            let _ = completed.send(result);
        },
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
            shutdown.clone(),
            |_| async {},
        ),
        Err(TryAcquireError::NoPermits)
    ));
    shutdown.cancel();
    released(&relay).await?;
    assert!(completion.try_recv().is_err(), "shutdown fabricated a completion");
    Ok(())
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
        |_| async {},
    )?;
    released(&relay).await?;
    relay.forward(
        client,
        "missing".into(),
        "caller".into(),
        Request::new(),
        Duration::from_secs(5),
        CancellationToken::new(),
        |_| async {},
    )?;
    released(&relay).await
}

#[tokio::test]
async fn transport_failures_reach_completion_and_release_capacity() -> TestResult {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut agent = client.subscribe("agent").await?;
    client.flush().await?;
    let relay = ImportedReplyRelay::new(1);
    for (subject, reply, expected) in [
        ("invalid subject", "caller", "gateway agent request failed"),
        ("agent", "invalid reply", "gateway caller reply publish failed"),
    ] {
        let (completed, completion) = tokio::sync::oneshot::channel();
        relay.forward(
            client.clone(),
            subject.into(),
            reply.into(),
            Request::new(),
            Duration::from_secs(5),
            CancellationToken::new(),
            move |result| async move {
                let _ = completed.send(result);
            },
        )?;
        if subject == "agent" {
            let request = tokio::time::timeout(Duration::from_secs(5), agent.next())
                .await?
                .ok_or("agent subscription ended")?;
            client
                .publish(request.reply.ok_or("missing reply")?, "response".into())
                .await?;
        }
        let error = tokio::time::timeout(Duration::from_secs(5), completion)
            .await??
            .expect_err("transport must fail");
        assert_eq!(error.code(), -32_803);
        assert_eq!(error.message(), expected);
        released(&relay).await?;
    }
    Ok(())
}
