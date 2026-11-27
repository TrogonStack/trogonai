use std::time::Duration;

use futures::StreamExt as _;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_peer_nats::{
    PeerAccount, PeerCaller, PeerConnectionId, PeerIngressError, PeerMethod, PeerReplySubjectError, PeerRequestError,
    PeerRequestTimeout, PeerRequester, PeerRequesterConfig, PeerTenant,
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
async fn full_route_and_reply_bounds_fail_with_their_typed_error() -> TestResult {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let tenant = "t".repeat(128);
    let caller = "c".repeat(128);
    let connection = "x".repeat(128);
    let route = PeerRequester::new(
        broker.client().await,
        configuration(
            PeerAccount::SharedFixture {
                tenant: PeerTenant::new(&tenant)?,
            },
            &caller,
            "one",
        )?,
    );
    assert!(matches!(
        route.request(PeerMethod::Enroll, Vec::new()).await,
        Err(PeerRequestError::Ingress(PeerIngressError::SubjectTooLong))
    ));
    let reply = PeerRequester::new(
        broker.client().await,
        configuration(
            PeerAccount::TenantAccount {
                tenant: PeerTenant::new(&tenant)?,
            },
            &caller,
            &connection,
        )?,
    );
    assert!(matches!(
        reply.request(PeerMethod::Enroll, Vec::new()).await,
        Err(PeerRequestError::Reply(PeerReplySubjectError::SubjectTooLong))
    ));
    Ok(())
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
    let respond = async {
        let message = tokio::time::timeout(Duration::from_secs(2), incoming.next())
            .await?
            .ok_or("no request")?;
        assert!(message.headers.is_none());
        assert_eq!(message.payload.as_ref(), &[1, 2, 3]);
        service
            .publish(message.reply.ok_or("no scoped reply")?, vec![4].into())
            .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    };
    let (response, served) = tokio::join!(requester.request(PeerMethod::Roster, vec![1, 2, 3]), respond);
    assert_eq!(response?.payload.as_ref(), &[4]);
    served?;
    let mut explicit = async_nats::HeaderMap::new();
    explicit.insert("X-Peer-Test", "preserved");
    for headers in [async_nats::HeaderMap::new(), explicit] {
        let expected = if headers.is_empty() {
            None
        } else {
            Some(headers.clone())
        };
        let respond = async {
            let message = tokio::time::timeout(Duration::from_secs(2), incoming.next())
                .await?
                .ok_or("no explicit-header request")?;
            assert_eq!(message.headers, expected);
            assert_eq!(message.payload.as_ref(), &[5, 6]);
            service
                .publish(message.reply.ok_or("no scoped reply")?, vec![7].into())
                .await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        };
        let (response, served) = tokio::join!(
            requester.request_with_headers(PeerMethod::Roster, headers, vec![5, 6]),
            respond
        );
        assert_eq!(response?.payload.as_ref(), &[7]);
        served?;
    }
    Ok(())
}

#[tokio::test]
async fn silent_request_expires_at_its_configured_whole_operation_deadline() -> TestResult {
    let broker = JetStreamTestServer::start_with_atomic_batches().await;
    let service = broker.client().await;
    let _silent = service.subscribe("peer.v1.alice.PeerService.Status").await?;
    service.flush().await?;
    let timeout = Duration::from_millis(50);
    let requester = PeerRequester::new(
        broker.client().await,
        PeerRequesterConfig::new(
            PeerAccount::TenantAccount {
                tenant: PeerTenant::new("tenant")?,
            },
            PeerCaller::new("alice")?,
            PeerConnectionId::new("one")?,
            PeerRequestTimeout::new(timeout)?,
        ),
    );
    tokio::time::pause();
    let request = async {
        let result = requester.request(PeerMethod::Status, Vec::new()).await;
        (result, tokio::time::Instant::now())
    };
    let reference = async {
        assert!(
            tokio::time::timeout(timeout, std::future::pending::<()>())
                .await
                .is_err()
        );
        tokio::time::Instant::now()
    };
    let ((response, completed), deadline) = tokio::join!(request, reference);
    tokio::time::resume();
    assert_eq!(completed, deadline);
    assert!(matches!(response,
        Err(PeerRequestError::Transport(error)) if error.kind() == async_nats::RequestErrorKind::TimedOut));
    Ok(())
}
