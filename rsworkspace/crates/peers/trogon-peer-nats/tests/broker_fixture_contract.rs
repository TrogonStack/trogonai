use futures::StreamExt as _;
use trogon_nats::{Location, NKeySeed, NatsAuth};
use trogon_peer_nats::test_support::broker::{Broker, TestResult, observed_barrier};

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("authenticated request probe timed out")]
    Request(#[source] tokio::time::error::Elapsed),
    #[error("authenticated reply probe timed out")]
    Reply {
        #[source]
        source: tokio::time::error::Elapsed,
        server_error: Option<async_nats::ServerError>,
    },
    #[error("cross-principal permission denial probe timed out")]
    Denial(#[source] tokio::time::error::Elapsed),
}

#[tokio::test]
async fn exported_fixture_credentials_authenticate_their_restricted_identity_and_expire_with_the_fixture() -> TestResult
{
    let broker = Broker::start().await?;
    assert!(broker.credentials_are_ephemeral());
    assert!(broker.credential_path("unregistered").is_err());
    let path = broker.credential_path("alice")?;
    assert!(path.is_file());
    assert_ne!(path, broker.credential_path("bob")?);
    let (send, mut errors) = tokio::sync::mpsc::channel(1);
    let options = broker
        .server
        .connect_options(NatsAuth::NKey(NKeySeed::File(Location::Path(path.clone()))))
        .await?
        .event_callback(move |event| {
            let send = send.clone();
            async move {
                if let async_nats::Event::ServerError(error) = event {
                    let _ = send.try_send(error);
                }
            }
        });
    let alice = options.connect(broker.server.address()).await?;
    assert!(alice.server_info().tls_required);
    let (service, _) = broker.connect("service").await?;
    let subject = "tenant.peer.v1.alice.PeerService.Enroll";
    let mut requests = service.subscribe(subject).await?;
    observed_barrier(&service).await?;
    let reply = "_INBOX.tenant.alice.one.fixture_probe";
    let mut replies = alice.subscribe(reply).await?;
    alice.publish_with_reply(subject, reply, vec![3].into()).await?;
    let request = tokio::time::timeout(PROBE_TIMEOUT, requests.next())
        .await
        .map_err(ProbeError::Request)?
        .ok_or("authenticated fixture request was not delivered")?;
    assert_eq!(request.subject.as_str(), subject);
    assert_eq!(request.reply.as_ref().map(|subject| subject.as_str()), Some(reply));
    assert_eq!(request.payload.as_ref(), &[3]);
    service
        .publish(
            request.reply.ok_or("authenticated request omitted its reply")?,
            vec![7].into(),
        )
        .await?;
    let message = tokio::time::timeout(PROBE_TIMEOUT, replies.next())
        .await
        .map_err(|source| ProbeError::Reply {
            source,
            server_error: errors.try_recv().ok(),
        })?
        .ok_or("authenticated fixture reply was not delivered")?;
    assert_eq!(message.payload.as_ref(), &[7]);
    let forbidden = "_INBOX.tenant.bob.one.fixture_probe";
    let denied = alice.subscribe(forbidden).await?;
    alice.flush().await?;
    let error = tokio::time::timeout(PROBE_TIMEOUT, errors.recv())
        .await
        .map_err(ProbeError::Denial)?
        .ok_or("fixture credential did not enforce its principal permissions")?;
    assert_eq!(
        error,
        async_nats::ServerError::Other(format!("Permissions Violation for Subscription to {forbidden:?}"))
    );
    drop(denied);
    drop(replies);
    drop(requests);
    drop(alice);
    drop(service);
    drop(broker);
    assert!(!path.exists());
    Ok(())
}
