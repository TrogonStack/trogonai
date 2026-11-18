use std::time::Duration;

use futures::StreamExt as _;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerMethod, PeerRequestError, PeerRequestTimeout, PeerRequester,
    PeerRequesterConfig, PeerTenant,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn configuration(
    account: PeerAccount,
    caller: &str,
    connection: &str,
) -> Result<PeerRequesterConfig, Box<dyn std::error::Error + Send + Sync>> {
    Ok(PeerRequesterConfig::new(
        account,
        PeerCaller::new(caller)?,
        PeerConnectionId::new(connection)?,
        PeerRequestTimeout::new(Duration::from_secs(1))?,
    ))
}

#[tokio::test]
async fn configured_request_preserves_explicit_headers_and_reports_no_responders() -> TestResult {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let service = broker.client().await;
    let requester = PeerRequester::new(
        broker.client().await,
        configuration(
            PeerAccount::TenantAccount {
                tenant: PeerTenant::new("tenant")?,
            },
            "alice",
            "one",
        )?,
    );
    assert_eq!(requester.config().account().tenant().as_str(), "tenant");
    assert_eq!(requester.config().caller().as_str(), "alice");
    assert_eq!(requester.config().connection().as_str(), "one");
    assert_eq!(requester.config().timeout().as_duration(), Duration::from_secs(1));
    assert!(matches!(requester.request(PeerMethod::Status, Vec::new()).await,
        Err(PeerRequestError::Transport(error)) if error.kind() == async_nats::RequestErrorKind::NoResponders));
    let mut incoming = service.subscribe("peer.v1.alice.PeerService.Roster").await?;
    let barrier = service.new_inbox();
    let mut observed = service.subscribe(barrier.clone()).await?;
    service.publish(barrier, vec![0].into()).await?;
    tokio::time::timeout(Duration::from_secs(2), observed.next())
        .await?
        .ok_or("subscription barrier missing")?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("X-Correlation", "public-contract");
    let respond = async {
        let message = tokio::time::timeout(Duration::from_secs(2), incoming.next())
            .await?
            .ok_or("no request")?;
        assert_eq!(
            message
                .headers
                .as_ref()
                .and_then(|h| h.get("X-Correlation"))
                .map(|h| h.as_str()),
            Some("public-contract")
        );
        assert_eq!(message.payload.as_ref(), &[1, 2, 3]);
        service
            .publish(message.reply.ok_or("no scoped reply")?, vec![4].into())
            .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (response, served) = tokio::join!(
        requester.request_with_headers(PeerMethod::Roster, headers, vec![1, 2, 3]),
        respond
    );
    assert_eq!(response?.payload.as_ref(), &[4]);
    served?;
    Ok(())
}
